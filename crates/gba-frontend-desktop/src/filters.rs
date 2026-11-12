use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

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

            let out_r = (255.0 * (lr * 0.845 + lg * 0.125 + lb * 0.030).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);
            let out_g = (255.0 * (lr * 0.055 + lg * 0.675 + lb * 0.270).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);
            let out_b = (255.0 * (lr * 0.000 + lg * 0.195 + lb * 0.805).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);

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

            let out_r = (255.0 * (0.04 + 0.82 * (r * 0.84 + g * 0.12 + b * 0.04))).clamp(0.0, 255.0);
            let out_g = (255.0 * (0.05 + 0.82 * (r * 0.06 + g * 0.82 + b * 0.12))).clamp(0.0, 255.0);
            let out_b = (255.0 * (0.08 + 0.84 * (r * 0.04 + g * 0.10 + b * 0.86))).clamp(0.0, 255.0);

            table[c] = [out_r as u8, out_g as u8, out_b as u8, 255];
        }
        table.try_into().expect("32768-entry LUT")
    })
}

#[inline]
pub fn lut_for_profile(color_correction: bool, profile: ColorProfileChoice) -> &'static [[u8; 4]; 32_768] {
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
            output_image: egui::ColorImage::new([LCD_WIDTH * 2, LCD_HEIGHT * 2], egui::Color32::BLACK),
            filter: DisplayFilter::SoftHandheld,
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
                let g = ((blended[1] as u16 * 166 + self.prev_rgba[off + 1] as u16 * 90) >> 8) as u8;
                let b = ((blended[2] as u16 * 166 + self.prev_rgba[off + 2] as u16 * 90) >> 8) as u8;
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
        std::fs::create_dir_all(&dir).map_err(|e| format!("Failed creating screenshots dir: {e}"))?;
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
    ) -> &ColorImage {
        self.filter = filter;
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
            DisplayFilter::RawNearest => {
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

            DisplayFilter::AuthenticGbaLcd => {
                let out_w = LCD_WIDTH * 3;
                let out_h = LCD_HEIGHT * 3;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let r0 = (y * 3) * out_w;
                    let r1 = r0 + out_w;
                    let r2 = r1 + out_w;

                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c_left = egui::Color32::from_rgb(
                            r,
                            ((g as u16 * 238) >> 8) as u8,
                            ((b as u16 * 230) >> 8) as u8,
                        );
                        let c_mid = egui::Color32::from_rgb(r, g, b);
                        let c_right = egui::Color32::from_rgb(
                            ((r as u16 * 215) >> 8) as u8,
                            ((g as u16 * 218) >> 8) as u8,
                            ((b as u16 * 228) >> 8) as u8,
                        );
                        let c_scan = egui::Color32::from_rgb(
                            ((r as u16 * 198) >> 8) as u8,
                            ((g as u16 * 198) >> 8) as u8,
                            ((b as u16 * 204) >> 8) as u8,
                        );
                        let c_corner = egui::Color32::from_rgb(
                            ((r as u16 * 175) >> 8) as u8,
                            ((g as u16 * 175) >> 8) as u8,
                            ((b as u16 * 180) >> 8) as u8,
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
}

pub struct FrontendBridge {
    // Precomputed 15-bit BGR555 -> 24-bit RGB Lookup Tables (0.02 ms lookup!)
    pub srgb_lut: Box<[[u8; 3]; 32768]>,
    pub raw_lut: Box<[[u8; 3]; 32768]>,

    pub base_rgb: Vec<[u8; 3]>,
    pub prev_rgb: Vec<[u8; 3]>,
    pub output_image: egui::ColorImage,

    pub filter: DisplayFilter,
    pub color_correction: bool,
    pub lcd_ghosting: bool,
    pub green_swap: bool,

    pub keypad_mask: u16,
    pub audio_prod: ringbuf::HeapProd<i16>,
    pub audio_muted: bool,
    pub volume: f32,
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
            output_image: egui::ColorImage::new([LCD_WIDTH * 3, LCD_HEIGHT * 3], egui::Color32::BLACK),
            filter: DisplayFilter::AuthenticGbaLcd,
            color_correction: true,
            lcd_ghosting: true,
            green_swap: false,
            keypad_mask: 0x03FF,
            audio_prod,
            audio_muted: false,
            volume: 0.85,
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
            DisplayFilter::RawNearest => {
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

            DisplayFilter::AuthenticGbaLcd => {
                let out_w = LCD_WIDTH * 3;
                let out_h = LCD_HEIGHT * 3;
                if self.output_image.size != [out_w, out_h] {
                    self.output_image = egui::ColorImage::new([out_w, out_h], egui::Color32::BLACK);
                }
                for y in 0..LCD_HEIGHT {
                    let src_row = y * LCD_WIDTH;
                    let r0 = (y * 3) * out_w;
                    let r1 = r0 + out_w;
                    let r2 = r1 + out_w;

                    for x in 0..LCD_WIDTH {
                        let [r, g, b] = self.base_rgb[src_row + x];
                        let c_left = egui::Color32::from_rgb(
                            r,
                            ((g as u16 * 238) >> 8) as u8,
                            ((b as u16 * 230) >> 8) as u8,
                        );
                        let c_mid = egui::Color32::from_rgb(r, g, b);
                        let c_right = egui::Color32::from_rgb(
                            ((r as u16 * 215) >> 8) as u8,
                            ((g as u16 * 218) >> 8) as u8,
                            ((b as u16 * 228) >> 8) as u8,
                        );
                        let c_scan = egui::Color32::from_rgb(
                            ((r as u16 * 198) >> 8) as u8,
                            ((g as u16 * 198) >> 8) as u8,
                            ((b as u16 * 204) >> 8) as u8,
                        );
                        let c_corner = egui::Color32::from_rgb(
                            ((r as u16 * 175) >> 8) as u8,
                            ((g as u16 * 175) >> 8) as u8,
                            ((b as u16 * 180) >> 8) as u8,
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
        use ringbuf::traits::Producer;
        if self.audio_muted {
            return;
        }
        for &s in samples_interleaved_i16 {
            let scaled = ((s as f32) * self.volume).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            let _ = self.audio_prod.try_push(scaled);
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
        assert_eq!(img1.pixels[0], egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255));

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
        assert_eq!(img2.pixels[0], egui::Color32::from_rgba_unmultiplied(127, 127, 127, 255));

        // Third frame (white) blended with previous raw frame (black) via internal history
        // still produces exact 50/50 mix (no exponential flicker decay).
        proc.frame_blending = true;
        let img3 = proc.process_frame(&white_fb, false, ColorProfileChoice::Linear, false, false);
        assert_eq!(img3.pixels[0], egui::Color32::from_rgba_unmultiplied(127, 127, 127, 255));

        // Disabling frame_blending restores full crisp white.
        proc.frame_blending = false;
        let img4 = proc.process_frame(&white_fb, false, ColorProfileChoice::Linear, false, false);
        assert_eq!(img4.pixels[0], egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255));
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
        assert_eq!(img.pixels[0], egui::Color32::from_rgba_unmultiplied(127, 127, 0, 255));
    }

    #[test]
    fn display_filter_modes_and_frontend_bridge_pipeline() {
        use ringbuf::traits::Split;
        use ringbuf::HeapRb;

        let rb = HeapRb::<i16>::new(1024);
        let (prod, _) = rb.split();
        let mut bridge = FrontendBridge::new(prod);

        let test_fb = [0x7FFF_u16; LCD_WIDTH * LCD_HEIGHT];

        // 1. Authentic GBA LCD (3x -> 720x480)
        bridge.filter = DisplayFilter::AuthenticGbaLcd;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 3, LCD_HEIGHT * 3]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::LINEAR);

        // 2. Soft Handheld (2x -> 480x320)
        bridge.filter = DisplayFilter::SoftHandheld;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 2, LCD_HEIGHT * 2]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::LINEAR);

        // 3. Smooth Scale2x (2x -> 480x320)
        bridge.filter = DisplayFilter::SmoothScale2x;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH * 2, LCD_HEIGHT * 2]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::NEAREST);

        // 4. Raw Nearest (1x -> 240x160)
        bridge.filter = DisplayFilter::RawNearest;
        bridge.present_frame(&test_fb);
        assert_eq!(bridge.output_image.size, [LCD_WIDTH, LCD_HEIGHT]);
        assert_eq!(bridge.texture_options(), egui::TextureOptions::NEAREST);

        // Also test VideoPostProcessor::process_display_filter directly
        let mut proc = VideoPostProcessor::new();
        for &f in &DisplayFilter::ALL {
            let img = proc.process_display_filter(&test_fb, true, true, f, false);
            let (exp_w, exp_h) = f.target_dimensions();
            assert_eq!(img.size, [exp_w, exp_h]);
        }
    }
}
