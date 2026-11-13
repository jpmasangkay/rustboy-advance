use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use egui::ColorImage;
use gba_core::{HostFrontend, LCD_HEIGHT, LCD_WIDTH};

use crate::config::{AppConfig, ColorProfileChoice, DisplayFilter};

static LINEAR_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGB_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS101_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS001_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();

static SRGB_LUT_3: OnceLock<Box<[[u8; 3]; 32_768]>> = OnceLock::new();
static RAW_LUT_3: OnceLock<Box<[[u8; 3]; 32_768]>> = OnceLock::new();

#[inline]
pub fn raw_lut_3() -> &'static [[u8; 3]; 32_768] {
    RAW_LUT_3.get_or_init(|| {
        let mut table = vec![[0u8; 3]; 32_768].into_boxed_slice();
        for bgr in 0..32_768usize {
            let r5 = (bgr & 0x1F) as f32;
            let g5 = ((bgr >> 5) & 0x1F) as f32;
            let b5 = ((bgr >> 10) & 0x1F) as f32;

            table[bgr] = [
                ((r5 as u8) << 3) | ((r5 as u8) >> 2),
                ((g5 as u8) << 3) | ((g5 as u8) >> 2),
                ((b5 as u8) << 3) | ((b5 as u8) >> 2),
            ];
        }
        table.try_into().expect("32768-entry raw LUT")
    })
}

#[inline]
pub fn srgb_lut_3() -> &'static [[u8; 3]; 32_768] {
    SRGB_LUT_3.get_or_init(|| {
        let mut table = vec![[0u8; 3]; 32_768].into_boxed_slice();
        for bgr in 0..32_768usize {
            let r5 = (bgr & 0x1F) as f32;
            let g5 = ((bgr >> 5) & 0x1F) as f32;
            let b5 = ((bgr >> 10) & 0x1F) as f32;

            let rl = (r5 / 31.0).powf(3.6);
            let gl = (g5 / 31.0).powf(3.6);
            let bl = (b5 / 31.0).powf(3.6);

            let sr = (0.845 * rl + 0.195 * gl - 0.040 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);
            let sg = (0.085 * rl + 0.690 * gl + 0.225 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);
            let sb = (0.050 * rl + 0.210 * gl + 0.740 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);

            table[bgr] = [(sr * 255.0) as u8, (sg * 255.0) as u8, (sb * 255.0) as u8];
        }
        table.try_into().expect("32768-entry sRGB LUT")
    })
}

#[inline]
pub fn linear_lut() -> &'static [[u8; 4]; 32_768] {
    LINEAR_LUT.get_or_init(|| {
        let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
        for c in 0..32_768usize {
            let r5 = (c & 0x1F) as u8;
            let g5 = ((c >> 5) & 0x1F) as u8;
            let b5 = ((c >> 10) & 0x1F) as u8;
            table[c] = [
                (r5 << 3) | (r5 >> 2),
                (g5 << 3) | (g5 >> 2),
                (b5 << 3) | (b5 >> 2),
                255,
            ];
        }
        table.try_into().expect("32768-entry LUT")
    })
}

#[inline]
pub fn agb_srgb_lut() -> &'static [[u8; 4]; 32_768] {
    AGB_LUT.get_or_init(|| {
        // AGB-001 unbacklit reflective TFT -> modern sRGB colorimetric matrix (Tonc/byuu).
        let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
        for c in 0..32_768usize {
            let r = ((c & 0x1F) as f32) / 31.0;
            let g = (((c >> 5) & 0x1F) as f32) / 31.0;
            let b = (((c >> 10) & 0x1F) as f32) / 31.0;

            let lr = r.powf(2.4);
            let lg = g.powf(2.4);
            let lb = b.powf(2.4);

            let out_r =
                (255.0 * (lr * 0.845 + lg * 0.125 + lb * 0.030).powf(1.0 / 2.2)).clamp(0.0, 255.0);
            let out_g =
                (255.0 * (lr * 0.055 + lg * 0.675 + lb * 0.270).powf(1.0 / 2.2)).clamp(0.0, 255.0);
            let out_b =
                (255.0 * (lr * 0.000 + lg * 0.195 + lb * 0.805).powf(1.0 / 2.2)).clamp(0.0, 255.0);

            table[c] = [out_r as u8, out_g as u8, out_b as u8, 255];
        }
        table.try_into().expect("32768-entry LUT")
    })
}

#[inline]
pub fn ags_101_lut() -> &'static [[u8; 4]; 32_768] {
    AGS101_LUT.get_or_init(|| {
        // AGS-101 backlit vibrant TFT curve (deeper contrast, vivid primary colors)
        let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
        for c in 0..32_768usize {
            let r = ((c & 0x1F) as f32) / 31.0;
            let g = (((c >> 5) & 0x1F) as f32) / 31.0;
            let b = (((c >> 10) & 0x1F) as f32) / 31.0;

            let out_r = (255.0 * (r * 0.94 + g * 0.04 + b * 0.02).powf(1.15)).clamp(0.0, 255.0);
            let out_g = (255.0 * (r * 0.03 + g * 0.92 + b * 0.05).powf(1.15)).clamp(0.0, 255.0);
            let out_b = (255.0 * (r * 0.01 + g * 0.07 + b * 0.92).powf(1.15)).clamp(0.0, 255.0);

            table[c] = [out_r as u8, out_g as u8, out_b as u8, 255];
        }
        table.try_into().expect("32768-entry LUT")
    })
}

#[inline]
pub fn ags_001_lut() -> &'static [[u8; 4]; 32_768] {
    AGS001_LUT.get_or_init(|| {
        // AGS-001 frontlit soft pastel curve (slight blue shift, gentle desaturation)
        let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
        for c in 0..32_768usize {
            let r = ((c & 0x1F) as f32) / 31.0;
            let g = (((c >> 5) & 0x1F) as f32) / 31.0;
            let b = (((c >> 10) & 0x1F) as f32) / 31.0;

            let out_r =
                (255.0 * (0.04 + 0.82 * (r * 0.84 + g * 0.12 + b * 0.04))).clamp(0.0, 255.0);
            let out_g =
                (255.0 * (0.05 + 0.82 * (r * 0.06 + g * 0.82 + b * 0.12))).clamp(0.0, 255.0);
            let out_b =
                (255.0 * (0.08 + 0.84 * (r * 0.04 + g * 0.10 + b * 0.86))).clamp(0.0, 255.0);

            table[c] = [out_r as u8, out_g as u8, out_b as u8, 255];
        }
        table.try_into().expect("32768-entry LUT")
    })
}

#[inline]
pub fn lut_for_profile(
    color_correction: bool,
    profile: ColorProfileChoice,
) -> &'static [[u8; 4]; 32_768] {
    if !color_correction || profile == ColorProfileChoice::Linear {
        linear_lut()
    } else {
        match profile {
            ColorProfileChoice::Agb001 => agb_srgb_lut(),
            ColorProfileChoice::Ags101 => ags_101_lut(),
            ColorProfileChoice::Ags001 => ags_001_lut(),
            ColorProfileChoice::Linear => linear_lut(),
        }
    }
}

/// Convert raw BGR555 framebuffer pixels to unfiltered RGBA8 bytes for GPU upload.
pub fn fill_linear_rgba(fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT], dst_rgba: &mut [u8]) {
    let lut = linear_lut();
    for (idx, &px) in fb_bgr555.iter().enumerate() {
        let off = idx * 4;
        dst_rgba[off..off + 4].copy_from_slice(&lut[(px & 0x7FFF) as usize]);
    }
}

#[derive(Clone, Copy)]
struct FrameComputeOptions {
    color_correction: bool,
    profile: ColorProfileChoice,
    frame_blending: bool,
    lcd_ghosting: bool,
    update_prev_history: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SyncCacheKey {
    frame_seq: u64,
    color_correction: bool,
    profile: ColorProfileChoice,
    frame_blending: bool,
    lcd_ghosting: bool,
}

pub struct VideoPostProcessor {
    pub prev_rgba: Vec<u8>,
    pub curr_rgba: Vec<u8>,
    pub grid_rgba: Vec<u8>,
    pub prev_bgr555: Vec<u16>,
    pub has_prev_frame: bool,
    pub frame_blending: bool,
    last_sync_key: Option<SyncCacheKey>,

    pub base_rgb: Vec<[u8; 3]>,
    pub prev_rgb: Vec<[u8; 3]>,
    pub output_image: egui::ColorImage,
    pub filter: DisplayFilter,
    pub lcd_grid_intensity: f32,
}

impl Default for VideoPostProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoPostProcessor {
    pub fn new() -> Self {
        Self {
            prev_rgba: vec![0; LCD_WIDTH * LCD_HEIGHT * 4],
            curr_rgba: vec![0; LCD_WIDTH * LCD_HEIGHT * 4],
            grid_rgba: vec![0; (LCD_WIDTH * 3) * (LCD_HEIGHT * 3) * 4],
            prev_bgr555: vec![0; LCD_WIDTH * LCD_HEIGHT],
            has_prev_frame: false,
            frame_blending: false,
            last_sync_key: None,

            base_rgb: vec![[0u8; 3]; LCD_WIDTH * LCD_HEIGHT],
            prev_rgb: vec![[0u8; 3]; LCD_WIDTH * LCD_HEIGHT],
            output_image: egui::ColorImage::new(
                [LCD_WIDTH * 3, LCD_HEIGHT * 3],
                egui::Color32::BLACK,
            ),
            filter: DisplayFilter::MyBoySmooth,
            lcd_grid_intensity: 0.08,
        }
    }

    pub fn reset(&mut self) {
        self.prev_rgba.fill(0);
        self.curr_rgba.fill(0);
        self.prev_bgr555.fill(0);
        self.prev_rgb.fill([0, 0, 0]);
        self.base_rgb.fill([0, 0, 0]);
        self.has_prev_frame = false;
        self.last_sync_key = None;
    }

    /// Prewarms the filter/shader pipelines and allocations before frame 0 so cold-start
    /// latency (e.g. 18.9 ms) is not incurred during active presentation.
    pub fn prewarm(
        &mut self,
        color_correction: bool,
        lcd_ghosting: bool,
        filter: DisplayFilter,
        color_correction_fast: bool,
        lcd_grid_intensity: f32,
    ) {
        let dummy = [0x7FFFu16; LCD_WIDTH * LCD_HEIGHT];
        let _ = raw_lut_3();
        let _ = srgb_lut_3();
        let _ = linear_lut();
        for &profile in &ColorProfileChoice::ALL {
            let _ = lut_for_profile(true, profile);
        }
        self.sync_rgba_for_capture(
            &dummy,
            Some(&dummy),
            1,
            color_correction,
            ColorProfileChoice::Agb001,
            false,
            lcd_ghosting,
        );
        let _ = self.process_display_filter(
            &dummy,
            color_correction,
            lcd_ghosting,
            filter,
            color_correction_fast,
            lcd_grid_intensity,
        );
        self.reset();
    }

