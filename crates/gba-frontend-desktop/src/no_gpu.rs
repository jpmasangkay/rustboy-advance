use std::collections::HashSet;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gba_core::{GbaCore, HostFrontend, LCD_HEIGHT, LCD_WIDTH};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use crate::audio::AudioOutput;
use crate::config::{AppConfig, DisplayFilter, KeyBindings};
use crate::filters::{
    FilterCost, GOVERNOR_MAX_CAP_HEIGHT, GOVERNOR_MAX_CAP_WIDTH, QualityGovernor,
    VideoPostProcessor, sort_filter_costs,
};
use crate::headless::CliConfig;

const GBA_FRAME_DURATION: Duration = Duration::from_nanos(16_742_706);

#[derive(Clone, Debug, serde::Serialize)]
pub struct SoftbufferSizeBenchRow {
    pub present_mode: String,
    pub resolution_label: String,
    pub width: u32,
    pub height: u32,
    pub filter: String,
    pub slowdown_x: usize,
    pub frames: usize,
    pub present_only_avg_ms: f64,
    pub present_only_p95_ms: f64,
    pub total_work_avg_ms: f64,
    pub total_work_p95_ms: f64,
    pub total_work_p99_ms: f64,
    pub total_work_worst_ms: f64,
    pub over_budget_pct: f64,
    pub fits_budget: bool,
}

#[inline]
pub fn compute_aspect_rect(
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> (usize, usize, usize, usize) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w, dst_h);
    }
    let scale_w = dst_w as f64 / src_w as f64;
    let scale_h = dst_h as f64 / src_h as f64;
    let scale = scale_w.min(scale_h);
    let draw_w = ((src_w as f64 * scale).round() as usize).clamp(1, dst_w);
    let draw_h = ((src_h as f64 * scale).round() as usize).clamp(1, dst_h);
    let off_x = (dst_w - draw_w) / 2;
    let off_y = (dst_h - draw_h) / 2;
    (off_x, off_y, draw_w, draw_h)
}

#[inline]
pub fn capped_source_dims(src_w: usize, src_h: usize) -> (usize, usize) {
    if src_w <= GOVERNOR_MAX_CAP_WIDTH && src_h <= GOVERNOR_MAX_CAP_HEIGHT {
        return (src_w.max(1), src_h.max(1));
    }
    let (_, _, draw_w, draw_h) = compute_aspect_rect(
        src_w,
        src_h,
        GOVERNOR_MAX_CAP_WIDTH,
        GOVERNOR_MAX_CAP_HEIGHT,
    );
    (draw_w.max(1), draw_h.max(1))
}

#[cfg(target_os = "windows")]
mod win32_gdi {
    use super::compute_aspect_rect;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    const BI_RGB: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const BLACKNESS: u32 = 0x0000_0042;
    pub(super) const COLORONCOLOR: i32 = 3;
    pub(super) const HALFTONE: i32 = 4;

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [u32; 1],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(hwnd: isize) -> isize;
        fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn SetStretchBltMode(hdc: isize, mode: i32) -> i32;
        fn SetBrushOrgEx(hdc: isize, x: i32, y: i32, lppt: *mut [i32; 2]) -> i32;
        fn PatBlt(hdc: isize, x: i32, y: i32, w: i32, h: i32, rop: u32) -> i32;
        fn StretchDIBits(
            hdc: isize,
            x_dest: i32,
            y_dest: i32,
            dest_width: i32,
            dest_height: i32,
            x_src: i32,
            y_src: i32,
            src_width: i32,
            src_height: i32,
            lp_bits: *const core::ffi::c_void,
            lpbmi: *const BitmapInfo,
            i_usage: u32,
            rop: u32,
        ) -> i32;
    }

    /// Presents a compact top-down `0x00RRGGBB` bitmap (at most `720x480`) to a Win32 window via
    /// `StretchDIBits`, painting clean black letterbox margins outside the 3:2 viewport.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn present_stretch_dibits(
        window: &Window,
        xrgb: &[u32],
        src_w: usize,
        src_h: usize,
        dst_w: usize,
        dst_h: usize,
        halftone: bool,
    ) -> bool {
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 || xrgb.len() < src_w * src_h {
            return false;
        }
        let Ok(handle) = window.window_handle() else {
            return false;
        };
        let hwnd = match handle.as_raw() {
            RawWindowHandle::Win32(h) => h.hwnd.get(),
            _ => return false,
        };
        let hdc = unsafe { GetDC(hwnd) };
        if hdc == 0 {
            return false;
        }

        let (off_x, off_y, draw_w, draw_h) = compute_aspect_rect(src_w, src_h, dst_w, dst_h);
        if off_x > 0 {
            unsafe {
                PatBlt(hdc, 0, 0, off_x as i32, dst_h as i32, BLACKNESS);
                let right_x = off_x + draw_w;
                if dst_w > right_x {
                    PatBlt(
                        hdc,
                        right_x as i32,
                        0,
                        (dst_w - right_x) as i32,
                        dst_h as i32,
                        BLACKNESS,
                    );
                }
            }
        }
        if off_y > 0 {
            unsafe {
                PatBlt(hdc, 0, 0, dst_w as i32, off_y as i32, BLACKNESS);
                let bottom_y = off_y + draw_h;
                if dst_h > bottom_y {
                    PatBlt(
                        hdc,
                        0,
                        bottom_y as i32,
                        dst_w as i32,
                        (dst_h - bottom_y) as i32,
                        BLACKNESS,
                    );
                }
            }
        }

        let mode = if halftone { HALFTONE } else { COLORONCOLOR };
        unsafe {
            SetStretchBltMode(hdc, mode);
            if halftone {
                SetBrushOrgEx(hdc, 0, 0, std::ptr::null_mut());
            }
        }

        let bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: src_w as i32,
                bi_height: -(src_h as i32), // top-down DIB
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };

        let copied = unsafe {
            StretchDIBits(
                hdc,
                off_x as i32,
                off_y as i32,
                draw_w as i32,
                draw_h as i32,
                0,
                0,
                src_w as i32,
                src_h as i32,
                xrgb.as_ptr().cast(),
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        unsafe {
            ReleaseDC(hwnd, hdc);
        }
        copied > 0
    }
}

#[derive(Clone, Copy)]
struct BilinearXTap {
    x0: u16,
    x1: u16,
    wx: u16,
    iwx: u16,
}

/// Pure-CPU XRGB8 (`0x00RRGGBB`) software rasterizer & letterbox blitter used both by the
/// live `winit + softbuffer` window backend and headless `--no-gpu` benchmarks.
pub struct NoGpuCpuPresenter {
    x_map: Vec<usize>,
    cached_dims: (usize, usize, usize, usize),
    x_bilinear_lut: Vec<BilinearXTap>,
    cached_bilinear_dims: (usize, usize),
    h_row0: Vec<u32>,
    h_row1: Vec<u32>,
    pub scratch_xrgb: Vec<u32>,
}

impl NoGpuCpuPresenter {
    pub fn new(dst_w: usize, dst_h: usize) -> Self {
        Self {
            x_map: Vec::new(),
            cached_dims: (0, 0, 0, 0),
            x_bilinear_lut: Vec::new(),
            cached_bilinear_dims: (0, 0),
            h_row0: Vec::new(),
            h_row1: Vec::new(),
            scratch_xrgb: vec![0u32; dst_w.saturating_mul(dst_h)],
        }
    }

