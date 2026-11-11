use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::ColorImage;
use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::config::AppConfig;

static LINEAR_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGB_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();

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

pub struct VideoPostProcessor {
    pub prev_rgba: Vec<u8>,
    pub curr_rgba: Vec<u8>,
    pub grid_rgba: Vec<u8>,
}

impl VideoPostProcessor {
    pub fn new() -> Self {
        Self {
            prev_rgba: vec![0; LCD_WIDTH * LCD_HEIGHT * 4],
            curr_rgba: vec![0; LCD_WIDTH * LCD_HEIGHT * 4],
            grid_rgba: vec![0; (LCD_WIDTH * 3) * (LCD_HEIGHT * 3) * 4],
        }
    }

    pub fn process_frame(
        &mut self,
        fb_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
        color_correction: bool,
        lcd_ghosting: bool,
        scanline_grid: bool,
    ) -> ColorImage {
        let lut = if color_correction {
            agb_srgb_lut()
        } else {
            linear_lut()
        };

        for (idx, &px) in fb_bgr555.iter().enumerate() {
            let off = idx * 4;
            let mapped = lut[(px & 0x7FFF) as usize];
            if lcd_ghosting {
                // Blend ~65% new frame + ~35% previous frame for authentic TFT persistence.
                let r = ((mapped[0] as u16 * 166 + self.prev_rgba[off] as u16 * 90) >> 8) as u8;
                let g = ((mapped[1] as u16 * 166 + self.prev_rgba[off + 1] as u16 * 90) >> 8) as u8;
                let b = ((mapped[2] as u16 * 166 + self.prev_rgba[off + 2] as u16 * 90) >> 8) as u8;
                self.curr_rgba[off] = r;
                self.curr_rgba[off + 1] = g;
                self.curr_rgba[off + 2] = b;
                self.curr_rgba[off + 3] = 255;
            } else {
                self.curr_rgba[off..off + 4].copy_from_slice(&mapped);
            }
        }
        self.prev_rgba.copy_from_slice(&self.curr_rgba);

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