    fn compute_rgba_frame(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb_bgr555: Option<&[u16; LCD_WIDTH * LCD_HEIGHT]>,
        opts: FrameComputeOptions,
    ) {
        let lut = lut_for_profile(opts.color_correction, opts.profile);
        let active_prev: Option<&[u16]> = if opts.frame_blending {
            prev_fb_bgr555
                .map(|s| s.as_slice())
                .or_else(|| self.has_prev_frame.then_some(self.prev_bgr555.as_slice()))
        } else {
            None
        };

        for (idx, &px) in fb_bgr555.iter().enumerate() {
            let off = idx * 4;
            let mapped = lut[(px & 0x7FFF) as usize];
            let blended = if let Some(prev_buf) = active_prev {
                let prev_mapped = lut[(prev_buf[idx] & 0x7FFF) as usize];
                [
                    ((mapped[0] as u16 + prev_mapped[0] as u16) >> 1) as u8,
                    ((mapped[1] as u16 + prev_mapped[1] as u16) >> 1) as u8,
                    ((mapped[2] as u16 + prev_mapped[2] as u16) >> 1) as u8,
                    255,
                ]
            } else {
                mapped
            };

            if opts.lcd_ghosting && !opts.frame_blending {
                // Blend ~65% new frame + ~35% previous frame for authentic TFT persistence.
                let r = ((blended[0] as u16 * 166 + self.prev_rgba[off] as u16 * 90) >> 8) as u8;
                let g =
                    ((blended[1] as u16 * 166 + self.prev_rgba[off + 1] as u16 * 90) >> 8) as u8;
                let b =
                    ((blended[2] as u16 * 166 + self.prev_rgba[off + 2] as u16 * 90) >> 8) as u8;
                self.curr_rgba[off] = r;
                self.curr_rgba[off + 1] = g;
                self.curr_rgba[off + 2] = b;
                self.curr_rgba[off + 3] = 255;
            } else {
                self.curr_rgba[off..off + 4].copy_from_slice(&blended);
            }
        }

        if opts.update_prev_history {
            self.prev_rgba.copy_from_slice(&self.curr_rgba);
            self.prev_bgr555.copy_from_slice(fb_bgr555);
            self.has_prev_frame = true;
        }
    }

    pub fn process_frame(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        color_correction: bool,
        profile: ColorProfileChoice,
        lcd_ghosting: bool,
        scanline_grid: bool,
    ) -> ColorImage {
        self.process_frame_with_blending(
            fb_bgr555,
            None,
            color_correction,
            profile,
            self.frame_blending,
            lcd_ghosting,
            scanline_grid,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn process_frame_with_blending(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb_bgr555: Option<&[u16; LCD_WIDTH * LCD_HEIGHT]>,
        color_correction: bool,
        profile: ColorProfileChoice,
        frame_blending: bool,
        lcd_ghosting: bool,
        scanline_grid: bool,
    ) -> ColorImage {
        self.compute_rgba_frame(
            fb_bgr555,
            prev_fb_bgr555,
            FrameComputeOptions {
                color_correction,
                profile,
                frame_blending,
                lcd_ghosting,
                update_prev_history: true,
            },
        );

        if !scanline_grid {
            return ColorImage::from_rgba_unmultiplied([LCD_WIDTH, LCD_HEIGHT], &self.curr_rgba);
        }

        // Upscale 3x (720x480) to draw crisp horizontal scanlines + vertical subpixel LCD grid.
        let dst_w = LCD_WIDTH * 3;
        let dst_h = LCD_HEIGHT * 3;
        for y in 0..LCD_HEIGHT {
            for x in 0..LCD_WIDTH {
                let src = (y * LCD_WIDTH + x) * 4;
                let r = self.curr_rgba[src] as u16;
                let g = self.curr_rgba[src + 1] as u16;
                let b = self.curr_rgba[src + 2] as u16;

                for sy in 0..3 {
                    let row_dim = if sy == 2 { 196 } else { 255 };
                    let dy = y * 3 + sy;
                    for sx in 0..3 {
                        let col_dim = if sx == 2 { 216 } else { 255 };
                        let factor = (row_dim * col_dim) / 255;
                        let dx = x * 3 + sx;
                        let dst = (dy * dst_w + dx) * 4;
                        self.grid_rgba[dst] = ((r * factor) / 255) as u8;
                        self.grid_rgba[dst + 1] = ((g * factor) / 255) as u8;
                        self.grid_rgba[dst + 2] = ((b * factor) / 255) as u8;
                        self.grid_rgba[dst + 3] = 255;
                    }
                }
            }
        }
        ColorImage::from_rgba_unmultiplied([dst_w, dst_h], &self.grid_rgba)
    }

    /// Keep `curr_rgba` synchronized for screenshots & save-state thumbnails when GPU shaders render the viewport.
    #[allow(clippy::too_many_arguments)]
    pub fn sync_rgba_for_capture(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        prev_fb_bgr555: Option<&[u16; LCD_WIDTH * LCD_HEIGHT]>,
        frame_seq: u64,
        color_correction: bool,
        profile: ColorProfileChoice,
        frame_blending: bool,
        lcd_ghosting: bool,
    ) {
        let key = SyncCacheKey {
            frame_seq,
            color_correction,
            profile,
            frame_blending,
            lcd_ghosting,
        };
        if self.last_sync_key == Some(key) {
            return;
        }
        let is_new_frame = self.last_sync_key.is_none_or(|k| k.frame_seq != frame_seq);
        self.compute_rgba_frame(
            fb_bgr555,
            prev_fb_bgr555,
            FrameComputeOptions {
                color_correction,
                profile,
                frame_blending,
                lcd_ghosting,
                update_prev_history: is_new_frame,
            },
        );
        self.last_sync_key = Some(key);
    }

    #[allow(dead_code)]
    pub fn save_thumbnail_png(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        image::save_buffer_with_format(
            path,
            &self.curr_rgba,
            LCD_WIDTH as u32,
            LCD_HEIGHT as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("PNG thumbnail encode failed: {e}"))
    }

    #[allow(dead_code)]
    pub fn save_screenshot_png(&self, rom_title: &str) -> Result<PathBuf, String> {
        let dir = AppConfig::screenshots_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed creating screenshots dir: {e}"))?;
        let sanitized: String = rom_title
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let filename = format!("{sanitized}_{ts}.png");
        let path = dir.join(filename);
        image::save_buffer_with_format(
            &path,
            &self.curr_rgba,
            LCD_WIDTH as u32,
            LCD_HEIGHT as u32,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(path)
    }

    pub fn process_display_filter(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        color_correction: bool,
        lcd_ghosting: bool,
        filter: DisplayFilter,
        green_swap: bool,
        grid_intensity: f32,
    ) -> &ColorImage {
        self.filter = filter;
        self.lcd_grid_intensity = grid_intensity;
        let lut = if color_correction {
            srgb_lut_3()
        } else {
            raw_lut_3()
        };

        // Pass 1: O(1) LUT Color Conversion + Integer LCD Ghosting (~0.05 ms)
        for i in 0..(LCD_WIDTH * LCD_HEIGHT) {
            let mut bgr = (fb_bgr555[i] & 0x7FFF) as usize;
            if green_swap && (i & 1) == 1 {
                let neighbor_g = fb_bgr555[i ^ 1] & 0x03E0;
                bgr = (bgr & !0x03E0) | (neighbor_g as usize);
            }

            let mut rgb = lut[bgr];
            if lcd_ghosting {
                let prev = self.prev_rgb[i];
                rgb[0] = ((rgb[0] as u16 * 3 + prev[0] as u16) >> 2) as u8;
                rgb[1] = ((rgb[1] as u16 * 3 + prev[1] as u16) >> 2) as u8;
                rgb[2] = ((rgb[2] as u16 * 3 + prev[2] as u16) >> 2) as u8;
                self.prev_rgb[i] = rgb;
            }
            self.base_rgb[i] = rgb;

            let off = i * 4;
            self.curr_rgba[off] = rgb[0];
            self.curr_rgba[off + 1] = rgb[1];
            self.curr_rgba[off + 2] = rgb[2];
            self.curr_rgba[off + 3] = 255;
        }

        // Pass 2: Fast Integer Display Filter (~0.15 - 0.25 ms)
        match filter {
            DisplayFilter::XbrSmooth3x => {
                render_xbr_smooth_3x(&self.base_rgb, &mut self.output_image);
            }
            DisplayFilter::MyBoySmooth | DisplayFilter::Sharp1x | DisplayFilter::RawNearest => {
                if self.output_image.size != [LCD_WIDTH, LCD_HEIGHT] {
                    self.output_image =
                        egui::ColorImage::new([LCD_WIDTH, LCD_HEIGHT], egui::Color32::BLACK);
                }
                for (dst, &[r, g, b]) in self
                    .output_image
                    .pixels
                    .iter_mut()
                    .zip(self.base_rgb.iter())
                {
                    *dst = egui::Color32::from_rgb(r, g, b);
                }
            }

            DisplayFilter::SoftHandheld => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;
                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c = egui::Color32::from_rgb(r, g, b);
                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] = c;
                        self.output_image.pixels[dst_r0 + dx + 1] = c;
                        self.output_image.pixels[dst_r1 + dx] = c;
                        self.output_image.pixels[dst_r1 + dx + 1] = c;
                    }
                }
            }

            DisplayFilter::CrtScanlines => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;
                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c0 = egui::Color32::from_rgb(r, g, b);
                        let c1 = egui::Color32::from_rgb(r / 2, g / 2, b / 2);
                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] = c0;
                        self.output_image.pixels[dst_r0 + dx + 1] = c0;
                        self.output_image.pixels[dst_r1 + dx] = c1;
                        self.output_image.pixels[dst_r1 + dx + 1] = c1;
                    }
                }
            }

            DisplayFilter::AuthenticGbaLcd => {
                let out_w = LCD_WIDTH * 3;
                let out_h = LCD_HEIGHT * 3;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }

                // Precompute dynamic integer multipliers once outside pixel loop (0.0 <= intensity <= 0.25)
                // At default 8% (0.08), produces subtle authentic ~8% cell boundary gaps (240/235/226):
                // Left: warm R bias, ~97% G/B (248/245)
                // Right: ~94% (240/242/246)
                // Scanline: ~92% (235/235/238)
                // Corner: ~88% (226/226/228)
                let intensity = grid_intensity.clamp(0.0, 0.25);
                let mul_left_g = (256.0 * (1.0 - intensity * 0.40)).round() as u16;
                let mul_left_b = (256.0 * (1.0 - intensity * 0.55)).round() as u16;
                let mul_right_r = (256.0 * (1.0 - intensity * 0.78)).round() as u16;
                let mul_right_g = (256.0 * (1.0 - intensity * 0.68)).round() as u16;
                let mul_right_b = (256.0 * (1.0 - intensity * 0.49)).round() as u16;
                let mul_scan_r = (256.0 * (1.0 - intensity * 1.02)).round() as u16;
                let mul_scan_g = (256.0 * (1.0 - intensity * 1.02)).round() as u16;
                let mul_scan_b = (256.0 * (1.0 - intensity * 0.88)).round() as u16;
                let mul_corner_r = (256.0 * (1.0 - intensity * 1.46)).round() as u16;
                let mul_corner_g = (256.0 * (1.0 - intensity * 1.46)).round() as u16;
                let mul_corner_b = (256.0 * (1.0 - intensity * 1.37)).round() as u16;

                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let r0 = (y * 3) * out_w;
                    let r1 = r0 + out_w;
                    let r2 = r1 + out_w;

                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c_left = egui::Color32::from_rgb(
                            r,
                            ((g as u16 * mul_left_g) >> 8) as u8,
                            ((b as u16 * mul_left_b) >> 8) as u8,
                        );
                        let c_mid = egui::Color32::from_rgb(r, g, b);
                        let c_right = egui::Color32::from_rgb(
                            ((r as u16 * mul_right_r) >> 8) as u8,
                            ((g as u16 * mul_right_g) >> 8) as u8,
                            ((b as u16 * mul_right_b) >> 8) as u8,
                        );
                        let c_scan = egui::Color32::from_rgb(
                            ((r as u16 * mul_scan_r) >> 8) as u8,
                            ((g as u16 * mul_scan_g) >> 8) as u8,
                            ((b as u16 * mul_scan_b) >> 8) as u8,
                        );
                        let c_corner = egui::Color32::from_rgb(
                            ((r as u16 * mul_corner_r) >> 8) as u8,
                            ((g as u16 * mul_corner_g) >> 8) as u8,
                            ((b as u16 * mul_corner_b) >> 8) as u8,
                        );

                        let dx = x * 3;
                        self.output_image.pixels[r0 + dx] = c_left;
                        self.output_image.pixels[r0 + dx + 1] = c_mid;
                        self.output_image.pixels[r0 + dx + 2] = c_right;

                        self.output_image.pixels[r1 + dx] = c_left;
                        self.output_image.pixels[r1 + dx + 1] = c_mid;
                        self.output_image.pixels[r1 + dx + 2] = c_right;

                        self.output_image.pixels[r2 + dx] = c_scan;
                        self.output_image.pixels[r2 + dx + 1] = c_scan;
                        self.output_image.pixels[r2 + dx + 2] = c_corner;
                    }
                }
            }

            DisplayFilter::SmoothScale2x => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let ym1 = y.saturating_sub(1);
                    let yp1 = (y + 1).min(LCD_HEIGHT - 1);
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;

                    for x in 0..LCD_WIDTH {
                        let xm1 = x.saturating_sub(1);
                        let xp1 = (x + 1).min(LCD_WIDTH - 1);

                        let p = self.base_rgb[y * LCD_WIDTH + x];
                        let a = self.base_rgb[ym1 * LCD_WIDTH + x]; // Up
                        let b = self.base_rgb[y * LCD_WIDTH + xp1]; // Right
                        let c = self.base_rgb[y * LCD_WIDTH + xm1]; // Left
                        let d = self.base_rgb[yp1 * LCD_WIDTH + x]; // Down

                        let mut p1 = p;
                        let mut p2 = p;
                        let mut p3 = p;
                        let mut p4 = p;

                        if c == a && c != d && a != b {
                            p1 = a;
                        }
                        if a == b && a != c && b != d {
                            p2 = b;
                        }
                        if d == c && d != b && c != a {
                            p3 = c;
                        }
                        if b == d && b != a && d != c {
                            p4 = d;
                        }

                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] =
                            egui::Color32::from_rgb(p1[0], p1[1], p1[2]);
                        self.output_image.pixels[dst_r0 + dx + 1] =
                            egui::Color32::from_rgb(p2[0], p2[1], p2[2]);
                        self.output_image.pixels[dst_r1 + dx] =
                            egui::Color32::from_rgb(p3[0], p3[1], p3[2]);
                        self.output_image.pixels[dst_r1 + dx + 1] =
                            egui::Color32::from_rgb(p4[0], p4[1], p4[2]);
                    }
                }
            }
        }

        &self.output_image
    }

    /// Briefly times every display filter on a patterned 240x160 frame during prewarm and
    /// returns the measured costs sorted ascending.
    pub fn measure_filter_costs(
        &mut self,
        color_correction: bool,
        lcd_ghosting: bool,
        color_correction_fast: bool,
        lcd_grid_intensity: f32,
    ) -> Vec<FilterCost> {
        let pattern = test_filter_pattern_fb();
        self.prewarm(
            color_correction,
            lcd_ghosting,
            DisplayFilter::Sharp1x,
            color_correction_fast,
            lcd_grid_intensity,
        );
        let mut costs = Vec::with_capacity(DisplayFilter::ALL.len());
        for &filter in &DisplayFilter::ALL {
            for _ in 0..2 {
                let img = self.process_display_filter(
                    &pattern,
                    color_correction,
                    lcd_ghosting,
                    filter,
                    color_correction_fast,
                    lcd_grid_intensity,
                );
                std::hint::black_box(img.pixels.as_ptr());
            }
            let mut samples = [0.0f64; 5];
            for slot in &mut samples {
                let t0 = Instant::now();
                for _ in 0..4 {
                    let img = self.process_display_filter(
                        &pattern,
                        color_correction,
                        lcd_ghosting,
                        filter,
                        color_correction_fast,
                        lcd_grid_intensity,
                    );
                    std::hint::black_box(img.pixels.as_ptr());
                }
                *slot = t0.elapsed().as_secs_f64() * 1000.0 / 4.0;
            }
            samples.sort_by(f64::total_cmp);
            costs.push(FilterCost {
                filter,
                cost_ms: samples[2],
            });
        }
        self.reset();
        sort_filter_costs(&mut costs);
        costs
    }
}