    pub fn present_rgba_to_xrgb(
        &mut self,
        src: &[egui::Color32],
        src_w: usize,
        src_h: usize,
        dst_w: usize,
        dst_h: usize,
        linear: bool,
    ) -> &[u32] {
        let needed = dst_w.saturating_mul(dst_h);
        if self.scratch_xrgb.len() != needed {
            self.scratch_xrgb.resize(needed, 0);
        }
        Self::blit_inner(
            &mut self.x_map,
            &mut self.cached_dims,
            &mut self.x_bilinear_lut,
            &mut self.cached_bilinear_dims,
            &mut self.h_row0,
            &mut self.h_row1,
            src,
            src_w,
            src_h,
            &mut self.scratch_xrgb,
            dst_w,
            dst_h,
            linear,
        );
        &self.scratch_xrgb
    }

    pub fn blit_rgba_to_slice(
        &mut self,
        src: &[egui::Color32],
        src_w: usize,
        src_h: usize,
        dst: &mut [u32],
        dst_w: usize,
        dst_h: usize,
        linear: bool,
    ) {
        Self::blit_inner(
            &mut self.x_map,
            &mut self.cached_dims,
            &mut self.x_bilinear_lut,
            &mut self.cached_bilinear_dims,
            &mut self.h_row0,
            &mut self.h_row1,
            src,
            src_w,
            src_h,
            dst,
            dst_w,
            dst_h,
            linear,
        );
    }

    #[inline]
    fn fill_letterbox_borders(
        dst: &mut [u32],
        dst_w: usize,
        dst_h: usize,
        off_x: usize,
        off_y: usize,
        draw_w: usize,
        draw_h: usize,
    ) {
        const BG: u32 = 0x000B_0C10;
        if off_y > 0 {
            dst[..off_y * dst_w].fill(BG);
        }
        let bottom_y = off_y + draw_h;
        if bottom_y < dst_h {
            dst[bottom_y * dst_w..dst_w * dst_h].fill(BG);
        }
        let right_x = off_x + draw_w;
        if off_x > 0 || right_x < dst_w {
            for y in off_y..bottom_y {
                let row = &mut dst[y * dst_w..(y + 1) * dst_w];
                if off_x > 0 {
                    row[..off_x].fill(BG);
                }
                if right_x < dst_w {
                    row[right_x..].fill(BG);
                }
            }
        }
    }

