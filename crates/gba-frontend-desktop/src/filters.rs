use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::ColorImage;
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::config::{AppConfig, ColorProfileChoice};

static LINEAR_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGB_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS101_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS001_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();

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
        }
    }

    pub fn reset(&mut self) {
        self.prev_rgba.fill(0);
        self.curr_rgba.fill(0);
        self.prev_bgr555.fill(0);
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
}
