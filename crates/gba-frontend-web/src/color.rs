use std::sync::OnceLock;

/// GBA display color correction profile.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ColorProfile {
    /// Uncorrected 5-bit to 8-bit bit-replicated RGB.
    #[default]
    Linear = 0,
    /// Original unbacklit reflective AGB-001 TFT colorimetric response.
    Agb001 = 1,
    /// Backlit GBA SP AGS-101 vibrant TFT response.
    Ags101 = 2,
    /// Frontlit GBA SP AGS-001 pastel TFT response.
    Ags001 = 3,
}

impl ColorProfile {
    /// Converts a numeric ID into a [`ColorProfile`], falling back to `Linear`.
    pub const fn from_u8(id: u8) -> Self {
        match id {
            1 => Self::Agb001,
            2 => Self::Ags101,
            3 => Self::Ags001,
            _ => Self::Linear,
        }
    }

    /// Returns the 32,768-entry BGR555 -> RGBA8888 lookup table for this profile.
    #[inline]
    pub fn lut(self) -> &'static [[u8; 4]; 32_768] {
        match self {
            Self::Linear => linear_lut(),
            Self::Agb001 => agb_srgb_lut(),
            Self::Ags101 => ags_101_lut(),
            Self::Ags001 => ags_001_lut(),
        }
    }
}

static LINEAR_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGB_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS101_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();
static AGS001_LUT: OnceLock<Box<[[u8; 4]; 32_768]>> = OnceLock::new();

fn build_lut(mut map_fn: impl FnMut(f32, f32, f32) -> [u8; 4]) -> Box<[[u8; 4]; 32_768]> {
    let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
    for (c, slot) in table.iter_mut().enumerate() {
        let r = ((c & 0x1F) as f32) / 31.0;
        let g = (((c >> 5) & 0x1F) as f32) / 31.0;
        let b = (((c >> 10) & 0x1F) as f32) / 31.0;
        *slot = map_fn(r, g, b);
    }
    table.try_into().expect("32768-entry RGBA LUT")
}

/// Returns the bit-replicated linear BGR555 -> RGBA8888 lookup table.
#[inline]
pub fn linear_lut() -> &'static [[u8; 4]; 32_768] {
    LINEAR_LUT.get_or_init(|| {
        let mut table = vec![[0u8; 4]; 32_768].into_boxed_slice();
        for (c, slot) in table.iter_mut().enumerate() {
            let r5 = (c & 0x1F) as u8;
            let g5 = ((c >> 5) & 0x1F) as u8;
            let b5 = ((c >> 10) & 0x1F) as u8;
            *slot = [
                (r5 << 3) | (r5 >> 2),
                (g5 << 3) | (g5 >> 2),
                (b5 << 3) | (b5 >> 2),
                255,
            ];
        }
        table.try_into().expect("32768-entry RGBA LUT")
    })
}

/// Returns the AGB-001 reflective TFT -> sRGB colorimetric lookup table.
#[inline]
pub fn agb_srgb_lut() -> &'static [[u8; 4]; 32_768] {
    AGB_LUT.get_or_init(|| {
        build_lut(|r, g, b| {
            let lr = r.powf(2.4);
            let lg = g.powf(2.4);
            let lb = b.powf(2.4);

            let out_r = (255.0 * (lr * 0.845 + lg * 0.125 + lb * 0.030).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);
            let out_g = (255.0 * (lr * 0.055 + lg * 0.675 + lb * 0.270).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);
            let out_b = (255.0 * (lr * 0.000 + lg * 0.195 + lb * 0.805).powf(1.0 / 2.2))
                .clamp(0.0, 255.0);

            [out_r as u8, out_g as u8, out_b as u8, 255]
        })
    })
}

/// Returns the AGS-101 backlit TFT lookup table.
#[inline]
pub fn ags_101_lut() -> &'static [[u8; 4]; 32_768] {
    AGS101_LUT.get_or_init(|| {
        build_lut(|r, g, b| {
            let out_r = (255.0 * (r * 0.94 + g * 0.04 + b * 0.02).powf(1.15)).clamp(0.0, 255.0);
            let out_g = (255.0 * (r * 0.03 + g * 0.92 + b * 0.05).powf(1.15)).clamp(0.0, 255.0);
            let out_b = (255.0 * (r * 0.01 + g * 0.07 + b * 0.92).powf(1.15)).clamp(0.0, 255.0);
            [out_r as u8, out_g as u8, out_b as u8, 255]
        })
    })
}

/// Returns the AGS-001 frontlit TFT lookup table.
#[inline]
pub fn ags_001_lut() -> &'static [[u8; 4]; 32_768] {
    AGS001_LUT.get_or_init(|| {
        build_lut(|r, g, b| {
            let out_r =
                (255.0 * (0.04 + 0.82 * (r * 0.84 + g * 0.12 + b * 0.04))).clamp(0.0, 255.0);
            let out_g =
                (255.0 * (0.05 + 0.82 * (r * 0.06 + g * 0.82 + b * 0.12))).clamp(0.0, 255.0);
            let out_b =
                (255.0 * (0.08 + 0.84 * (r * 0.04 + g * 0.10 + b * 0.86))).clamp(0.0, 255.0);
            [out_r as u8, out_g as u8, out_b as u8, 255]
        })
    })
}