    #[inline]
    fn horiz_interp_row(src_row: &[egui::Color32], taps: &[BilinearXTap], out: &mut [u32]) {
        for (dst_px, tap) in out.iter_mut().zip(taps.iter()) {
            let c0 = src_row[tap.x0 as usize];
            let c1 = src_row[tap.x1 as usize];
            let wx = tap.wx as u32;
            let iwx = tap.iwx as u32;
            let r = (c0.r() as u32 * iwx + c1.r() as u32 * wx) >> 8;
            let g = (c0.g() as u32 * iwx + c1.g() as u32 * wx) >> 8;
            let b = (c0.b() as u32 * iwx + c1.b() as u32 * wx) >> 8;
            *dst_px = (r << 16) | (g << 8) | b;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn blit_inner(
        x_map: &mut Vec<usize>,
        cached_dims: &mut (usize, usize, usize, usize),
        x_bilinear_lut: &mut Vec<BilinearXTap>,
        cached_bilinear_dims: &mut (usize, usize),
        h_row0: &mut Vec<u32>,
        h_row1: &mut Vec<u32>,
        src: &[egui::Color32],
        src_w: usize,
        src_h: usize,
        dst: &mut [u32],
        dst_w: usize,
        dst_h: usize,
        linear: bool,
    ) {
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 || src.len() < src_w * src_h {
            dst.fill(0);
            return;
        }
        if dst.len() < dst_w * dst_h {
            return;
        }

        // Fast 1:1 exact pixel copy (e.g. XbrSmooth3x at 720x480 or Native 240x160).
        if src_w == dst_w && src_h == dst_h {
            for (d, c) in dst[..dst_w * dst_h].iter_mut().zip(&src[..src_w * src_h]) {
                *d = ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | (c.b() as u32);
            }
            return;
        }

        // Preserve GBA 3:2 aspect ratio centered inside arbitrary window dimensions.
        let (off_x, off_y, draw_w, draw_h) = compute_aspect_rect(src_w, src_h, dst_w, dst_h);

        if off_x > 0 || off_y > 0 {
            Self::fill_letterbox_borders(dst, dst_w, dst_h, off_x, off_y, draw_w, draw_h);
        }

        // Exact nearest / integer upscaling with vertical row reuse (memcpy repeated source rows).
        if !linear || (draw_w.is_multiple_of(src_w) && draw_h.is_multiple_of(src_h)) {
            if *cached_dims != (src_w, src_h, draw_w, draw_h) {
                x_map.resize(draw_w, 0);
                for (dx, slot) in x_map.iter_mut().enumerate() {
                    *slot = (dx * src_w) / draw_w;
                }
                *cached_dims = (src_w, src_h, draw_w, draw_h);
            }
            let mut prev_sy = usize::MAX;
            let mut prev_row_start = 0usize;
            for dy in 0..draw_h {
                let sy = (dy * src_h) / draw_h;
                let row_start = (off_y + dy) * dst_w + off_x;
                if sy == prev_sy {
                    dst.copy_within(prev_row_start..prev_row_start + draw_w, row_start);
                    continue;
                }
                let src_row = &src[sy * src_w..(sy + 1) * src_w];
                let dst_row = &mut dst[row_start..row_start + draw_w];
                for (d, &sx) in dst_row.iter_mut().zip(x_map.iter()) {
                    let c = src_row[sx];
                    *d = ((c.r() as u32) << 16) | ((c.g() as u32) << 8) | (c.b() as u32);
                }
                prev_sy = sy;
                prev_row_start = row_start;
            }
            return;
        }

        // Non-integer bilinear path: separable horizontal LUT + cached source-row pairs + SWAR vertical blend.
        if *cached_bilinear_dims != (src_w, draw_w) || x_bilinear_lut.len() != draw_w {
            x_bilinear_lut.resize(
                draw_w,
                BilinearXTap {
                    x0: 0,
                    x1: 0,
                    wx: 0,
                    iwx: 256,
                },
            );
            for (dx, tap) in x_bilinear_lut.iter_mut().enumerate() {
                let fx = ((dx as u64 * src_w as u64) << 16) / draw_w.max(1) as u64;
                let x0 = ((fx >> 16) as usize).min(src_w - 1);
                let x1 = (x0 + 1).min(src_w - 1);
                let wx = ((fx & 0xFFFF) >> 8) as u16;
                *tap = BilinearXTap {
                    x0: x0 as u16,
                    x1: x1 as u16,
                    wx,
                    iwx: 256 - wx,
                };
            }
            *cached_bilinear_dims = (src_w, draw_w);
        }
        if h_row0.len() < draw_w {
            h_row0.resize(draw_w, 0);
        }
        if h_row1.len() < draw_w {
            h_row1.resize(draw_w, 0);
        }
        let taps = &x_bilinear_lut[..draw_w];
        let mut cached_y0 = usize::MAX;
        let mut cached_y1 = usize::MAX;

        for dy in 0..draw_h {
            let fy = ((dy as u64 * src_h as u64) << 16) / draw_h.max(1) as u64;
            let y0 = ((fy >> 16) as usize).min(src_h - 1);
            let y1 = (y0 + 1).min(src_h - 1);
            let wy = ((fy & 0xFFFF) >> 8) as u32;
            let iwy = 256 - wy;

            if y0 != cached_y0 {
                if y0 == cached_y1 {
                    std::mem::swap(h_row0, h_row1);
                    cached_y0 = y0;
                    cached_y1 = usize::MAX;
                } else {
                    Self::horiz_interp_row(
                        &src[y0 * src_w..(y0 + 1) * src_w],
                        taps,
                        &mut h_row0[..draw_w],
                    );
                    cached_y0 = y0;
                }
            }
            if y1 != cached_y1 {
                if y1 == y0 {
                    h_row1[..draw_w].copy_from_slice(&h_row0[..draw_w]);
                } else {
                    Self::horiz_interp_row(
                        &src[y1 * src_w..(y1 + 1) * src_w],
                        taps,
                        &mut h_row1[..draw_w],
                    );
                }
                cached_y1 = y1;
            }

            let dst_row =
                &mut dst[(off_y + dy) * dst_w + off_x..(off_y + dy) * dst_w + off_x + draw_w];
            if wy == 0 {
                dst_row.copy_from_slice(&h_row0[..draw_w]);
            } else {
                for ((d, &p0), &p1) in dst_row
                    .iter_mut()
                    .zip(&h_row0[..draw_w])
                    .zip(&h_row1[..draw_w])
                {
                    let rb0 = p0 & 0x00FF_00FF;
                    let rb1 = p1 & 0x00FF_00FF;
                    let rb = ((rb0 * iwy + rb1 * wy) >> 8) & 0x00FF_00FF;
                    let g0 = p0 & 0x0000_FF00;
                    let g1 = p1 & 0x0000_FF00;
                    let g = ((g0 * iwy + g1 * wy) >> 8) & 0x0000_FF00;
                    *d = rb | g;
                }
            }
        }
    }

    pub fn measure_filter_costs(
        &mut self,
        postproc: &mut VideoPostProcessor,
        color_correction: bool,
        lcd_ghosting: bool,
        grid_intensity: f32,
        dst_w: usize,
        dst_h: usize,
    ) -> Vec<FilterCost> {
        let pattern = crate::filters::test_filter_pattern_fb();
        let mut costs = Vec::with_capacity(DisplayFilter::ALL.len());
        for &filter in &DisplayFilter::ALL {
            postproc.prewarm(color_correction, lcd_ghosting, filter, false, grid_intensity);
            let linear = filter.texture_options() == egui::TextureOptions::LINEAR;
            for _ in 0..2 {
                let img = postproc.process_display_filter(
                    &pattern,
                    color_correction,
                    lcd_ghosting,
                    filter,
                    false,
                    grid_intensity,
                );
                self.present_rgba_to_xrgb(&img.pixels, img.size[0], img.size[1], dst_w, dst_h, linear);
            }
            let mut samples = [0.0f64; 3];
            for slot in &mut samples {
                let t0 = Instant::now();
                for _ in 0..4 {
                    let img = postproc.process_display_filter(
                        &pattern,
                        color_correction,
                        lcd_ghosting,
                        filter,
                        false,
                        grid_intensity,
                    );
                    self.present_rgba_to_xrgb(
                        &img.pixels,
                        img.size[0],
                        img.size[1],
                        dst_w,
                        dst_h,
                        linear,
                    );
                }
                *slot = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;
            }
            samples.sort_by(f64::total_cmp);
            costs.push(FilterCost {
                filter,
                cost_ms: samples[1],
            });
        }
        sort_filter_costs(&mut costs);
        costs
    }

    /// Presents `img` capped at most `720x480` via Win32 `StretchDIBits` (`None` on non-Windows).
    #[allow(unused_variables)]
    pub fn present_capped_os_stretch(
        &mut self,
        window: &Window,
        src: &[egui::Color32],
        src_w: usize,
        src_h: usize,
        dst_w: usize,
        dst_h: usize,
        halftone: bool,
    ) -> bool {
        let (cap_w, cap_h) = capped_source_dims(src_w, src_h);
        let xrgb_len = cap_w.saturating_mul(cap_h);
        self.present_rgba_to_xrgb(src, src_w, src_h, cap_w, cap_h, halftone);
        #[cfg(target_os = "windows")]
        {
            return win32_gdi::present_stretch_dibits(
                window,
                &self.scratch_xrgb[..xrgb_len],
                cap_w,
                cap_h,
                dst_w,
                dst_h,
                halftone,
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    pub fn measure_capped_filter_costs(
        &mut self,
        postproc: &mut VideoPostProcessor,
        color_correction: bool,
        lcd_ghosting: bool,
        grid_intensity: f32,
        dst_w: usize,
        dst_h: usize,
        window: Option<&Window>,
    ) -> Vec<FilterCost> {
        let pattern = crate::filters::test_filter_pattern_fb();
        let mut costs = Vec::with_capacity(DisplayFilter::ALL.len());
        for &filter in &DisplayFilter::ALL {
            postproc.prewarm(color_correction, lcd_ghosting, filter, false, grid_intensity);
            let linear = filter.texture_options() == egui::TextureOptions::LINEAR;
            for _ in 0..2 {
                let img = postproc.process_display_filter(
                    &pattern,
                    color_correction,
                    lcd_ghosting,
                    filter,
                    false,
                    grid_intensity,
                );
                let (cw, ch) = capped_source_dims(img.size[0], img.size[1]);
                self.present_rgba_to_xrgb(&img.pixels, img.size[0], img.size[1], cw, ch, linear);
            }
            let mut samples = [0.0f64; 3];
            for slot in &mut samples {
                let t0 = Instant::now();
                for _ in 0..4 {
                    let img = postproc.process_display_filter(
                        &pattern,
                        color_correction,
                        lcd_ghosting,
                        filter,
                        false,
                        grid_intensity,
                    );
                    if let Some(w) = window {
                        let _ = self.present_capped_os_stretch(
                            w,
                            &img.pixels,
                            img.size[0],
                            img.size[1],
                            dst_w,
                            dst_h,
                            false,
                        );
                    } else {
                        let (cw, ch) = capped_source_dims(img.size[0], img.size[1]);
                        self.present_rgba_to_xrgb(
                            &img.pixels,
                            img.size[0],
                            img.size[1],
                            cw,
                            ch,
                            linear,
                        );
                    }
                }
                *slot = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;
            }
            samples.sort_by(f64::total_cmp);
            costs.push(FilterCost {
                filter,
                cost_ms: samples[1],
            });
        }
        sort_filter_costs(&mut costs);
        costs
    }
}

struct NoGpuSink {
    audio: AudioOutput,
    framebuffer: [u16; LCD_WIDTH * LCD_HEIGHT],
    keypad: u16,
    frame_dirty: bool,
}

impl NoGpuSink {
    fn new(audio: AudioOutput) -> Self {
        Self {
            audio,
            framebuffer: [0u16; LCD_WIDTH * LCD_HEIGHT],
            keypad: 0x03FF,
            frame_dirty: true,
        }
    }
}

impl HostFrontend for NoGpuSink {
    fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        self.framebuffer.copy_from_slice(fb);
        self.frame_dirty = true;
    }

    fn push_audio_samples(&mut self, samples: &[i16]) {
        self.audio.push_interleaved(samples);
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad
    }
}

fn binding_matches_keycode(binding: &str, code: KeyCode) -> bool {
    let b = binding.trim();
    match code {
        KeyCode::ArrowUp => b.eq_ignore_ascii_case("ArrowUp") || b.eq_ignore_ascii_case("Up"),
        KeyCode::ArrowDown => b.eq_ignore_ascii_case("ArrowDown") || b.eq_ignore_ascii_case("Down"),
        KeyCode::ArrowLeft => b.eq_ignore_ascii_case("ArrowLeft") || b.eq_ignore_ascii_case("Left"),
        KeyCode::ArrowRight => {
            b.eq_ignore_ascii_case("ArrowRight") || b.eq_ignore_ascii_case("Right")
        }
        KeyCode::Enter | KeyCode::NumpadEnter => {
            b.eq_ignore_ascii_case("Enter") || b.eq_ignore_ascii_case("Return")
        }
        KeyCode::Backspace => b.eq_ignore_ascii_case("Backspace"),
        KeyCode::Space => b.eq_ignore_ascii_case("Space") || b == " ",
        KeyCode::ShiftLeft | KeyCode::ShiftRight => {
            b.eq_ignore_ascii_case("Shift")
                || b.eq_ignore_ascii_case("LShift")
                || b.eq_ignore_ascii_case("RShift")
        }
        KeyCode::Tab => b.eq_ignore_ascii_case("Tab"),
        KeyCode::Escape => b.eq_ignore_ascii_case("Escape") || b.eq_ignore_ascii_case("Esc"),
        KeyCode::KeyA => b.eq_ignore_ascii_case("A"),
        KeyCode::KeyB => b.eq_ignore_ascii_case("B"),
        KeyCode::KeyC => b.eq_ignore_ascii_case("C"),
        KeyCode::KeyD => b.eq_ignore_ascii_case("D"),
        KeyCode::KeyE => b.eq_ignore_ascii_case("E"),
        KeyCode::KeyF => b.eq_ignore_ascii_case("F"),
        KeyCode::KeyG => b.eq_ignore_ascii_case("G"),
        KeyCode::KeyH => b.eq_ignore_ascii_case("H"),
        KeyCode::KeyI => b.eq_ignore_ascii_case("I"),
        KeyCode::KeyJ => b.eq_ignore_ascii_case("J"),
        KeyCode::KeyK => b.eq_ignore_ascii_case("K"),
        KeyCode::KeyL => b.eq_ignore_ascii_case("L"),
        KeyCode::KeyM => b.eq_ignore_ascii_case("M"),
        KeyCode::KeyN => b.eq_ignore_ascii_case("N"),
        KeyCode::KeyO => b.eq_ignore_ascii_case("O"),
        KeyCode::KeyP => b.eq_ignore_ascii_case("P"),
        KeyCode::KeyQ => b.eq_ignore_ascii_case("Q"),
        KeyCode::KeyR => b.eq_ignore_ascii_case("R"),
        KeyCode::KeyS => b.eq_ignore_ascii_case("S"),
        KeyCode::KeyT => b.eq_ignore_ascii_case("T"),
        KeyCode::KeyU => b.eq_ignore_ascii_case("U"),
        KeyCode::KeyV => b.eq_ignore_ascii_case("V"),
        KeyCode::KeyW => b.eq_ignore_ascii_case("W"),
        KeyCode::KeyX => b.eq_ignore_ascii_case("X"),
        KeyCode::KeyY => b.eq_ignore_ascii_case("Y"),
        KeyCode::KeyZ => b.eq_ignore_ascii_case("Z"),
        KeyCode::Digit0 => b == "0",
        KeyCode::Digit1 => b == "1",
        KeyCode::Digit2 => b == "2",
        KeyCode::Digit3 => b == "3",
        KeyCode::Digit4 => b == "4",
        KeyCode::Digit5 => b == "5",
        KeyCode::Digit6 => b == "6",
        KeyCode::Digit7 => b == "7",
        KeyCode::Digit8 => b == "8",
        KeyCode::Digit9 => b == "9",
        _ => false,
    }
}

fn compute_gba_keypad(pressed: &HashSet<KeyCode>, kb: &KeyBindings) -> u16 {
    let mut mask = 0x03FFu16;
    let is_down = |binding: &str| pressed.iter().any(|&c| binding_matches_keycode(binding, c));
    if is_down(&kb.a) {
        mask &= !(1 << 0);
    }
    if is_down(&kb.b) {
        mask &= !(1 << 1);
    }
    if is_down(&kb.select) {
        mask &= !(1 << 2);
    }
    if is_down(&kb.start) {
        mask &= !(1 << 3);
    }
    if is_down(&kb.right) || pressed.contains(&KeyCode::ArrowRight) {
        mask &= !(1 << 4);
    }
    if is_down(&kb.left) || pressed.contains(&KeyCode::ArrowLeft) {
        mask &= !(1 << 5);
    }
    if is_down(&kb.up) || pressed.contains(&KeyCode::ArrowUp) {
        mask &= !(1 << 6);
    }
    if is_down(&kb.down) || pressed.contains(&KeyCode::ArrowDown) {
        mask &= !(1 << 7);
    }
    if is_down(&kb.r) {
        mask &= !(1 << 8);
    }
    if is_down(&kb.l) {
        mask &= !(1 << 9);
    }
    mask
}

struct NoGpuApp {
    title: String,
    window: Option<Arc<Window>>,
    context: Option<softbuffer::Context<Arc<Window>>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    size: (u32, u32),
    config: AppConfig,
    core: Option<GbaCore>,
    bios_bytes: Option<Vec<u8>>,
    sav_path: Option<PathBuf>,
    rom_name: String,
    sink: NoGpuSink,
    postproc: VideoPostProcessor,
    blitter: NoGpuCpuPresenter,
    governor: QualityGovernor,
    profile_key: String,
    pressed_keys: HashSet<KeyCode>,
    paused: bool,
    next_frame_due: Instant,
    last_title_update: Instant,
    frames_in_window: u32,
    last_core_ms: f64,
    last_present_ms: f64,
    pending_emu_ms: f64,
    frames_stepped_since_present: u32,
    fullscreen_on_start: bool,
    is_fullscreen: bool,
    sync_60hz: bool,
    slowdown: usize,
    drc_pacer: crate::emu_thread::DrcPacer,
    #[cfg(windows)]
    #[allow(dead_code)]
    high_res_timer: Option<crate::emu_thread::win_timer::HighResSleepTimer>,
    replay: Option<crate::bench::InputReplay>,
    loop_replay: bool,
    replay_frame: usize,
    perf_logger: Option<crate::bench::DesktopPerfLogger>,
    perf_seconds: Option<f64>,
    perf_start_wall: Instant,
    startup_err: Option<String>,
}

impl NoGpuApp {
    fn new(
        title: String,
        init_w: u32,
        init_h: u32,
        cli: CliConfig,
        reason: &str,
    ) -> Self {
        let mut config = AppConfig::load();
        if let Some(f) = cli.filter {
            config.display_filter = f;
        }
        if let Some(gov_flag) = cli.governor {
            config.auto_quality_governor = gov_flag;
        }
        let bios_bytes = cli
            .bios_path
            .as_deref()
            .and_then(|p| fs::read(p).ok())
            .or_else(|| config.bios_path.as_deref().and_then(|p| fs::read(p).ok()));

        let audio = AudioOutput::start();
        audio.set_muted(config.muted);
        let audio_cap = audio.capacity();
        let sync_decision =
            crate::display_timing::resolve_sync_decision(cli.sync_60hz, cli.simulate_refresh_hz);
        println!("[sync] {}", sync_decision.reason);
        let sync_60hz = sync_decision.sync_60hz;
        let display_refresh_hz = sync_decision.effective_refresh_hz;
        let mut drc_pacer = crate::emu_thread::DrcPacer::new(audio_cap);
        if sync_60hz {
            drc_pacer.set_base_fps(60.0);
        }
        #[cfg(windows)]
        let high_res_timer = if sync_60hz {
            Some(crate::emu_thread::win_timer::HighResSleepTimer::new())
        } else {
            None
        };
        let replay = cli
            .input_replay
            .as_deref()
            .and_then(|p| crate::bench::InputReplay::load(p).ok());
        let perf_logger = cli.perf_log.as_deref().and_then(|p| {
            let tag = if sync_60hz {
                "no-gpu-softbuffer-sync60"
            } else {
                "no-gpu-softbuffer"
            };
            crate::bench::DesktopPerfLogger::create_with_timing(
                p,
                tag,
                display_refresh_hz,
                sync_60hz,
            )
            .ok()
        });
        let mut sink = NoGpuSink::new(audio);
        let mut postproc = VideoPostProcessor::new();
        let mut blitter = NoGpuCpuPresenter::new(init_w as usize, init_h as usize);

        let costs = blitter.measure_filter_costs(
            &mut postproc,
            config.color_correction,
            config.lcd_ghosting,
            config.lcd_grid_intensity,
            init_w as usize,
            init_h as usize,
        );
        let capped_costs = blitter.measure_capped_filter_costs(
            &mut postproc,
            config.color_correction,
            config.lcd_ghosting,
            config.lcd_grid_intensity,
            init_w as usize,
            init_h as usize,
            None,
        );
        let mut governor = QualityGovernor::with_measured_costs(
            config.display_filter,
            config.auto_quality_governor,
            costs,
        );
        governor.set_resolution_cap_support(
            init_w as usize > GOVERNOR_MAX_CAP_WIDTH || init_h as usize > GOVERNOR_MAX_CAP_HEIGHT,
            Some(capped_costs),
        );
        let profile_key = AppConfig::governor_profile_key(
            "winit+softbuffer",
            "NoGpu",
            init_w,
            init_h,
            config.window_scale,
        );
        if config.auto_quality_governor
            && cli.filter.is_none()
            && let Some(settled) = config.settled_filter_for(&profile_key)
        {
            governor.set_active_from_persisted(settled);
            config.display_filter = governor.active_filter();
            log::info!(
                "Restored settled governor filter `{}` for profile `{profile_key}`",
                config.display_filter.label()
            );
        }

        let mut core = None;
        let mut sav_path = cli.sav_path.clone();
        let mut rom_name = String::from("No ROM loaded (press Ctrl+O or drop .gba)");
        if let Some(path) = &cli.rom_path {
            match fs::read(path) {
                Ok(rom) => {
                    let mut c = GbaCore::new(bios_bytes.as_deref(), rom);
                    let inferred_sav = sav_path
                        .take()
                        .unwrap_or_else(|| path.with_extension("sav"));
                    if inferred_sav.exists()
                        && let Ok(bytes) = fs::read(&inferred_sav)
                    {
                        c.bus.cart.load_save_data(&bytes);
                    }
                    sav_path = Some(inferred_sav);
                    rom_name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "ROM".to_string());
                    core = Some(c);
                }
                Err(e) => {
                    log::error!("Failed to load CLI ROM in no-GPU mode: {e}");
                }
            }
        } else {
            // Draw deterministic test card until user opens or drops a ROM.
            sink.framebuffer
                .copy_from_slice(&*crate::filters::test_filter_pattern_fb());
            sink.frame_dirty = true;
        }

        postproc.prewarm(
            config.color_correction,
            config.lcd_ghosting,
            config.display_filter,
            false,
            config.lcd_grid_intensity,
        );

        log::info!(
            "Selected backend: No-GPU presenter (winit + softbuffer) — reason: {reason}; fallback order: {}",
            crate::shaders::ADAPTER_FALLBACK_ORDER.join(" -> ")
        );

        Self {
            title,
            window: None,
            context: None,
            surface: None,
            size: (init_w.max(1), init_h.max(1)),
            config,
            core,
            bios_bytes,
            sav_path,
            rom_name,
            sink,
            postproc,
            blitter,
            governor,
            profile_key,
            pressed_keys: HashSet::new(),
            paused: false,
            next_frame_due: Instant::now(),
            last_title_update: Instant::now(),
            frames_in_window: 0,
            last_core_ms: 0.0,
            last_present_ms: 0.0,
            pending_emu_ms: 0.0,
            frames_stepped_since_present: 0,
            fullscreen_on_start: cli.fullscreen,
            is_fullscreen: cli.fullscreen,
            sync_60hz,
            slowdown: cli.slowdown.max(1),
            drc_pacer,
            #[cfg(windows)]
            high_res_timer,
            replay,
            loop_replay: cli.loop_replay,
            replay_frame: 0,
            perf_logger,
            perf_seconds: cli.perf_seconds,
            perf_start_wall: Instant::now(),
            startup_err: None,
        }
    }

    fn load_rom(&mut self, path: &Path) {
        self.flush_save();
        let Ok(rom) = fs::read(path) else {
            return;
        };
        let mut core = GbaCore::new(self.bios_bytes.as_deref(), rom);
        let sav = path.with_extension("sav");
        if sav.exists()
            && let Ok(bytes) = fs::read(&sav)
        {
            core.bus.cart.load_save_data(&bytes);
        }
        self.core = Some(core);
        self.sav_path = Some(sav);
        self.rom_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ROM".to_string());
        self.paused = false;
        self.next_frame_due = Instant::now();
        self.sink.frame_dirty = true;
    }

    fn flush_save(&mut self) {
        if let (Some(core), Some(sav)) = (&mut self.core, &self.sav_path)
            && core.bus.cart.save_dirty
        {
            let data = core.bus.cart.export_save_data();
            if fs::write(sav, &data).is_ok() {
                core.bus.cart.save_dirty = false;
            }
        }
    }

    fn present_current_frame(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let (dst_w, dst_h) = (self.size.0 as usize, self.size.1 as usize);
        if dst_w == 0 || dst_h == 0 {
            return;
        }
        let t0 = Instant::now();
        let green_swap = self
            .core
            .as_ref()
            .is_some_and(|c| (c.bus.ppu.greenswap & 1) != 0);
        let img = self.postproc.process_display_filter(
            &self.sink.framebuffer,
            self.config.color_correction,
            self.config.lcd_ghosting,
            self.config.display_filter,
            green_swap,
            self.config.lcd_grid_intensity,
        );
        let linear = self.config.display_filter.texture_options() == egui::TextureOptions::LINEAR;
        let used_os_stretch = if self.governor.is_resolution_capped() {
            self.blitter.present_capped_os_stretch(
                window,
                &img.pixels,
                img.size[0],
                img.size[1],
                dst_w,
                dst_h,
                false,
            )
        } else {
            false
        };
        if !used_os_stretch {
            let Some(surface) = self.surface.as_mut() else {
                return;
            };
            let Ok(mut buf) = surface.buffer_mut() else {
                return;
            };
            self.blitter.blit_rgba_to_slice(
                &img.pixels,
                img.size[0],
                img.size[1],
                &mut buf,
                dst_w,
                dst_h,
                linear,
            );
            let _ = buf.present();
        }
        self.last_present_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.sink.frame_dirty = false;
    }
}

impl ApplicationHandler for NoGpuApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let mut attrs = Window::default_attributes()
            .with_title(format!("{} [No-GPU: softbuffer] — {}", self.title, self.rom_name))
            .with_inner_size(LogicalSize::new(self.size.0, self.size.1));
        if self.fullscreen_on_start {
            attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.startup_err = Some(format!("winit create_window failed: {e}"));
                event_loop.exit();
                return;
            }
        };
        let context = match softbuffer::Context::new(Arc::clone(&window)) {
            Ok(c) => c,
            Err(e) => {
                self.startup_err = Some(format!("softbuffer Context::new failed: {e}"));
                event_loop.exit();
                return;
            }
        };
        let mut surface = match softbuffer::Surface::new(&context, Arc::clone(&window)) {
            Ok(s) => s,
            Err(e) => {
                self.startup_err = Some(format!("softbuffer Surface::new failed: {e}"));
                event_loop.exit();
                return;
            }
        };
        let phys = window.inner_size();
        self.size = (phys.width.max(1), phys.height.max(1));
        if let (Some(w), Some(h)) = (NonZeroU32::new(self.size.0), NonZeroU32::new(self.size.1)) {
            let _ = surface.resize(w, h);
        }
        self.profile_key = AppConfig::governor_profile_key(
            "winit+softbuffer",
            "NoGpu",
            self.size.0,
            self.size.1,
            self.config.window_scale,
        );
        let capped_costs = self.blitter.measure_capped_filter_costs(
            &mut self.postproc,
            self.config.color_correction,
            self.config.lcd_ghosting,
            self.config.lcd_grid_intensity,
            self.size.0 as usize,
            self.size.1 as usize,
            Some(&window),
        );
        let costs = self.blitter.measure_filter_costs(
            &mut self.postproc,
            self.config.color_correction,
            self.config.lcd_ghosting,
            self.config.lcd_grid_intensity,
            self.size.0 as usize,
            self.size.1 as usize,
        );
        self.governor.set_measured_costs(costs);
        self.governor.set_resolution_cap_support(
            self.size.0 as usize > GOVERNOR_MAX_CAP_WIDTH
                || self.size.1 as usize > GOVERNOR_MAX_CAP_HEIGHT,
            Some(capped_costs),
        );
        self.window = Some(window);
        self.context = Some(context);
        self.surface = Some(surface);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.flush_save();
                if let Some(ref mut logger) = self.perf_logger {
                    logger.finish_and_print();
                }
                self.config
                    .set_settled_filter(self.profile_key.clone(), self.governor.active_filter());
                self.config.save();
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                self.size = (new_size.width.max(1), new_size.height.max(1));
                if let (Some(surface), Some(w), Some(h)) = (
                    self.surface.as_mut(),
                    NonZeroU32::new(self.size.0),
                    NonZeroU32::new(self.size.1),
                ) {
                    let _ = surface.resize(w, h);
                }
                self.profile_key = AppConfig::governor_profile_key(
                    "winit+softbuffer",
                    "NoGpu",
                    self.size.0,
                    self.size.1,
                    self.config.window_scale,
                );
                let costs = self.blitter.measure_filter_costs(
                    &mut self.postproc,
                    self.config.color_correction,
                    self.config.lcd_ghosting,
                    self.config.lcd_grid_intensity,
                    self.size.0 as usize,
                    self.size.1 as usize,
                );
                let capped_costs = self.blitter.measure_capped_filter_costs(
                    &mut self.postproc,
                    self.config.color_correction,
                    self.config.lcd_ghosting,
                    self.config.lcd_grid_intensity,
                    self.size.0 as usize,
                    self.size.1 as usize,
                    self.window.as_deref(),
                );
                self.governor.set_measured_costs(costs);
                self.governor.set_resolution_cap_support(
                    self.size.0 as usize > GOVERNOR_MAX_CAP_WIDTH
                        || self.size.1 as usize > GOVERNOR_MAX_CAP_HEIGHT,
                    Some(capped_costs),
                );
                self.sink.frame_dirty = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.load_rom(&path);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        self.pressed_keys.insert(code);
                        if !event.repeat {
                            match code {
                                KeyCode::KeyP => self.paused = !self.paused,
                                KeyCode::KeyF => {
                                    let next = self.config.display_filter.next();
                                    self.config.display_filter = next;
                                    self.config.shader_preset = next.default_shader_preset();
                                    self.governor.set_user_target(next);
                                    self.config.save();
                                    self.sink.frame_dirty = true;
                                }
                                KeyCode::KeyO
                                    if self.pressed_keys.contains(&KeyCode::ControlLeft)
                                        || self.pressed_keys.contains(&KeyCode::ControlRight) =>
                                {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .set_title("Open GBA ROM")
                                        .add_filter("GBA ROMs", &["gba", "agb", "bin"])
                                        .pick_file()
                                    {
                                        self.load_rom(&path);
                                    }
                                }
                                KeyCode::Escape => {
                                    self.flush_save();
                                    if let Some(ref mut logger) = self.perf_logger {
                                        logger.finish_and_print();
                                    }
                                    self.config.set_settled_filter(
                                        self.profile_key.clone(),
                                        self.governor.active_filter(),
                                    );
                                    self.config.save();
                                    event_loop.exit();
                                }
                                _ => {}
                            }
                        }
                    } else {
                        self.pressed_keys.remove(&code);
                    }
                    self.sink.keypad = compute_gba_keypad(&self.pressed_keys, &self.config.keybindings);
                }
            }
            WindowEvent::RedrawRequested => {
                self.present_current_frame();
                if let Some(ref mut logger) = self.perf_logger
                    && self.core.is_some()
                {
                    logger.record_present(
                        self.pending_emu_ms,
                        self.last_present_ms,
                        self.sink.audio.underruns(),
                        self.config.display_filter.label(),
                        self.frames_stepped_since_present,
                        self.size.0,
                        self.size.1,
                        self.is_fullscreen,
                    );
                }
                self.pending_emu_ms = 0.0;
                self.frames_stepped_since_present = 0;
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(limit_secs) = self.perf_seconds
            && self.perf_start_wall.elapsed().as_secs_f64() >= limit_secs
        {
            self.flush_save();
            if let Some(ref mut logger) = self.perf_logger {
                logger.finish_and_print();
            }
            event_loop.exit();
            return;
        }

        let mut now = Instant::now();
        if self.sync_60hz && !self.paused && now < self.next_frame_due {
            let remaining = self.next_frame_due.saturating_duration_since(now);
            if remaining <= Duration::from_micros(2200) {
                while Instant::now() < self.next_frame_due {
                    std::hint::spin_loop();
                }
                now = Instant::now();
            }
        }
        if !self.paused && now >= self.next_frame_due {
            if let Some(ref replay) = self.replay {
                let eff = replay.effective_frame(self.replay_frame, self.loop_replay);
                self.sink.keypad = replay.keyinput_at(eff);
                self.replay_frame += 1;
            }
            if let Some(core) = &mut self.core {
                let t0 = Instant::now();
                core.run_frame(&mut self.sink);
                self.last_core_ms = t0.elapsed().as_secs_f64() * 1000.0;
                if self.slowdown > 1 {
                    let extra_ms =
                        (self.last_core_ms + self.last_present_ms) * (self.slowdown - 1) as f64;
                    let spin_until = Instant::now() + Duration::from_secs_f64(extra_ms / 1000.0);
                    while Instant::now() < spin_until {
                        std::hint::spin_loop();
                    }
                }
                let scaled_core_ms = self.last_core_ms * self.slowdown as f64;
                let scaled_present_ms = self.last_present_ms * self.slowdown as f64;
                self.pending_emu_ms += scaled_core_ms;
                self.frames_stepped_since_present += 1;
                self.frames_in_window += 1;

                let work_ms = scaled_core_ms + scaled_present_ms;
                if let Some(tr) = self.governor.record_frame(work_ms) {
                    self.config.display_filter = tr.to;
                    self.config.shader_preset = tr.to.default_shader_preset();
                    self.config
                        .set_settled_filter(self.profile_key.clone(), tr.to);
                    self.config.save();
                    self.sink.frame_dirty = true;
                    println!(
                        "[governor] No-GPU {:?}: {} -> {} | capped_720x480={} (rolling_p95={:.2} ms)",
                        tr.kind,
                        tr.from.label(),
                        tr.to.label(),
                        self.governor.is_resolution_capped(),
                        tr.rolling_p95_ms
                    );
                }
            }
            let frame_period = if self.sync_60hz {
                let occ = self.sink.audio.occupied_len();
                self.drc_pacer.set_target(self.sink.audio.target_fill());
                self.drc_pacer
                    .next_period_duration(occ, self.sink.audio.has_device())
            } else {
                GBA_FRAME_DURATION
            };
            self.next_frame_due += frame_period;
            if self.next_frame_due + frame_period < now {
                self.next_frame_due = now + frame_period;
            }
            if self.sink.frame_dirty
                && let Some(w) = &self.window
            {
                w.request_redraw();
            }
        }

        if self.last_title_update.elapsed() >= Duration::from_millis(500) {
            let secs = self.last_title_update.elapsed().as_secs_f64().max(0.001);
            let fps = self.frames_in_window as f64 / secs;
            self.frames_in_window = 0;
            self.last_title_update = now;
            if let Some(w) = &self.window {
                let mode_tag = if self.governor.is_resolution_capped() {
                    "StretchDIBits <=720x480"
                } else {
                    "softbuffer"
                };
                w.set_title(&format!(
                    "{} [No-GPU: {} | {} | {:.1} FPS] — {}",
                    self.title,
                    mode_tag,
                    self.config.display_filter.label(),
                    fps,
                    self.rom_name
                ));
            }
        }

        let wait_until = if self.paused {
            now + Duration::from_millis(16)
        } else if self.sync_60hz {
            self.next_frame_due
                .checked_sub(Duration::from_millis(1))
                .unwrap_or(self.next_frame_due)
        } else {
            self.next_frame_due
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(wait_until));
    }
}