#[inline(always)]
fn color_dist(c1: [u8; 3], c2: [u8; 3]) -> u32 {
    let dr = (c1[0] as i32 - c2[0] as i32).unsigned_abs();
    let dg = (c1[1] as i32 - c2[1] as i32).unsigned_abs();
    let db = (c1[2] as i32 - c2[2] as i32).unsigned_abs();
    dr + dg + db
}

#[inline(always)]
fn blend_corner_xbr(p: [u8; 3], c1: [u8; 3], c2: [u8; 3], diag: [u8; 3]) -> egui::Color32 {
    let d_card = color_dist(c1, c2);
    let d_diag = color_dist(p, diag);
    let d_p_c1 = color_dist(p, c1);
    let d_p_c2 = color_dist(p, c2);

    // Precise diagonal edge check:
    // 1. Cardinal neighbors c1 and c2 are similar to each other (same color/ramp)
    // 2. p differs from BOTH c1 and c2 (p is NOT connected along horizontal or vertical axis)
    // 3. c1 and c2 contrast with the diagonal neighbor relative to p
    if d_card < d_diag && d_card <= 40 && d_p_c1 >= 24 && d_p_c2 >= 24 {
        let n_r = (c1[0] as u16 + c2[0] as u16) >> 1;
        let n_g = (c1[1] as u16 + c2[1] as u16) >> 1;
        let n_b = (c1[2] as u16 + c2[2] as u16) >> 1;

        egui::Color32::from_rgb(
            ((p[0] as u16 + n_r) >> 1) as u8,
            ((p[1] as u16 + n_g) >> 1) as u8,
            ((p[2] as u16 + n_b) >> 1) as u8,
        )
    } else {
        egui::Color32::from_rgb(p[0], p[1], p[2])
    }
}

pub fn render_xbr_smooth_3x(base_rgb: &[[u8; 3]], output_image: &mut egui::ColorImage) {
    let out_w = LCD_WIDTH * 3;
    let out_h = LCD_HEIGHT * 3;
    if output_image.size != [out_w, out_h] {
        *output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
    }

    for y in 0..LCD_HEIGHT {
        let ym1 = y.saturating_sub(1);
        let yp1 = (y + 1).min(LCD_HEIGHT - 1);
        let dst_r0 = (y * 3) * out_w;
        let dst_r1 = dst_r0 + out_w;
        let dst_r2 = dst_r1 + out_w;

        for x in 0..LCD_WIDTH {
            let xm1 = x.saturating_sub(1);
            let xp1 = (x + 1).min(LCD_WIDTH - 1);

            let p = base_rgb[y * LCD_WIDTH + x];
            let u = base_rgb[ym1 * LCD_WIDTH + x];
            let d = base_rgb[yp1 * LCD_WIDTH + x];
            let l = base_rgb[y * LCD_WIDTH + xm1];
            let r = base_rgb[y * LCD_WIDTH + xp1];

            let ul = base_rgb[ym1 * LCD_WIDTH + xm1];
            let ur = base_rgb[ym1 * LCD_WIDTH + xp1];
            let dl = base_rgb[yp1 * LCD_WIDTH + xm1];
            let dr = base_rgb[yp1 * LCD_WIDTH + xp1];

            let c_center = egui::Color32::from_rgb(p[0], p[1], p[2]);
            let c_tl = blend_corner_xbr(p, u, l, ul);
            let c_tr = blend_corner_xbr(p, u, r, ur);
            let c_bl = blend_corner_xbr(p, d, l, dl);
            let c_br = blend_corner_xbr(p, d, r, dr);

            let dx = x * 3;
            // Row 0: [0, 0] (TL corner), [0, 1] (Top edge), [0, 2] (TR corner)
            output_image.pixels[dst_r0 + dx] = c_tl;
            output_image.pixels[dst_r0 + dx + 1] = c_center;
            output_image.pixels[dst_r0 + dx + 2] = c_tr;

            // Row 1: [1, 0] (Left edge), [1, 1] (Center), [1, 2] (Right edge)
            output_image.pixels[dst_r1 + dx] = c_center;
            output_image.pixels[dst_r1 + dx + 1] = c_center;
            output_image.pixels[dst_r1 + dx + 2] = c_center;

            // Row 2: [2, 0] (BL corner), [2, 1] (Bottom edge), [2, 2] (BR corner)
            output_image.pixels[dst_r2 + dx] = c_bl;
            output_image.pixels[dst_r2 + dx + 1] = c_center;
            output_image.pixels[dst_r2 + dx + 2] = c_br;
        }
    }
}

pub struct FrontendBridge {
    // Precomputed 15-bit BGR555 -> 24-bit RGB Lookup Tables (0.02 ms lookup!)
    pub srgb_lut: Box<[[u8; 3]; 32768]>,
    pub raw_lut: Box<[[u8; 3]; 32768]>,

    pub base_rgb: Vec<[u8; 3]>,
    pub prev_rgb: Vec<[u8; 3]>,
    pub output_image: egui::ColorImage,

    pub filter: DisplayFilter,
    pub lcd_grid_intensity: f32,
    pub color_correction: bool,
    pub lcd_ghosting: bool,
    pub green_swap: bool,

    pub keypad_mask: u16,
    pub audio_prod: ringbuf::HeapProd<i16>,
    pub audio_muted: bool,
    pub volume: f32,
    pub host_sample_rate: u32,
    pub resample_phase: f64,
    pub prev_audio_l: i16,
    pub prev_audio_r: i16,
}

