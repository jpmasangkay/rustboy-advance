use std::sync::Arc;

pub(crate) const APP_ICON_SIZE: u32 = 64;

/// Generates a 64x64 RGBA handheld-inspired Obsidian & Teal application icon.
pub(crate) fn build_app_icon_rgba() -> Vec<u8> {
    let size = APP_ICON_SIZE as usize;
    let mut rgba = vec![0u8; size * size * 4];

    let set_px = |buf: &mut [u8], x: usize, y: usize, color: [u8; 4]| {
        if x < size && y < size {
            let idx = (y * size + x) * 4;
            buf[idx..idx + 4].copy_from_slice(&color);
        }
    };

    let fill_rounded_rect =
        |buf: &mut [u8], x0: i32, y0: i32, x1: i32, y1: i32, radius: i32, color: [u8; 4]| {
            let r2 = radius * radius;
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let dx = if x < x0 + radius {
                        x0 + radius - x
                    } else if x > x1 - radius {
                        x - (x1 - radius)
                    } else {
                        0
                    };
                    let dy = if y < y0 + radius {
                        y0 + radius - y
                    } else if y > y1 - radius {
                        y - (y1 - radius)
                    } else {
                        0
                    };
                    if dx * dx + dy * dy <= r2 && x >= 0 && y >= 0 {
                        set_px(buf, x as usize, y as usize, color);
                    }
                }
            }
        };

    // Outer Obsidian chassis + subtle border
    fill_rounded_rect(&mut rgba, 3, 12, 60, 51, 8, [0x2E, 0x2E, 0x32, 0xFF]);
    fill_rounded_rect(&mut rgba, 4, 13, 59, 50, 7, [0x18, 0x18, 0x1B, 0xFF]);

    // Shoulder trigger accents
    fill_rounded_rect(&mut rgba, 8, 10, 19, 13, 2, [0x2E, 0xC4, 0xB6, 0xFF]);
    fill_rounded_rect(&mut rgba, 44, 10, 55, 13, 2, [0x2E, 0xC4, 0xB6, 0xFF]);

    // Recessed screen lens
    fill_rounded_rect(&mut rgba, 17, 17, 46, 44, 3, [0x0D, 0x0D, 0x10, 0xFF]);
    fill_rounded_rect(&mut rgba, 19, 19, 44, 41, 2, [0x15, 0x3D, 0x3A, 0xFF]);
    // Active pixel screen frame + stylized "R" mark inside LCD
    for x in 19..=44 {
        set_px(&mut rgba, x, 19, [0x2E, 0xC4, 0xB6, 0xFF]);
    }
    fill_rounded_rect(&mut rgba, 26, 24, 37, 36, 1, [0x2E, 0xC4, 0xB6, 0xFF]);
    fill_rounded_rect(&mut rgba, 29, 27, 34, 29, 0, [0x15, 0x3D, 0x3A, 0xFF]);
    fill_rounded_rect(&mut rgba, 29, 32, 33, 36, 0, [0x15, 0x3D, 0x3A, 0xFF]);
    set_px(&mut rgba, 37, 32, [0x15, 0x3D, 0x3A, 0xFF]);

    // Left D-Pad silhouette
    fill_rounded_rect(&mut rgba, 9, 24, 12, 34, 1, [0xD4, 0xD4, 0xD8, 0xFF]);
    fill_rounded_rect(&mut rgba, 6, 27, 15, 31, 1, [0xD4, 0xD4, 0xD8, 0xFF]);

    // Right A / B buttons
    fill_rounded_rect(&mut rgba, 52, 24, 56, 28, 2, [0x2E, 0xC4, 0xB6, 0xFF]);
    fill_rounded_rect(&mut rgba, 48, 29, 52, 33, 2, [0xD4, 0xD4, 0xD8, 0xFF]);

    // Power LED dot
    fill_rounded_rect(&mut rgba, 49, 18, 51, 20, 1, [0x2E, 0xC4, 0xB6, 0xFF]);

    rgba
}

/// Returns the shared `egui::IconData` attached to desktop viewports.
pub(crate) fn app_window_icon() -> Arc<egui::IconData> {
    Arc::new(egui::IconData {
        rgba: build_app_icon_rgba(),
        width: APP_ICON_SIZE,
        height: APP_ICON_SIZE,
    })
}

#[cfg(test)]
mod tests {
    use super::{APP_ICON_SIZE, app_window_icon, build_app_icon_rgba};

    #[test]
    fn app_icon_has_expected_dimensions_and_non_empty_pixels() {
        let rgba = build_app_icon_rgba();
        assert_eq!(rgba.len(), (APP_ICON_SIZE * APP_ICON_SIZE * 4) as usize);
        let opaque_pixels = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[3] == 0xFF)
            .count();
        assert!(
            opaque_pixels > 1500,
            "icon should have solid console silhouette"
        );
        assert!(
            rgba.as_chunks::<4>().0.contains(&[0x2E, 0xC4, 0xB6, 0xFF]),
            "icon should include Obsidian teal accent"
        );
        let icon_data = app_window_icon();
        assert_eq!(icon_data.width, APP_ICON_SIZE);
        assert_eq!(icon_data.height, APP_ICON_SIZE);
    }
}