/// Runs the desktop emulator purely via `winit + softbuffer` (zero `wgpu`/Vulkan/DX12/GL usage).
pub fn run_no_gpu_desktop(
    title: &str,
    init_width: f32,
    init_height: f32,
    cli: CliConfig,
    reason: &str,
) -> Result<(), String> {
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(true);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("failed to build winit event loop for No-GPU presenter: {e}"))?;

    let mut app = NoGpuApp::new(
        title.to_string(),
        init_width.round().max(240.0) as u32,
        init_height.round().max(160.0) as u32,
        cli,
        reason,
    );
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("winit No-GPU event loop failed: {e}"))?;
    if let Some(err) = app.startup_err {
        return Err(err);
    }
    Ok(())
}

/// Benchmarks real windowed `winit + softbuffer::Surface` present latency (GDI/X11/Wayland)
/// for `none` (`Sharp1x`) and `myboy-smooth` (`MyBoySmooth`) on the desktop OS presenter at `720x480`.
pub fn bench_real_softbuffer_present(frames: usize) -> Result<Vec<(String, f64, f64)>, String> {
    let rows = bench_softbuffer_sizes(frames, false)?;
    Ok(rows
        .into_iter()
        .filter(|r| r.slowdown_x == 1 && r.width == (LCD_WIDTH * 3) as u32)
        .map(|r| (r.filter, r.present_only_avg_ms, r.present_only_p95_ms))
        .collect())
}