impl FrontendBridge {
    pub fn new(audio_prod: ringbuf::HeapProd<i16>) -> Self {
        let mut srgb_lut = Box::new([[0u8; 3]; 32768]);
        let mut raw_lut = Box::new([[0u8; 3]; 32768]);

        for bgr in 0..32768usize {
            let r5 = (bgr & 0x1F) as f32;
            let g5 = ((bgr >> 5) & 0x1F) as f32;
            let b5 = ((bgr >> 10) & 0x1F) as f32;

            raw_lut[bgr] = [
                ((r5 as u8) << 3) | ((r5 as u8) >> 2),
                ((g5 as u8) << 3) | ((g5 as u8) >> 2),
                ((b5 as u8) << 3) | ((b5 as u8) >> 2),
            ];

            let rl = (r5 / 31.0).powf(3.6);
            let gl = (g5 / 31.0).powf(3.6);
            let bl = (b5 / 31.0).powf(3.6);

            let sr = (0.845 * rl + 0.195 * gl - 0.040 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);
            let sg = (0.085 * rl + 0.690 * gl + 0.225 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);
            let sb = (0.050 * rl + 0.210 * gl + 0.740 * bl)
                .clamp(0.0, 1.0)
                .powf(1.0 / 2.2);

            srgb_lut[bgr] = [(sr * 255.0) as u8, (sg * 255.0) as u8, (sb * 255.0) as u8];
        }

        Self {
            srgb_lut,
            raw_lut,
            base_rgb: vec![[0u8; 3]; LCD_WIDTH * LCD_HEIGHT],
            prev_rgb: vec![[0u8; 3]; LCD_WIDTH * LCD_HEIGHT],
            output_image: egui::ColorImage::new(
                [LCD_WIDTH * 3, LCD_HEIGHT * 3],
                egui::Color32::BLACK,
            ),
            filter: DisplayFilter::MyBoySmooth,
            lcd_grid_intensity: 0.08,
            color_correction: true,
            lcd_ghosting: true,
            green_swap: false,
            keypad_mask: 0x03FF,
            audio_prod,
            audio_muted: false,
            volume: 0.85,
            host_sample_rate: 48_000,
            resample_phase: 0.0,
            prev_audio_l: 0,
            prev_audio_r: 0,
        }
    }

    pub fn texture_options(&self) -> egui::TextureOptions {
        self.filter.texture_options()
    }
}

