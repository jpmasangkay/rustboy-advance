//! Pixel format conversion (`BGR555 -> RGB565` / `0RGB1555`) and LCD color correction LUTs.

use std::sync::OnceLock;

use gba_core::{LCD_HEIGHT, LCD_WIDTH};

use crate::ffi::{RETRO_PIXEL_FORMAT_0RGB1555, RETRO_PIXEL_FORMAT_RGB565, retro_pixel_format};

/// Libretro pixel format selected via `RETRO_ENVIRONMENT_SET_PIXEL_FORMAT`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum OutputPixelFormat {
    #[default]
    Rgb565,
    Xrgb1555,
}

impl OutputPixelFormat {
    pub const fn as_retro(self) -> retro_pixel_format {
        match self {
            Self::Rgb565 => RETRO_PIXEL_FORMAT_RGB565,
            Self::Xrgb1555 => RETRO_PIXEL_FORMAT_0RGB1555,
        }
    }

    pub const fn bytes_per_pixel(self) -> usize {
        2
    }

    pub const fn pitch_bytes(self) -> usize {
        self.pitch_bytes_for_width(LCD_WIDTH)
    }

    pub const fn pitch_bytes_for_width(self, width: usize) -> usize {
        width * self.bytes_per_pixel()
    }
}

/// LCD color correction profile exposed via core options.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ColorCorrection {
    #[default]
    Off,
    Gba,
    GbaSp,
}

impl ColorCorrection {
    pub fn from_option_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "gba" | "agb" | "agb-001" => Self::Gba,
            "gba sp" | "gba_sp" | "ags-101" | "sp" => Self::GbaSp,
            _ => Self::Off,
        }
    }
}

static LUT_RGB565_OFF: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();
static LUT_RGB565_GBA: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();
static LUT_RGB565_SP: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();
static LUT_XRGB1555_OFF: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();
static LUT_XRGB1555_GBA: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();
static LUT_XRGB1555_SP: OnceLock<Box<[u16; 32_768]>> = OnceLock::new();

#[inline]
fn color_correct_rgb8(px_bgr555: usize, cc: ColorCorrection) -> (u8, u8, u8) {
    let r5 = (px_bgr555 & 0x1F) as u8;
    let g5 = ((px_bgr555 >> 5) & 0x1F) as u8;
    let b5 = ((px_bgr555 >> 10) & 0x1F) as u8;

    match cc {
        ColorCorrection::Off => (
            (r5 << 3) | (r5 >> 2),
            (g5 << 3) | (g5 >> 2),
            (b5 << 3) | (b5 >> 2),
        ),
        ColorCorrection::Gba => {
            // AGB-001 unbacklit reflective TFT -> modern sRGB colorimetric matrix.
            let r = (r5 as f32) / 31.0;
            let g = (g5 as f32) / 31.0;
            let b = (b5 as f32) / 31.0;

            let lr = r.powf(2.4);
            let lg = g.powf(2.4);
            let lb = b.powf(2.4);

            let out_r = (255.0 * (lr * 0.845 + lg * 0.125 + lb * 0.030).powf(1.0 / 2.2))
                .clamp(0.0, 255.0)
                .round() as u8;
            let out_g = (255.0 * (lr * 0.055 + lg * 0.675 + lb * 0.270).powf(1.0 / 2.2))
                .clamp(0.0, 255.0)
                .round() as u8;
            let out_b = (255.0 * (lr * 0.000 + lg * 0.195 + lb * 0.805).powf(1.0 / 2.2))
                .clamp(0.0, 255.0)
                .round() as u8;

            (out_r, out_g, out_b)
        }
        ColorCorrection::GbaSp => {
            // AGS-101 backlit vibrant TFT curve.
            let r = (r5 as f32) / 31.0;
            let g = (g5 as f32) / 31.0;
            let b = (b5 as f32) / 31.0;

            let out_r = (255.0 * (r * 0.94 + g * 0.04 + b * 0.02).powf(1.15))
                .clamp(0.0, 255.0)
                .round() as u8;
            let out_g = (255.0 * (r * 0.03 + g * 0.92 + b * 0.05).powf(1.15))
                .clamp(0.0, 255.0)
                .round() as u8;
            let out_b = (255.0 * (r * 0.01 + g * 0.07 + b * 0.92).powf(1.15))
                .clamp(0.0, 255.0)
                .round() as u8;

            (out_r, out_g, out_b)
        }
    }
}