/// Benchmarks real `winit + softbuffer::Surface` presentation + full GBA frame work across
/// `720x480` (baseline), `1280x720` (720p), `1920x1080` (1080p), and the monitor's native
/// fullscreen resolution with both `none` and `myboy-smooth` filters at `1x`, `8x`, and `15x`.
pub fn bench_softbuffer_sizes(
    frames: usize,
    include_scaled_slowdowns: bool,
) -> Result<Vec<SoftbufferSizeBenchRow>, String> {
    struct BenchRunner {
        frames: usize,
        include_scaled_slowdowns: bool,
        surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
        _ctx: Option<softbuffer::Context<Arc<Window>>>,
        _win: Option<Arc<Window>>,
        results: Vec<SoftbufferSizeBenchRow>,
        err: Option<String>,
    }

    impl ApplicationHandler for BenchRunner {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            if self._win.is_some() {
                return;
            }
            let init_w = (LCD_WIDTH * 3) as u32;
            let init_h = (LCD_HEIGHT * 3) as u32;
            let attrs = Window::default_attributes()
                .with_title("Rustboy Advance No-GPU Bench")
                .with_inner_size(LogicalSize::new(init_w, init_h))
                .with_visible(false);
            let win = match event_loop.create_window(attrs) {
                Ok(win) => Arc::new(win),
                Err(e) => {
                    self.err = Some(format!("softbuffer bench window failed: {e}"));
                    event_loop.exit();
                    return;
                }
            };
            let mon_size = win
                .current_monitor()
                .or_else(|| event_loop.primary_monitor())
                .map(|m| m.size())
                .unwrap_or(winit::dpi::PhysicalSize::new(1920, 1080));
            let ctx = match softbuffer::Context::new(Arc::clone(&win)) {
                Ok(c) => c,
                Err(e) => {
                    self.err = Some(format!("softbuffer bench context failed: {e}"));
                    event_loop.exit();
                    return;
                }
            };
            let mut surf = match softbuffer::Surface::new(&ctx, Arc::clone(&win)) {
                Ok(s) => s,
                Err(e) => {
                    self.err = Some(format!("softbuffer bench surface failed: {e}"));
                    event_loop.exit();
                    return;
                }
            };

            let rom = crate::stress_rom::build_stress_mode0_4bg_128spr_rom();
            let mut core = Box::new(GbaCore::new(None, rom));
            struct NullSink([u16; LCD_WIDTH * LCD_HEIGHT]);
            impl HostFrontend for NullSink {
                fn present_frame(&mut self, fb: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
                    self.0.copy_from_slice(fb);
                }
                fn push_audio_samples(&mut self, _samples: &[i16]) {}
                fn poll_keypad(&mut self) -> u16 {
                    0x03FF
                }
            }
            let mut core_sink = NullSink([0u16; LCD_WIDTH * LCD_HEIGHT]);
            for _ in 0..30 {
                core.run_frame(&mut core_sink);
            }

            let mut sizes = vec![
                ("720x480 (3x GBA baseline)".to_string(), 720u32, 480u32),
            ];
            if self.include_scaled_slowdowns {
                sizes.push(("1280x720 (720p)".to_string(), 1280, 720));
                sizes.push(("1920x1080 (1080p)".to_string(), 1920, 1080));
                sizes.push((
                    format!("{}x{} (fullscreen)", mon_size.width, mon_size.height),
                    mon_size.width.max(240),
                    mon_size.height.max(160),
                ));
            }
            let slowdowns: &[usize] = if self.include_scaled_slowdowns {
                &[1, 8, 15]
            } else {
                &[1]
            };

            let mut postproc = VideoPostProcessor::new();
            let mut blitter = NoGpuCpuPresenter::new(init_w as usize, init_h as usize);

            #[derive(Clone, Copy)]
            enum SizePresentVariant {
                SoftbufferUncapped,
                #[cfg(target_os = "windows")]
                StretchDibitsColorOnColor,
                #[cfg(target_os = "windows")]
                StretchDibitsHalftone,
            }

            impl SizePresentVariant {
                fn label(self) -> &'static str {
                    match self {
                        Self::SoftbufferUncapped => "softbuffer",
                        #[cfg(target_os = "windows")]
                        Self::StretchDibitsColorOnColor => "stretchdibits-coloroncolor",
                        #[cfg(target_os = "windows")]
                        Self::StretchDibitsHalftone => "stretchdibits-halftone",
                    }
                }
            }

            let run_variant = |variant: SizePresentVariant,
                               res_label: &str,
                               w: u32,
                               h: u32,
                               surf: &mut softbuffer::Surface<Arc<Window>, Arc<Window>>,
                               core: &mut GbaCore,
                               core_sink: &mut NullSink,
                               postproc: &mut VideoPostProcessor,
                               blitter: &mut NoGpuCpuPresenter,
                               results: &mut Vec<SoftbufferSizeBenchRow>| {
                for &(slug, filter, cc) in &[
                    ("none", DisplayFilter::Sharp1x, false),
                    ("myboy-smooth", DisplayFilter::MyBoySmooth, false),
                ] {
                    postproc.prewarm(cc, false, filter, false, 0.08);
                    let linear = filter.texture_options() == egui::TextureOptions::LINEAR;
                    let mut present_once = |core_fb: &[u16; LCD_WIDTH * LCD_HEIGHT]| {
                        let img = postproc.process_display_filter(
                            core_fb, cc, false, filter, false, 0.08,
                        );
                        match variant {
                            SizePresentVariant::SoftbufferUncapped => {
                                if let Ok(mut buf) = surf.buffer_mut() {
                                    blitter.blit_rgba_to_slice(
                                        &img.pixels,
                                        img.size[0],
                                        img.size[1],
                                        &mut buf,
                                        w as usize,
                                        h as usize,
                                        linear,
                                    );
                                    let _ = buf.present();
                                }
                            }
                            #[cfg(target_os = "windows")]
                            SizePresentVariant::StretchDibitsColorOnColor => {
                                let _ = blitter.present_capped_os_stretch(
                                    &win,
                                    &img.pixels,
                                    img.size[0],
                                    img.size[1],
                                    w as usize,
                                    h as usize,
                                    false,
                                );
                            }
                            #[cfg(target_os = "windows")]
                            SizePresentVariant::StretchDibitsHalftone => {
                                let _ = blitter.present_capped_os_stretch(
                                    &win,
                                    &img.pixels,
                                    img.size[0],
                                    img.size[1],
                                    w as usize,
                                    h as usize,
                                    true,
                                );
                            }
                        }
                    };
                    for _ in 0..8 {
                        core.run_frame(core_sink);
                        present_once(&core_sink.0);
                    }

                    for &slowdown in slowdowns {
                        let mut present_samples = Vec::with_capacity(self.frames);
                        let mut total_samples = Vec::with_capacity(self.frames);
                        let mut over_budget = 0usize;
                        for _ in 0..self.frames {
                            let t_frame = Instant::now();
                            core.run_frame(core_sink);
                            let t_pres = Instant::now();
                            present_once(&core_sink.0);
                            let pres_ms = t_pres.elapsed().as_secs_f64() * 1000.0;
                            let raw_dur = t_frame.elapsed();
                            if slowdown > 1 {
                                let target = raw_dur.mul_f64(slowdown as f64);
                                while t_frame.elapsed() < target {
                                    std::hint::spin_loop();
                                }
                            }
                            let total_ms = t_frame.elapsed().as_secs_f64() * 1000.0;
                            if total_ms > 1000.0 / (16_777_216.0 / 280_896.0) {
                                over_budget += 1;
                            }
                            present_samples.push(pres_ms * slowdown as f64);
                            total_samples.push(total_ms);
                        }
                        present_samples.sort_by(f64::total_cmp);
                        total_samples.sort_by(f64::total_cmp);
                        let n = self.frames.max(1);
                        let pres_avg = present_samples.iter().sum::<f64>() / n as f64;
                        let pres_p95 = present_samples[((n - 1) * 95) / 100];
                        let tot_avg = total_samples.iter().sum::<f64>() / n as f64;
                        let tot_p95 = total_samples[((n - 1) * 95) / 100];
                        let tot_p99 = total_samples[((n - 1) * 99) / 100];
                        let tot_worst = *total_samples.last().unwrap_or(&0.0);
                        let over_pct = (over_budget as f64 / n as f64) * 100.0;
                        results.push(SoftbufferSizeBenchRow {
                            present_mode: variant.label().to_string(),
                            resolution_label: res_label.to_string(),
                            width: w,
                            height: h,
                            filter: slug.to_string(),
                            slowdown_x: slowdown,
                            frames: self.frames,
                            present_only_avg_ms: pres_avg,
                            present_only_p95_ms: pres_p95,
                            total_work_avg_ms: tot_avg,
                            total_work_p95_ms: tot_p95,
                            total_work_p99_ms: tot_p99,
                            total_work_worst_ms: tot_worst,
                            over_budget_pct: over_pct,
                            fits_budget: over_pct < 0.50,
                        });
                    }
                }
            };

            for (res_label, w, h) in sizes {
                let _ = surf.resize(
                    NonZeroU32::new(w.max(1)).unwrap(),
                    NonZeroU32::new(h.max(1)).unwrap(),
                );
                run_variant(
                    SizePresentVariant::SoftbufferUncapped,
                    &res_label,
                    w,
                    h,
                    &mut surf,
                    &mut core,
                    &mut core_sink,
                    &mut postproc,
                    &mut blitter,
                    &mut self.results,
                );
                #[cfg(target_os = "windows")]
                if self.include_scaled_slowdowns
                    && (w as usize > GOVERNOR_MAX_CAP_WIDTH || h as usize > GOVERNOR_MAX_CAP_HEIGHT)
                {
                    let _ = win.request_inner_size(LogicalSize::new(w, h));
                    run_variant(
                        SizePresentVariant::StretchDibitsColorOnColor,
                        &res_label,
                        w,
                        h,
                        &mut surf,
                        &mut core,
                        &mut core_sink,
                        &mut postproc,
                        &mut blitter,
                        &mut self.results,
                    );
                    run_variant(
                        SizePresentVariant::StretchDibitsHalftone,
                        &res_label,
                        w,
                        h,
                        &mut surf,
                        &mut core,
                        &mut core_sink,
                        &mut postproc,
                        &mut blitter,
                        &mut self.results,
                    );
                }
            }
            self.surface = Some(surf);
            self._ctx = Some(ctx);
            self._win = Some(win);
            event_loop.exit();
        }

        fn window_event(
            &mut self,
            _event_loop: &ActiveEventLoop,
            _window_id: WindowId,
            _event: WindowEvent,
        ) {
        }
    }

    let mut builder = EventLoop::builder();
    #[cfg(target_os = "windows")]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_any_thread(true);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("softbuffer bench EventLoop build failed: {e}"))?;
    let mut runner = BenchRunner {
        frames: frames.max(30),
        include_scaled_slowdowns,
        surface: None,
        _ctx: None,
        _win: None,
        results: Vec::new(),
        err: None,
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|e| format!("softbuffer bench run_app failed: {e}"))?;
    if let Some(e) = runner.err {
        return Err(e);
    }
    Ok(runner.results)
}