impl HostFrontend for FrontendBridge {
    fn present_frame(&mut self, framebuffer_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT]) {
        let lut = if self.color_correction {
            &self.srgb_lut
        } else {
            &self.raw_lut
        };

        // Pass 1: O(1) LUT Color Conversion + Integer LCD Ghosting (~0.05 ms)
        for i in 0..(LCD_WIDTH * LCD_HEIGHT) {
            let mut bgr = (framebuffer_bgr555[i] & 0x7FFF) as usize;
            if self.green_swap && (i & 1) == 1 {
                let neighbor_g = framebuffer_bgr555[i ^ 1] & 0x03E0;
                bgr = (bgr & !0x03E0) | (neighbor_g as usize);
            }

            let mut rgb = lut[bgr];
            if self.lcd_ghosting {
                let prev = self.prev_rgb[i];
                rgb[0] = ((rgb[0] as u16 * 3 + prev[0] as u16) >> 2) as u8;
                rgb[1] = ((rgb[1] as u16 * 3 + prev[1] as u16) >> 2) as u8;
                rgb[2] = ((rgb[2] as u16 * 3 + prev[2] as u16) >> 2) as u8;
                self.prev_rgb[i] = rgb;
            }
            self.base_rgb[i] = rgb;
        }

        // Pass 2: Fast Integer Display Filter (~0.15 - 0.25 ms)
        match self.filter {
            DisplayFilter::XbrSmooth3x => {
                render_xbr_smooth_3x(&self.base_rgb, &mut self.output_image);
            }
            DisplayFilter::MyBoySmooth | DisplayFilter::Sharp1x | DisplayFilter::RawNearest => {
                if self.output_image.size != [LCD_WIDTH, LCD_HEIGHT] {
                    self.output_image =
                        egui::ColorImage::new([LCD_WIDTH, LCD_HEIGHT], egui::Color32::BLACK);
                }
                for (dst, &[r, g, b]) in self
                    .output_image
                    .pixels
                    .iter_mut()
                    .zip(self.base_rgb.iter())
                {
                    *dst = egui::Color32::from_rgb(r, g, b);
                }
            }

            DisplayFilter::SoftHandheld => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;
                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c = egui::Color32::from_rgb(r, g, b);
                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] = c;
                        self.output_image.pixels[dst_r0 + dx + 1] = c;
                        self.output_image.pixels[dst_r1 + dx] = c;
                        self.output_image.pixels[dst_r1 + dx + 1] = c;
                    }
                }
            }

            DisplayFilter::CrtScanlines => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;
                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c0 = egui::Color32::from_rgb(r, g, b);
                        let c1 = egui::Color32::from_rgb(r / 2, g / 2, b / 2);
                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] = c0;
                        self.output_image.pixels[dst_r0 + dx + 1] = c0;
                        self.output_image.pixels[dst_r1 + dx] = c1;
                        self.output_image.pixels[dst_r1 + dx + 1] = c1;
                    }
                }
            }

            DisplayFilter::AuthenticGbaLcd => {
                let out_w = LCD_WIDTH * 3;
                let out_h = LCD_HEIGHT * 3;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }

                // Precompute dynamic integer multipliers once outside pixel loop (0.0 <= intensity <= 0.25)
                // At default 8% (0.08), produces subtle authentic ~8% cell boundary gaps (240/235/226)
                let intensity = self.lcd_grid_intensity.clamp(0.0, 0.25);
                let mul_left_g = (256.0 * (1.0 - intensity * 0.40)).round() as u16;
                let mul_left_b = (256.0 * (1.0 - intensity * 0.55)).round() as u16;
                let mul_right_r = (256.0 * (1.0 - intensity * 0.78)).round() as u16;
                let mul_right_g = (256.0 * (1.0 - intensity * 0.68)).round() as u16;
                let mul_right_b = (256.0 * (1.0 - intensity * 0.49)).round() as u16;
                let mul_scan_r = (256.0 * (1.0 - intensity * 1.02)).round() as u16;
                let mul_scan_g = (256.0 * (1.0 - intensity * 1.02)).round() as u16;
                let mul_scan_b = (256.0 * (1.0 - intensity * 0.88)).round() as u16;
                let mul_corner_r = (256.0 * (1.0 - intensity * 1.46)).round() as u16;
                let mul_corner_g = (256.0 * (1.0 - intensity * 1.46)).round() as u16;
                let mul_corner_b = (256.0 * (1.0 - intensity * 1.37)).round() as u16;

                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let r0 = (y * 3) * out_w;
                    let r1 = r0 + out_w;
                    let r2 = r1 + out_w;

                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c_left = egui::Color32::from_rgb(
                            r,
                            ((g as u16 * mul_left_g) >> 8) as u8,
                            ((b as u16 * mul_left_b) >> 8) as u8,
                        );
                        let c_mid = egui::Color32::from_rgb(r, g, b);
                        let c_right = egui::Color32::from_rgb(
                            ((r as u16 * mul_right_r) >> 8) as u8,
                            ((g as u16 * mul_right_g) >> 8) as u8,
                            ((b as u16 * mul_right_b) >> 8) as u8,
                        );
                        let c_scan = egui::Color32::from_rgb(
                            ((r as u16 * mul_scan_r) >> 8) as u8,
                            ((g as u16 * mul_scan_g) >> 8) as u8,
                            ((b as u16 * mul_scan_b) >> 8) as u8,
                        );
                        let c_corner = egui::Color32::from_rgb(
                            ((r as u16 * mul_corner_r) >> 8) as u8,
                            ((g as u16 * mul_corner_g) >> 8) as u8,
                            ((b as u16 * mul_corner_b) >> 8) as u8,
                        );

                        let dx = x * 3;
                        self.output_image.pixels[r0 + dx] = c_left;
                        self.output_image.pixels[r0 + dx + 1] = c_mid;
                        self.output_image.pixels[r0 + dx + 2] = c_right;

                        self.output_image.pixels[r1 + dx] = c_left;
                        self.output_image.pixels[r1 + dx + 1] = c_mid;
                        self.output_image.pixels[r1 + dx + 2] = c_right;

                        self.output_image.pixels[r2 + dx] = c_scan;
                        self.output_image.pixels[r2 + dx + 1] = c_scan;
                        self.output_image.pixels[r2 + dx + 2] = c_corner;
                    }
                }
            }

            DisplayFilter::SmoothScale2x => {
                let out_w = LCD_WIDTH * 2;
                let out_h = LCD_HEIGHT * 2;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let ym1 = y.saturating_sub(1);
                    let yp1 = (y + 1).min(LCD_HEIGHT - 1);
                    let dst_r0 = (y * 2) * out_w;
                    let dst_r1 = dst_r0 + out_w;

                    for x in 0..LCD_WIDTH {
                        let xm1 = x.saturating_sub(1);
                        let xp1 = (x + 1).min(LCD_WIDTH - 1);

                        let p = self.base_rgb[y * LCD_WIDTH + x];
                        let a = self.base_rgb[ym1 * LCD_WIDTH + x]; // Up
                        let b = self.base_rgb[y * LCD_WIDTH + xp1]; // Right
                        let c = self.base_rgb[y * LCD_WIDTH + xm1]; // Left
                        let d = self.base_rgb[yp1 * LCD_WIDTH + x]; // Down

                        let mut p1 = p;
                        let mut p2 = p;
                        let mut p3 = p;
                        let mut p4 = p;

                        if c == a && c != d && a != b {
                            p1 = a;
                        }
                        if a == b && a != c && b != d {
                            p2 = b;
                        }
                        if d == c && d != b && c != a {
                            p3 = c;
                        }
                        if b == d && b != a && d != c {
                            p4 = d;
                        }

                        let dx = x * 2;
                        self.output_image.pixels[dst_r0 + dx] =
                            egui::Color32::from_rgb(p1[0], p1[1], p1[2]);
                        self.output_image.pixels[dst_r0 + dx + 1] =
                            egui::Color32::from_rgb(p2[0], p2[1], p2[2]);
                        self.output_image.pixels[dst_r1 + dx] =
                            egui::Color32::from_rgb(p3[0], p3[1], p3[2]);
                        self.output_image.pixels[dst_r1 + dx + 1] =
                            egui::Color32::from_rgb(p4[0], p4[1], p4[2]);
                    }
                }
            }
        }
    }

    fn push_audio_samples(&mut self, samples_interleaved_i16: &[i16]) {
        use ringbuf::traits::{Observer, Producer};
        if self.audio_muted || samples_interleaved_i16.is_empty() {
            return;
        }

        // Dynamic Rate Control (DRC): Keep the 8192-sample buffer centered at 50% (4096 samples)
        let capacity = 8192.0;
        let target_fill = capacity * 0.5;
        let current_fill = self.audio_prod.occupied_len() as f64;
        let fill_delta = (target_fill - current_fill) / target_fill;
        let drc_adjust = (fill_delta * 0.005).clamp(-0.005, 0.005);

        // Base ratio from GBA 48kHz output to actual host soundcard rate + DRC adjustment
        let out_rate = (self.host_sample_rate as f64) * (1.0 + drc_adjust);
        let step = (gba_core::SAMPLE_RATE_HZ as f64) / out_rate.max(8000.0);

        let frames = samples_interleaved_i16.len() / 2;
        for i in 0..frames {
            let cur_l = samples_interleaved_i16[i * 2];
            let cur_r = samples_interleaved_i16[i * 2 + 1];

            while self.resample_phase < 1.0 {
                let frac = self.resample_phase as f32;
                let l = (self.prev_audio_l as f32
                    + (cur_l as f32 - self.prev_audio_l as f32) * frac)
                    * self.volume;
                let r = (self.prev_audio_r as f32
                    + (cur_r as f32 - self.prev_audio_r as f32) * frac)
                    * self.volume;

                let _ = self.audio_prod.try_push(l.clamp(-32768.0, 32767.0) as i16);
                let _ = self.audio_prod.try_push(r.clamp(-32768.0, 32767.0) as i16);
                self.resample_phase += step;
            }
            self.resample_phase -= 1.0;
            self.prev_audio_l = cur_l;
            self.prev_audio_r = cur_r;
        }
    }

    fn poll_keypad(&mut self) -> u16 {
        self.keypad_mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_blending_mixes_50_50_without_exponential_decay() {
        let mut proc = VideoPostProcessor::new();
        let white_fb = [0x7FFF_u16; LCD_WIDTH * LCD_HEIGHT];
        let black_fb = [0x0000_u16; LCD_WIDTH * LCD_HEIGHT];

        // First frame with explicit prev=None should not darken against uninitialized history.
        let img1 = proc.process_frame_with_blending(
            &white_fb,
            None,
            false,
            ColorProfileChoice::Linear,
            true,
            false,
            false,
        );
        assert_eq!(
            img1.pixels[0],
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255)
        );

        // Second frame (black) blended 50/50 with previous frame (white) -> ~127 gray.
        let img2 = proc.process_frame_with_blending(
            &black_fb,
            Some(&white_fb),
            false,
            ColorProfileChoice::Linear,
            true,
            false,
            false,
        );
        assert_eq!(
            img2.pixels[0],
            egui::Color32::from_rgba_unmultiplied(127, 127, 127, 255)
        );

        // Third frame (white) blended with previous raw frame (black) via internal history
        // still produces exact 50/50 mix (no exponential flicker decay).
        proc.frame_blending = true;
        let img3 = proc.process_frame(&white_fb, false, ColorProfileChoice::Linear, false, false);
        assert_eq!(
            img3.pixels[0],
            egui::Color32::from_rgba_unmultiplied(127, 127, 127, 255)
        );

        // Disabling frame_blending restores full crisp white.
        proc.frame_blending = false;
        let img4 = proc.process_frame(&white_fb, false, ColorProfileChoice::Linear, false, false);
        assert_eq!(
            img4.pixels[0],
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255)
        );
    }

    #[test]
    fn frame_blending_with_explicit_color_channels_averages_each_channel() {
        let mut proc = VideoPostProcessor::new();
        // Pure red (r5=31 -> 255) and pure green (g5=31 -> 255)
        let red_fb = [0x001F_u16; LCD_WIDTH * LCD_HEIGHT];
        let green_fb = [0x03E0_u16; LCD_WIDTH * LCD_HEIGHT];

        let img = proc.process_frame_with_blending(
            &red_fb,
            Some(&green_fb),
            false,
            ColorProfileChoice::Linear,
            true,
            false,
            false,
        );
        assert_eq!(
            img.pixels[0],
            egui::Color32::from_rgba_unmultiplied(127, 127, 0, 255)
        );
    }

    #[test]
    fn display_filter_modes_and_frontend_bridge_pipeline() {
        use ringbuf::HeapRb;
        use ringbuf::traits::Split;

        let rb = HeapRb::<i16>::new(1024);
        let (prod, _) = rb.split();
        let mut bridge = FrontendBridge::new(prod);

        let test_fb = [0x7FFF_u16; LCD_WIDTH * LCD_HEIGHT];

        // 1. xBR 3x (Laptop HD) (3x -> 720x480)
        bridge.filter = DisplayFilter::XbrSmooth3x;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 3, LCD_HEIGHT * 3]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::LINEAR);

        // 2. Authentic GBA LCD (3x -> 720x480)
        bridge.filter = DisplayFilter::AuthenticGbaLcd;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 3, LCD_HEIGHT * 3]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::LINEAR);

        // 3. Soft Handheld (2x -> 480x320)
        bridge.filter = DisplayFilter::SoftHandheld;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 2, LCD_HEIGHT * 2]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::LINEAR);

        // 4. Smooth Scale2x (2x -> 480x320)
        bridge.filter = DisplayFilter::SmoothScale2x;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 2, LCD_HEIGHT * 2]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::NEAREST);

        // 5. Raw Nearest (1x -> 240x160)
        bridge.filter = DisplayFilter::RawNearest;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH, LCD_HEIGHT]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::NEAREST);

        // Also test VideoPostProcessor::process_display_filter directly
        let mut proc = VideoPostProcessor::new();
        for &f in &DisplayFilter::ALL {
            let img = proc.process_display_filter(&test_fb, true, true, f, false, 0.08);
            let (exp_w, exp_h) = f.target_dimensions();
            assert_eq!(img.size, [exp_w, exp_h]);
        }

        // Verify AuthenticGbaLcd intensity scaling on pure white (0x7FFF)
        // With intensity 0.08, center is full white (255), scanline is subtle ~92% (234), corner is ~88% (225)
        let lcd_img_8pct = proc.process_display_filter(
            &test_fb,
            false,
            false,
            DisplayFilter::AuthenticGbaLcd,
            false,
            0.08,
        );
        let out_w = LCD_WIDTH * 3;
        // Pixel (1, 0) is c_mid
        assert_eq!(
            lcd_img_8pct.pixels[1],
            egui::Color32::from_rgb(255, 255, 255)
        );
        // Pixel (0, 2) is c_scan: [234, 234, 237]
        assert_eq!(
            lcd_img_8pct.pixels[2 * out_w],
            egui::Color32::from_rgb(234, 234, 237)
        );
        // Pixel (2, 2) is c_corner: [225, 225, 227]
        assert_eq!(
            lcd_img_8pct.pixels[2 * out_w + 2],
            egui::Color32::from_rgb(225, 225, 227)
        );

        // With intensity 0.0, grid is disabled: all subpixels evaluate to full white (255)
        let lcd_img_0pct = proc.process_display_filter(
            &test_fb,
            false,
            false,
            DisplayFilter::AuthenticGbaLcd,
            false,
            0.0,
        );
        assert_eq!(
            lcd_img_0pct.pixels[1],
            egui::Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(
            lcd_img_0pct.pixels[2 * out_w],
            egui::Color32::from_rgb(255, 255, 255)
        );
        assert_eq!(
            lcd_img_0pct.pixels[2 * out_w + 2],
            egui::Color32::from_rgb(255, 255, 255)
        );
    }

    #[test]
    fn xbr_smooth_3x_edge_smoothing_and_sharp_interior() {
        let mut proc = VideoPostProcessor::new();
        // Create an all-white framebuffer (0x7FFF) with a black pixel (0x0000) at (0, 0), (1, 0), (0, 1)
        // so that for pixel (1, 1) [white]:
        //   Up (1, 0) is black
        //   Left (0, 1) is black
        //   Up-Left (0, 0) is black
        //   Down, Right, and all other neighbors are white
        let mut test_fb = [0x7FFF_u16; LCD_WIDTH * LCD_HEIGHT];
        test_fb[0] = 0x0000; // (0, 0)
        test_fb[1] = 0x0000; // (1, 0) - Up for (1, 1)
        test_fb[LCD_WIDTH] = 0x0000; // (0, 1) - Left for (1, 1)

        let img = proc.process_display_filter(
            &test_fb,
            false,
            false,
            DisplayFilter::XbrSmooth3x,
            false,
            0.08,
        );
        let out_w = LCD_WIDTH * 3;
        assert_eq!(img.size, [LCD_WIDTH * 3, LCD_HEIGHT * 3]);

        // Examine the 3x3 block corresponding to source pixel (1, 1):
        // Destination rows: 3, 4, 5
        // Destination cols: 3, 4, 5
        let r0 = 3 * out_w;
        let r1 = 4 * out_w;
        let r2 = 5 * out_w;

        // 1. Center [1, 1] at (4, 4) must be 100% original pixel sharpness (white: 255, 255, 255)
        assert_eq!(img.pixels[r1 + 4], egui::Color32::from_rgb(255, 255, 255));

        // 2. Orthogonal edges must also be 100% original pixel sharpness:
        // Top edge [0, 1] at (3, 4)
        assert_eq!(img.pixels[r0 + 4], egui::Color32::from_rgb(255, 255, 255));
        // Left edge [1, 0] at (4, 3)
        assert_eq!(img.pixels[r1 + 3], egui::Color32::from_rgb(255, 255, 255));
        // Right edge [1, 2] at (4, 5)
        assert_eq!(img.pixels[r1 + 5], egui::Color32::from_rgb(255, 255, 255));
        // Bottom edge [2, 1] at (5, 4)
        assert_eq!(img.pixels[r2 + 4], egui::Color32::from_rgb(255, 255, 255));

        // 3. Top-Left corner [0, 0] at (3, 3):
        // Up is black [0, 0, 0], Left is black [0, 0, 0], Up-Left is black [0, 0, 0].
        // Cardinal neighbors are identical and similar, contrasting with white center (255, 255, 255).
        // Blends 50/50: ((255 + 0) >> 1) = 127
        assert_eq!(img.pixels[r0 + 3], egui::Color32::from_rgb(127, 127, 127));

        // 4. Bottom-Right corner [2, 2] at (5, 5):
        // Down is white, Right is white, center is white -> stays 100% white (255, 255, 255)
        assert_eq!(img.pixels[r2 + 5], egui::Color32::from_rgb(255, 255, 255));
    }

    #[test]
    fn quality_governor_jumps_directly_under_50_pct_budget_with_hysteresis_and_override() {
        let costs = vec![
            FilterCost {
                filter: DisplayFilter::Sharp1x,
                cost_ms: 0.4,
            },
            FilterCost {
                filter: DisplayFilter::MyBoySmooth,
                cost_ms: 0.5,
            },
            FilterCost {
                filter: DisplayFilter::SoftHandheld,
                cost_ms: 1.5,
            },
            FilterCost {
                filter: DisplayFilter::CrtScanlines,
                cost_ms: 2.2,
            },
            FilterCost {
                filter: DisplayFilter::AuthenticGbaLcd,
                cost_ms: 4.5,
            },
            FilterCost {
                filter: DisplayFilter::SmoothScale2x,
                cost_ms: 6.5,
            },
            FilterCost {
                filter: DisplayFilter::XbrSmooth3x,
                cost_ms: 10.0,
            },
        ];
        // Under work_p95 = 13.5 ms with XbrSmooth3x (10.0 ms): est_base = 3.5 ms.
        // 50% of 16.7427 ms = 8.371 ms -> max affordable filter cost is 4.871 ms -> AuthenticGbaLcd (4.5 ms).
        let mut gov =
            QualityGovernor::with_measured_costs(DisplayFilter::XbrSmooth3x, true, costs.clone());
        assert_eq!(gov.measured_costs().len(), 7);
        // Transient ~2.3s burst above 70% must NOT trigger premature step-down (hysteresis).
        for _ in 0..140 {
            assert_eq!(gov.record_frame(14.0), None);
        }
        // Recovery period clears rolling p95 and resets sustained overload streak.
        for _ in 0..120 {
            assert_eq!(gov.record_frame(5.0), None);
        }
        // Sustained overload (>70% = >11.72 ms) jumps straight to AuthenticGbaLcd in one step.
        let mut saw_step = None;
        for f in 0..240 {
            if let Some(ev) = gov.record_frame(13.5) {
                saw_step = Some((f, ev));
                break;
            }
        }
        let (trigger_idx, ev) = saw_step.expect("expected direct measured-cost jump");
        assert!(
            trigger_idx < 190,
            "expected settle within ~3s (<190 frames), got {trigger_idx}"
        );
        assert_eq!(ev.from, DisplayFilter::XbrSmooth3x);
        assert_eq!(ev.to, DisplayFilter::AuthenticGbaLcd);
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);

        // Also check cold-start settle time (<180 frames = ~3.0s) from an empty window.
        let mut cold_gov = QualityGovernor::with_measured_costs(DisplayFilter::XbrSmooth3x, true, costs);
        let cold_ev = (0..185)
            .find_map(|f| cold_gov.record_frame(13.5).map(|ev| (f, ev)))
            .expect("expected cold-start settle within 180 frames");
        assert_eq!(cold_ev.1.to, DisplayFilter::AuthenticGbaLcd);

        // During 5-minute (18,000f) lockout after a step-down, quiet frames must NOT step up.
        for _ in 0..2000 {
            assert_eq!(gov.record_frame(2.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);

        // User override (`enabled = false`) disables all automatic transitions.
        gov.set_enabled(false);
        for _ in 0..500 {
            assert_eq!(gov.record_frame(15.5), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);
    }

    #[test]
    fn quality_governor_respects_backend_order_and_never_picks_costlier_filter() {
        // Simulate SwiftShader where AuthenticGbaLcd (`lcd-grid`) is the cheapest filter (0.41 ms),
        // Sharp1x (`none`) is slightly more expensive (0.52 ms), and XbrSmooth3x/SmoothScale2x are top.
        let sw_costs = vec![
            FilterCost {
                filter: DisplayFilter::AuthenticGbaLcd,
                cost_ms: 4.1,
            },
            FilterCost {
                filter: DisplayFilter::Sharp1x,
                cost_ms: 5.2,
            },
            FilterCost {
                filter: DisplayFilter::MyBoySmooth,
                cost_ms: 6.9,
            },
            FilterCost {
                filter: DisplayFilter::CrtScanlines,
                cost_ms: 7.2,
            },
            FilterCost {
                filter: DisplayFilter::SoftHandheld,
                cost_ms: 8.6,
            },
            FilterCost {
                filter: DisplayFilter::XbrSmooth3x,
                cost_ms: 8.7,
            },
            FilterCost {
                filter: DisplayFilter::SmoothScale2x,
                cost_ms: 11.8,
            },
        ];
        // Start at SmoothScale2x with work = 16.5 ms -> est_base = 4.7 ms.
        // Remaining budget for 50% (8.37 ms) is 3.67 ms -> even cheapest (4.1 ms) doesn't fit under 50%,
        // so governor jumps straight to costs[0] = AuthenticGbaLcd, NEVER picking Sharp1x or anything costlier.
        let mut gov =
            QualityGovernor::with_measured_costs(DisplayFilter::SmoothScale2x, true, sw_costs);
        let mut ev = None;
        for _ in 0..240 {
            if let Some(tr) = gov.record_frame(16.5) {
                ev = Some(tr);
                break;
            }
        }
        let tr = ev.expect("expected direct drop to cheapest measured filter");
        assert_eq!(tr.kind, GovernorTransitionKind::StepDown);
        assert_eq!(tr.to, DisplayFilter::AuthenticGbaLcd);

        // Already at cheapest (AuthenticGbaLcd): further overload must never pick Sharp1x or any other filter.
        for _ in 0..500 {
            assert_eq!(gov.record_frame(15.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);
    }

    #[test]
    fn quality_governor_recovery_and_doubled_lockout_on_flapping() {
        let costs = vec![
            FilterCost {
                filter: DisplayFilter::Sharp1x,
                cost_ms: 0.4,
            },
            FilterCost {
                filter: DisplayFilter::MyBoySmooth,
                cost_ms: 0.5,
            },
            FilterCost {
                filter: DisplayFilter::SoftHandheld,
                cost_ms: 0.7,
            },
            FilterCost {
                filter: DisplayFilter::CrtScanlines,
                cost_ms: 0.9,
            },
            FilterCost {
                filter: DisplayFilter::AuthenticGbaLcd,
                cost_ms: 1.2,
            },
            FilterCost {
                filter: DisplayFilter::SmoothScale2x,
                cost_ms: 5.0,
            },
            FilterCost {
                filter: DisplayFilter::XbrSmooth3x,
                cost_ms: 10.0,
            },
        ];

        // 1. Transient hiccup (<120 sustained p95 frames >70%): no step-down.
        let mut gov =
            QualityGovernor::with_measured_costs(DisplayFilter::XbrSmooth3x, true, costs.clone());
        for _ in 0..100 {
            assert_eq!(gov.record_frame(14.5), None);
        }
        for _ in 0..180 {
            assert_eq!(gov.record_frame(4.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::XbrSmooth3x);

        // 2. Sustained overload: est_base = 14.0 - 10.0 = 4.0 ms -> max affordable filter
        //    under 8.37 ms is AuthenticGbaLcd (4.0 + 5.0 = 9.0 > 8.37, so drops to AuthenticGbaLcd 1.2).
        let step_down = (0..240)
            .find_map(|_| gov.record_frame(14.0))
            .expect("sustained overload must step down");
        assert_eq!(step_down.kind, GovernorTransitionKind::StepDown);
        assert_eq!(step_down.to, DisplayFilter::AuthenticGbaLcd);
        assert_eq!(gov.current_lockout_frames(), GOVERNOR_BASE_LOCKOUT_FRAMES);
        assert_eq!(gov.up_lockout_remaining(), GOVERNOR_BASE_LOCKOUT_FRAMES);

        // 3. Recovery lockout (18,000f = 5 min): stays locked even under ultra-fast 2.0 ms frames.
        for _ in 0..GOVERNOR_BASE_LOCKOUT_FRAMES {
            assert_eq!(gov.record_frame(2.0), None);
        }
        assert_eq!(gov.up_lockout_remaining(), 0);
        // Needs continuous 60s (3,600f) below 35%; interrupt once at 2,000f to test sustain reset.
        for _ in 0..2000 {
            assert_eq!(gov.record_frame(2.0), None);
        }
        assert_eq!(gov.record_frame(6.5), None); // p95 rises above 35% only if sustained, feed burst >35%
        for _ in 0..15 {
            assert_eq!(gov.record_frame(6.5), None);
        }
        for _ in 0..1600 {
            assert_eq!(gov.record_frame(2.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);

        // Full unbroken 3,600f <35%: step up ONE step (AuthenticGbaLcd -> SmoothScale2x, not XbrSmooth3x).
        let step_up = (0..GOVERNOR_RECOVERY_SUSTAIN_FRAMES)
            .find_map(|_| gov.record_frame(2.0))
            .expect("expected single-step recovery after lockout + 60s <35%");
        assert_eq!(step_up.kind, GovernorTransitionKind::StepUp);
        assert_eq!(step_up.from, DisplayFilter::AuthenticGbaLcd);
        assert_eq!(step_up.to, DisplayFilter::SmoothScale2x);

        // 4. Flapping: overload returns within 30s (<1,800f) of step-up -> immediate step-down + DOUBLED lockout.
        //    est_base = 12.0 - 5.0 = 7.0 ms; AuthenticGbaLcd adds 1.2 ms (8.2 <= 8.37) while SmoothScale2x would be 12.0.
        let flap_down = (0..120)
            .find_map(|_| gov.record_frame(12.0))
            .expect("post-step-up overload within 30s must step back down immediately");
        assert_eq!(flap_down.kind, GovernorTransitionKind::StepDown);
        assert_eq!(flap_down.to, DisplayFilter::AuthenticGbaLcd);
        assert_eq!(
            gov.current_lockout_frames(),
            GOVERNOR_BASE_LOCKOUT_FRAMES * 2
        );
        assert_eq!(
            gov.up_lockout_remaining(),
            GOVERNOR_BASE_LOCKOUT_FRAMES * 2
        );

        // Well past old 5min + 60s window (24,000 frames), must STILL be locked out because lockout doubled to 10min!
        for _ in 0..24_000 {
            assert_eq!(gov.record_frame(2.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::AuthenticGbaLcd);
        assert!(gov.up_lockout_remaining() > 0);

        // Wait remainder of 10-min (36,000f) lockout + 60s recovery (2.0 ms -> total 6.5? no, est_base with 2.0 is 0):
        let remaining = gov.up_lockout_remaining() + GOVERNOR_RECOVERY_SUSTAIN_FRAMES + 5;
        let recovered = (0..remaining)
            .find_map(|_| gov.record_frame(2.0))
            .expect("expected recovery after doubled 10-minute lockout expired");
        assert_eq!(recovered.kind, GovernorTransitionKind::StepUp);
        assert_eq!(recovered.to, DisplayFilter::SmoothScale2x);

        // Next recovery must respect user_target ceiling and predicted <50% budget:
        // With work=5.0 ms (<35% of 16.74 = 5.86 ms), XbrSmooth3x predicts (5.0 - 5.0) + 10.0 = 10.0 ms >= 8.37 ms (50%) -> must NOT step up!
        for _ in 0..(GOVERNOR_RECOVERY_SUSTAIN_FRAMES + 200) {
            assert_eq!(gov.record_frame(5.0), None);
        }
        assert_eq!(gov.active_filter(), DisplayFilter::SmoothScale2x);

        // Also test persisted starting filter below user_target: starts settled at AuthenticGbaLcd while targeting XbrSmooth3x.
        let mut persisted_gov = QualityGovernor::with_measured_costs(
            DisplayFilter::XbrSmooth3x,
            true,
            costs,
        );
        persisted_gov.set_active_from_persisted(DisplayFilter::AuthenticGbaLcd);
        assert_eq!(persisted_gov.active_filter(), DisplayFilter::AuthenticGbaLcd);
        let recovered_from_persisted =
            (0..(GOVERNOR_WINDOW_FRAMES + GOVERNOR_RECOVERY_SUSTAIN_FRAMES))
                .find_map(|_| persisted_gov.record_frame(2.0))
                .expect("persisted filter with zero initial lockout can recover after 60s <35%");
        assert_eq!(recovered_from_persisted.to, DisplayFilter::SmoothScale2x);
    }

    #[test]
    fn video_postproc_measure_filter_costs_returns_sorted_all_filters() {
        let mut postproc = VideoPostProcessor::new();
        let costs = postproc.measure_filter_costs(false, false, false, 0.08);
        assert_eq!(costs.len(), DisplayFilter::ALL.len());
        for win in costs.windows(2) {
            assert!(
                win[0].cost_ms <= win[1].cost_ms,
                "costs not sorted ascending: {:?}",
                costs
            );
        }
    }

    #[test]
    fn quality_governor_caps_large_output_to_720x480_before_dropping_filter() {
        let large_costs = vec![
            FilterCost { filter: DisplayFilter::Sharp1x, cost_ms: 9.0 },
            FilterCost { filter: DisplayFilter::MyBoySmooth, cost_ms: 12.0 },
            FilterCost { filter: DisplayFilter::XbrSmooth3x, cost_ms: 20.0 },
        ];
        let capped_costs = vec![
            FilterCost { filter: DisplayFilter::Sharp1x, cost_ms: 0.4 },
            FilterCost { filter: DisplayFilter::MyBoySmooth, cost_ms: 1.5 },
            FilterCost { filter: DisplayFilter::XbrSmooth3x, cost_ms: 7.0 },
        ];
        let mut gov =
            QualityGovernor::with_measured_costs(DisplayFilter::MyBoySmooth, true, large_costs);
        gov.set_resolution_cap_support(true, Some(capped_costs));
        assert!(!gov.is_resolution_capped());

        // Under large window (e.g. 2560x1600 @ slowdown 8), rolling p95=15 ms triggers 720x480 cap
        // while keeping MyBoySmooth (since 15 - 12 + 1.5 = 4.5 ms <= 8.37 ms target).
        let cap_tr = (0..(GOVERNOR_WINDOW_FRAMES + GOVERNOR_DOWN_SUSTAIN_FRAMES))
            .find_map(|_| gov.record_frame(15.0))
            .expect("sustained large-window overload should cap output resolution");
        assert_eq!(cap_tr.kind, GovernorTransitionKind::CapResolution);
        assert_eq!(cap_tr.to, DisplayFilter::MyBoySmooth);
        assert!(gov.is_resolution_capped());
        assert_eq!(
            gov.max_output_size(),
            Some((GOVERNOR_MAX_CAP_WIDTH, GOVERNOR_MAX_CAP_HEIGHT))
        );

        // If user switches filter manually, user target resets cleanly without lingering cap.
        gov.set_user_target(DisplayFilter::Sharp1x);
        assert!(!gov.is_resolution_capped());
        assert_eq!(gov.max_output_size(), None);
    }
}

pub const GOVERNOR_BUDGET_MS: f64 = 1000.0 * 280_896.0 / 16_777_216.0;
pub const GOVERNOR_HIGH_PCT: f64 = 0.70;
pub const GOVERNOR_TARGET_PCT: f64 = 0.50;
pub const GOVERNOR_RECOVERY_LOW_PCT: f64 = 0.35;
pub const GOVERNOR_WINDOW_FRAMES: usize = 120;
pub const GOVERNOR_DOWN_SUSTAIN_FRAMES: usize = 120;
pub const GOVERNOR_COOLDOWN_FRAMES: usize = 180;
pub const GOVERNOR_BASE_LOCKOUT_FRAMES: usize = 18_000; // ~5 minutes at 59.73 Hz
pub const GOVERNOR_RECOVERY_SUSTAIN_FRAMES: usize = 3_600; // ~60 seconds at 59.73 Hz
pub const GOVERNOR_FLAP_WINDOW_FRAMES: usize = 1_800; // ~30 seconds at 59.73 Hz
pub const GOVERNOR_MAX_CAP_WIDTH: usize = 720;
pub const GOVERNOR_MAX_CAP_HEIGHT: usize = 480;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GovernorTransitionKind {
    StepDown,
    StepUp,
    CapResolution,
    UncapResolution,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterCost {
    pub filter: DisplayFilter,
    pub cost_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GovernorTransition {
    pub kind: GovernorTransitionKind,
    pub from: DisplayFilter,
    pub to: DisplayFilter,
    pub rolling_p95_ms: f64,
}

#[inline]
pub const fn canonical_governor_filter(filter: DisplayFilter) -> DisplayFilter {
    match filter {
        DisplayFilter::RawNearest => DisplayFilter::Sharp1x,
        other => other,
    }
}

/// Deterministic high-contrast 240x160 BGR555 test frame so edge-adaptive filters exercise real branches.
pub fn test_filter_pattern_fb() -> Box<[u16; LCD_WIDTH * LCD_HEIGHT]> {
    let mut fb = vec![0u16; LCD_WIDTH * LCD_HEIGHT].into_boxed_slice();
    for y in 0..LCD_HEIGHT {
        for x in 0..LCD_WIDTH {
            let edge = ((x / 4) ^ (y / 4)) & 1;
            let diag = if (x + y) % 9 < 3 { 31 } else { 4 };
            let r = if edge == 0 { (x & 31) as u16 } else { diag as u16 };
            let g = if edge == 0 { diag as u16 } else { (y & 31) as u16 };
            let b = (((x ^ y) >> 1) & 31) as u16;
            fb[y * LCD_WIDTH + x] = (b << 10) | (g << 5) | r;
        }
    }
    fb.try_into().unwrap()
}

pub fn sort_filter_costs(costs: &mut Vec<FilterCost>) {
    for entry in costs.iter_mut() {
        entry.filter = canonical_governor_filter(entry.filter);
        if !entry.cost_ms.is_finite() || entry.cost_ms < 0.0 {
            entry.cost_ms = 0.0;
        }
    }
    costs.sort_by(|a, b| {
        a.cost_ms
            .total_cmp(&b.cost_ms)
            .then_with(|| a.filter.quality_tier().cmp(&b.filter.quality_tier()))
    });
    let mut seen = [false; DisplayFilter::ALL.len()];
    costs.retain(|entry| {
        let Some(idx) = DisplayFilter::ALL.iter().position(|&f| f == entry.filter) else {
            return false;
        };
        if seen[idx] {
            return false;
        }
        seen[idx] = true;
        true
    });
}

/// Measured-cost quality governor with overload step-down, conservative recovery, and flap backoff.
///
/// Tracks a 120-frame (~2s) rolling `p95` work time against the 16.7427 ms GBA frame budget:
/// - When rolling `p95` exceeds 70% (`~11.72 ms`) continuously for 120 frames (~2s), jumps
///   directly to the most expensive strictly cheaper filter predicted to keep total frame time
///   under ~50% of budget (`~8.37 ms`), or the cheapest measured filter if none fits under 50%.
/// - Locks out step-ups for 5 minutes (18,000 frames) after any step-down. Once lockout expires,
///   if rolling `p95` stays below 35% (`~5.86 ms`) continuously for 60s (3,600 frames) and the
///   next costlier filter predicts total work below 50%, steps up one filter (never above `user_target`).
/// - If overload (`p95 > 70%`) returns within 30s (1,800 frames) of a step-up, steps back down
///   immediately and doubles the lockout duration (10m -> 20m -> ...).
#[derive(Clone, Debug)]
pub struct QualityGovernor {
    enabled: bool,
    user_target: DisplayFilter,
    active: DisplayFilter,
    costs: Vec<FilterCost>,
    capped_costs: Option<Vec<FilterCost>>,
    can_cap_resolution: bool,
    resolution_capped: bool,
    window: VecDeque<f64>,
    scratch: Vec<f64>,
    high_streak: usize,
    low_streak: usize,
    cooldown: usize,
    up_lockout_remaining: usize,
    current_lockout_frames: usize,
    frames_since_step_up: Option<usize>,
}

impl QualityGovernor {
    pub fn default_measured_costs() -> Vec<FilterCost> {
        let mut costs = vec![
            FilterCost {
                filter: DisplayFilter::Sharp1x,
                cost_ms: 0.08,
            },
            FilterCost {
                filter: DisplayFilter::MyBoySmooth,
                cost_ms: 0.09,
            },
            FilterCost {
                filter: DisplayFilter::SoftHandheld,
                cost_ms: 0.12,
            },
            FilterCost {
                filter: DisplayFilter::CrtScanlines,
                cost_ms: 0.13,
            },
            FilterCost {
                filter: DisplayFilter::AuthenticGbaLcd,
                cost_ms: 0.28,
            },
            FilterCost {
                filter: DisplayFilter::SmoothScale2x,
                cost_ms: 0.35,
            },
            FilterCost {
                filter: DisplayFilter::XbrSmooth3x,
                cost_ms: 0.66,
            },
        ];
        sort_filter_costs(&mut costs);
        costs
    }

    pub fn new(initial_filter: DisplayFilter, enabled: bool) -> Self {
        Self::with_measured_costs(initial_filter, enabled, Self::default_measured_costs())
    }

    pub fn with_measured_costs(
        initial_filter: DisplayFilter,
        enabled: bool,
        mut costs: Vec<FilterCost>,
    ) -> Self {
        sort_filter_costs(&mut costs);
        if costs.is_empty() {
            costs = Self::default_measured_costs();
        }
        let norm = canonical_governor_filter(initial_filter);
        Self {
            enabled,
            user_target: norm,
            active: norm,
            costs,
            capped_costs: None,
            can_cap_resolution: false,
            resolution_capped: false,
            window: VecDeque::with_capacity(GOVERNOR_WINDOW_FRAMES),
            scratch: Vec::with_capacity(GOVERNOR_WINDOW_FRAMES),
            high_streak: 0,
            low_streak: 0,
            cooldown: 0,
            up_lockout_remaining: 0,
            current_lockout_frames: GOVERNOR_BASE_LOCKOUT_FRAMES,
            frames_since_step_up: None,
        }
    }

    pub fn set_measured_costs(&mut self, mut costs: Vec<FilterCost>) {
        sort_filter_costs(&mut costs);
        if !costs.is_empty() {
            self.costs = costs;
        }
    }

    pub fn set_resolution_cap_support(
        &mut self,
        window_exceeds_cap: bool,
        mut capped_costs: Option<Vec<FilterCost>>,
    ) {
        self.can_cap_resolution = window_exceeds_cap;
        if let Some(ref mut cc) = capped_costs {
            sort_filter_costs(cc);
            if cc.is_empty() {
                self.capped_costs = None;
            } else {
                self.capped_costs = capped_costs;
            }
        } else {
            self.capped_costs = None;
        }
        if !window_exceeds_cap && self.resolution_capped {
            self.resolution_capped = false;
            self.window.clear();
            self.high_streak = 0;
            self.low_streak = 0;
        }
    }

    pub fn is_resolution_capped(&self) -> bool {
        self.can_cap_resolution && self.resolution_capped
    }

    #[cfg(test)]
    pub fn max_output_size(&self) -> Option<(usize, usize)> {
        if self.is_resolution_capped() {
            Some((GOVERNOR_MAX_CAP_WIDTH, GOVERNOR_MAX_CAP_HEIGHT))
        } else {
            None
        }
    }

    pub fn measured_costs(&self) -> &[FilterCost] {
        &self.costs
    }

    #[cfg(test)]
    pub fn up_lockout_remaining(&self) -> usize {
        self.up_lockout_remaining
    }

    #[cfg(test)]
    pub fn current_lockout_frames(&self) -> usize {
        self.current_lockout_frames
    }

    /// Initializes the active filter from a previously settled per-`(adapter, resolution)` entry
    /// while preserving `user_target` so conservative recovery can still step back up later.
    pub fn set_active_from_persisted(&mut self, settled: DisplayFilter) {
        let norm_settled = canonical_governor_filter(settled);
        let settled_idx = self.costs.iter().position(|c| c.filter == norm_settled);
        let target_idx = self.costs.iter().position(|c| c.filter == self.user_target);
        match (settled_idx, target_idx) {
            (Some(s), Some(t)) if s <= t => {
                self.active = norm_settled;
            }
            _ => {
                self.active = self.user_target;
            }
        }
        self.high_streak = 0;
        self.low_streak = 0;
        self.cooldown = 0;
        self.up_lockout_remaining = 0;
        self.frames_since_step_up = None;
        self.window.clear();
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.high_streak = 0;
            self.low_streak = 0;
        }
    }

    pub fn active_filter(&self) -> DisplayFilter {
        self.active
    }

    /// Updates the user-chosen filter target and clears transient streaks and flap backoff.
    pub fn set_user_target(&mut self, filter: DisplayFilter) {
        let norm = canonical_governor_filter(filter);
        self.user_target = norm;
        self.active = norm;
        self.resolution_capped = false;
        self.high_streak = 0;
        self.low_streak = 0;
        self.cooldown = 0;
        self.up_lockout_remaining = 0;
        self.current_lockout_frames = GOVERNOR_BASE_LOCKOUT_FRAMES;
        self.frames_since_step_up = None;
        self.window.clear();
    }

    pub fn sync_external_filter(&mut self, filter: DisplayFilter) {
        let norm = canonical_governor_filter(filter);
        if norm != canonical_governor_filter(self.active) {
            self.set_user_target(norm);
        }
    }

    fn active_cost_table(&self) -> &[FilterCost] {
        if self.is_resolution_capped()
            && let Some(ref capped) = self.capped_costs
            && !capped.is_empty()
        {
            return capped;
        }
        &self.costs
    }

    fn rolling_p95(&mut self) -> Option<f64> {
        if self.window.len() < GOVERNOR_WINDOW_FRAMES / 2 {
            return None;
        }
        self.scratch.clear();
        self.scratch.extend(self.window.iter().copied());
        self.scratch.sort_by(f64::total_cmp);
        let idx = ((self.scratch.len() - 1) * 95) / 100;
        self.scratch.get(idx).copied()
    }

    /// Selects the most expensive strictly cheaper filter predicted to keep total work time
    /// under ~50% of budget, or the cheapest measured filter when none fits under 50%.
    fn select_target_under_budget(&self, rolling_p95_ms: f64) -> Option<DisplayFilter> {
        let table = self.active_cost_table();
        let active_norm = canonical_governor_filter(self.active);
        let cur_idx = table.iter().position(|c| c.filter == active_norm)?;
        if cur_idx == 0 {
            return None;
        }
        let cur_cost_ms = table[cur_idx].cost_ms;
        let est_base_ms = (rolling_p95_ms - cur_cost_ms).max(0.0);
        let target_budget_ms = GOVERNOR_BUDGET_MS * GOVERNOR_TARGET_PCT;

        for cand in table[..cur_idx].iter().rev() {
            if cand.cost_ms <= cur_cost_ms && est_base_ms + cand.cost_ms <= target_budget_ms {
                return Some(cand.filter);
            }
        }
        // Fallback: jump straight to the cheapest measured filter (never a costlier filter).
        let cheapest = table[0];
        if cheapest.filter != active_norm && cheapest.cost_ms <= cur_cost_ms {
            Some(cheapest.filter)
        } else {
            None
        }
    }

    /// Selects the filter to run immediately upon enabling `720x480` OS-scaled cap:
    /// preserves the current active filter if it fits under 50% of budget (e.g. `MyBoySmooth`),
    /// otherwise jumps to the best filter in `capped_costs` under 50%.
    fn select_filter_when_capping(&self, rolling_p95_ms: f64) -> DisplayFilter {
        let active_norm = canonical_governor_filter(self.active);
        let uncapped_cost = self
            .costs
            .iter()
            .find(|c| c.filter == active_norm)
            .map(|c| c.cost_ms)
            .unwrap_or(0.0);
        let est_emu_base_ms = (rolling_p95_ms - uncapped_cost).max(0.0);
        let target_budget_ms = GOVERNOR_BUDGET_MS * GOVERNOR_TARGET_PCT;
        let table = self.capped_costs.as_deref().unwrap_or(&self.costs);
        if let Some(cur_capped) = table.iter().find(|c| c.filter == active_norm)
            && est_emu_base_ms + cur_capped.cost_ms <= target_budget_ms
        {
            return active_norm;
        }
        // Check if active_norm is still <= target or pick highest filter <= cur tier fitting budget.
        let active_tier_pos = table
            .iter()
            .position(|c| c.filter == active_norm)
            .unwrap_or(table.len().saturating_sub(1));
        for cand in table[..=active_tier_pos].iter().rev() {
            if est_emu_base_ms + cand.cost_ms <= target_budget_ms {
                return cand.filter;
            }
        }
        active_norm
    }

    /// Selects the next costlier filter one tier above `active` (capped at `user_target`)
    /// only when its measured cost predicts total frame work below ~50% of budget.
    fn select_next_step_up(&self, rolling_p95_ms: f64) -> Option<DisplayFilter> {
        let table = self.active_cost_table();
        let active_norm = canonical_governor_filter(self.active);
        let target_norm = canonical_governor_filter(self.user_target);
        if active_norm == target_norm {
            return None;
        }
        let cur_idx = table.iter().position(|c| c.filter == active_norm)?;
        let target_idx = table.iter().position(|c| c.filter == target_norm)?;
        if cur_idx >= target_idx || cur_idx + 1 >= table.len() {
            return None;
        }
        let cur_cost_ms = table[cur_idx].cost_ms;
        let next = table[cur_idx + 1];
        let est_base_ms = (rolling_p95_ms - cur_cost_ms).max(0.0);
        let target_budget_ms = GOVERNOR_BUDGET_MS * GOVERNOR_TARGET_PCT;
        if est_base_ms + next.cost_ms < target_budget_ms {
            Some(next.filter)
        } else {
            None
        }
    }

    /// Feeds one completed frame's work time (`ms`) and returns any filter or resolution-cap transition.
    pub fn record_frame(&mut self, work_ms: f64) -> Option<GovernorTransition> {
        if !self.enabled {
            return None;
        }
        if self.window.len() == GOVERNOR_WINDOW_FRAMES {
            self.window.pop_front();
        }
        self.window.push_back(work_ms.max(0.0));

        if let Some(frames_up) = self.frames_since_step_up {
            let next_frames = frames_up.saturating_add(1);
            if next_frames > GOVERNOR_FLAP_WINDOW_FRAMES {
                self.frames_since_step_up = None;
                self.current_lockout_frames = GOVERNOR_BASE_LOCKOUT_FRAMES;
            } else {
                self.frames_since_step_up = Some(next_frames);
            }
        }

        if self.up_lockout_remaining > 0 {
            self.up_lockout_remaining -= 1;
        }

        if self.frames_since_step_up.is_none() && self.cooldown > 0 {
            self.cooldown -= 1;
            return None;
        }

        let p95 = self.rolling_p95()?;
        let high_thresh = GOVERNOR_BUDGET_MS * GOVERNOR_HIGH_PCT;
        let low_thresh = GOVERNOR_BUDGET_MS * GOVERNOR_RECOVERY_LOW_PCT;

        if p95 > high_thresh {
            self.low_streak = 0;
            if work_ms > high_thresh {
                self.high_streak += 1;
            } else {
                // Current frame is below 70%: do not count recovering frames toward overload.
                self.high_streak = self.high_streak.saturating_sub(1);
            }
            let in_flap_probation = self.frames_since_step_up.is_some();
            let sustain_met = in_flap_probation || self.high_streak >= GOVERNOR_DOWN_SUSTAIN_FRAMES;
            if sustain_met {
                if self.can_cap_resolution && !self.resolution_capped {
                    let prev = self.active;
                    let next = self.select_filter_when_capping(p95);
                    self.resolution_capped = true;
                    self.active = next;
                    self.high_streak = 0;
                    self.low_streak = 0;
                    self.cooldown = GOVERNOR_COOLDOWN_FRAMES;
                    let next_lockout = if in_flap_probation {
                        self.current_lockout_frames
                            .saturating_mul(2)
                            .max(GOVERNOR_BASE_LOCKOUT_FRAMES * 2)
                    } else {
                        GOVERNOR_BASE_LOCKOUT_FRAMES
                    };
                    self.current_lockout_frames = next_lockout;
                    self.up_lockout_remaining = next_lockout;
                    self.frames_since_step_up = None;
                    self.window.clear();
                    return Some(GovernorTransition {
                        kind: GovernorTransitionKind::CapResolution,
                        from: prev,
                        to: next,
                        rolling_p95_ms: p95,
                    });
                }
                if let Some(next) = self.select_target_under_budget(p95) {
                    let prev = self.active;
                    self.active = next;
                    self.high_streak = 0;
                    self.low_streak = 0;
                    self.cooldown = GOVERNOR_COOLDOWN_FRAMES;
                    let next_lockout = if in_flap_probation {
                        self.current_lockout_frames
                            .saturating_mul(2)
                            .max(GOVERNOR_BASE_LOCKOUT_FRAMES * 2)
                    } else {
                        GOVERNOR_BASE_LOCKOUT_FRAMES
                    };
                    self.current_lockout_frames = next_lockout;
                    self.up_lockout_remaining = next_lockout;
                    self.frames_since_step_up = None;
                    self.window.clear();
                    return Some(GovernorTransition {
                        kind: GovernorTransitionKind::StepDown,
                        from: prev,
                        to: next,
                        rolling_p95_ms: p95,
                    });
                }
            }
            return None;
        }

        self.high_streak = 0;
        if self.up_lockout_remaining == 0 && p95 < low_thresh {
            if let Some(next_up) = self.select_next_step_up(p95) {
                self.low_streak += 1;
                if self.low_streak >= GOVERNOR_RECOVERY_SUSTAIN_FRAMES {
                    let prev = self.active;
                    self.active = next_up;
                    self.low_streak = 0;
                    self.high_streak = 0;
                    self.cooldown = 0;
                    self.frames_since_step_up = Some(0);
                    self.window.clear();
                    return Some(GovernorTransition {
                        kind: GovernorTransitionKind::StepUp,
                        from: prev,
                        to: next_up,
                        rolling_p95_ms: p95,
                    });
                }
            } else if self.is_resolution_capped() {
                let active_norm = canonical_governor_filter(self.active);
                let capped_cost = self
                    .active_cost_table()
                    .iter()
                    .find(|c| c.filter == active_norm)
                    .map(|c| c.cost_ms)
                    .unwrap_or(0.0);
                let uncapped_cost = self
                    .costs
                    .iter()
                    .find(|c| c.filter == active_norm)
                    .map(|c| c.cost_ms)
                    .unwrap_or(capped_cost);
                let est_emu_base = (p95 - capped_cost).max(0.0);
                let target_budget_ms = GOVERNOR_BUDGET_MS * GOVERNOR_TARGET_PCT;
                if est_emu_base + uncapped_cost < target_budget_ms {
                    self.low_streak += 1;
                    if self.low_streak >= GOVERNOR_RECOVERY_SUSTAIN_FRAMES {
                        self.resolution_capped = false;
                        self.low_streak = 0;
                        self.high_streak = 0;
                        self.cooldown = 0;
                        self.frames_since_step_up = Some(0);
                        self.window.clear();
                        return Some(GovernorTransition {
                            kind: GovernorTransitionKind::UncapResolution,
                            from: active_norm,
                            to: active_norm,
                            rolling_p95_ms: p95,
                        });
                    }
                } else {
                    self.low_streak = 0;
                }
            } else {
                self.low_streak = 0;
            }
        } else {
            self.low_streak = 0;
        }

        None
    }
}