fn build_lut(fmt: OutputPixelFormat, cc: ColorCorrection) -> Box<[u16; 32_768]> {
    let mut table = vec![0u16; 32_768].into_boxed_slice();
    for (c, slot) in table.iter_mut().enumerate() {
        let (r8, g8, b8) = color_correct_rgb8(c, cc);
        *slot = match fmt {
            OutputPixelFormat::Rgb565 => {
                let r5 = (r8 as u16) >> 3;
                let g6 = (g8 as u16) >> 2;
                let b5 = (b8 as u16) >> 3;
                (r5 << 11) | (g6 << 5) | b5
            }
            OutputPixelFormat::Xrgb1555 => {
                let r5 = (r8 as u16) >> 3;
                let g5 = (g8 as u16) >> 3;
                let b5 = (b8 as u16) >> 3;
                (r5 << 10) | (g5 << 5) | b5
            }
        };
    }
    table.try_into().expect("32768-entry pixel LUT")
}

/// Returns the cached 32,768-entry `BGR555` -> output LUT for `(fmt, cc)`.
pub fn pixel_lut(fmt: OutputPixelFormat, cc: ColorCorrection) -> &'static [u16; 32_768] {
    match (fmt, cc) {
        (OutputPixelFormat::Rgb565, ColorCorrection::Off) => {
            LUT_RGB565_OFF.get_or_init(|| build_lut(fmt, cc))
        }
        (OutputPixelFormat::Rgb565, ColorCorrection::Gba) => {
            LUT_RGB565_GBA.get_or_init(|| build_lut(fmt, cc))
        }
        (OutputPixelFormat::Rgb565, ColorCorrection::GbaSp) => {
            LUT_RGB565_SP.get_or_init(|| build_lut(fmt, cc))
        }
        (OutputPixelFormat::Xrgb1555, ColorCorrection::Off) => {
            LUT_XRGB1555_OFF.get_or_init(|| build_lut(fmt, cc))
        }
        (OutputPixelFormat::Xrgb1555, ColorCorrection::Gba) => {
            LUT_XRGB1555_GBA.get_or_init(|| build_lut(fmt, cc))
        }
        (OutputPixelFormat::Xrgb1555, ColorCorrection::GbaSp) => {
            LUT_XRGB1555_SP.get_or_init(|| build_lut(fmt, cc))
        }
    }
}

/// Converts a slice of `BGR555` pixels into `RGB565` or `0RGB1555` via a 32K LUT.
pub fn convert_pixels(
    src_bgr555: &[u16],
    dst: &mut [u16],
    fmt: OutputPixelFormat,
    cc: ColorCorrection,
) {
    let lut = pixel_lut(fmt, cc);
    for (dst_px, &src_px) in dst.iter_mut().zip(src_bgr555.iter()) {
        *dst_px = lut[(src_px & 0x7FFF) as usize];
    }
}

/// Converts a 240x160 GBA `BGR555` framebuffer into `RGB565` or `0RGB1555` via a 32K LUT.
pub fn convert_framebuffer(
    src_bgr555: &[u16; LCD_WIDTH * LCD_HEIGHT],
    dst: &mut [u16; LCD_WIDTH * LCD_HEIGHT],
    fmt: OutputPixelFormat,
    cc: ColorCorrection,
) {
    convert_pixels(src_bgr555, dst, fmt, cc);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lut_uncorrected_primaries_match_rgb565_and_0rgb1555() {
        let lut565 = pixel_lut(OutputPixelFormat::Rgb565, ColorCorrection::Off);
        assert_eq!(lut565[0x0000], 0x0000);
        assert_eq!(lut565[0x001F], 0xF800); // Pure Red
        assert_eq!(lut565[0x03E0], 0x07E0); // Pure Green
        assert_eq!(lut565[0x7C00], 0x001F); // Pure Blue
        assert_eq!(lut565[0x7FFF], 0xFFFF); // Pure White

        let lut1555 = pixel_lut(OutputPixelFormat::Xrgb1555, ColorCorrection::Off);
        assert_eq!(lut1555[0x0000], 0x0000);
        assert_eq!(lut1555[0x001F], 0x7C00);
        assert_eq!(lut1555[0x03E0], 0x03E0);
        assert_eq!(lut1555[0x7C00], 0x001F);
        assert_eq!(lut1555[0x7FFF], 0x7FFF);
    }

    #[test]
    fn color_correction_profiles_produce_distinct_mappings() {
        let off = pixel_lut(OutputPixelFormat::Rgb565, ColorCorrection::Off);
        let gba = pixel_lut(OutputPixelFormat::Rgb565, ColorCorrection::Gba);
        let sp = pixel_lut(OutputPixelFormat::Rgb565, ColorCorrection::GbaSp);

        // Mid-tone red-green differs across Off / GBA / GBA SP profiles.
        let sample = 0x0210;
        assert_ne!(off[sample], gba[sample]);
        assert_ne!(gba[sample], sp[sample]);
    }
}
