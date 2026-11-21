use std::sync::{Arc, OnceLock};

pub(crate) const APP_ICON_SIZE: u32 = 64;
pub(crate) const APP_LOGO_SIZE: u32 = 256;

const ICON_64_PNG: &[u8] = include_bytes!("../assets/icon_64.png");
const LOGO_256_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// Decodes the embedded 64x64 transparent crab-GBA application icon into RGBA8 pixels.
pub(crate) fn build_app_icon_rgba() -> Vec<u8> {
    static CACHED_64: OnceLock<Vec<u8>> = OnceLock::new();
    CACHED_64
        .get_or_init(|| {
            let img = image::load_from_memory(ICON_64_PNG)
                .expect("embedded assets/icon_64.png must be a valid PNG")
                .into_rgba8();
            assert_eq!(img.width(), APP_ICON_SIZE);
            assert_eq!(img.height(), APP_ICON_SIZE);
            img.into_raw()
        })
        .clone()
}

/// Decodes the embedded 256x256 transparent crab-GBA mascot logo into RGBA8 pixels.
pub(crate) fn build_app_logo_rgba() -> Vec<u8> {
    static CACHED_256: OnceLock<Vec<u8>> = OnceLock::new();
    CACHED_256
        .get_or_init(|| {
            let img = image::load_from_memory(LOGO_256_PNG)
                .expect("embedded assets/logo.png must be a valid PNG")
                .into_rgba8();
            assert_eq!(img.width(), APP_LOGO_SIZE);
            assert_eq!(img.height(), APP_LOGO_SIZE);
            img.into_raw()
        })
        .clone()
}

/// Returns the shared `egui::IconData` attached to desktop viewports.
pub(crate) fn app_window_icon() -> Arc<egui::IconData> {
    Arc::new(egui::IconData {
        rgba: build_app_icon_rgba(),
        width: APP_ICON_SIZE,
        height: APP_ICON_SIZE,
    })
}

/// Returns a `winit::window::Icon` for the No-GPU winit window presenter.
pub(crate) fn winit_window_icon() -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(build_app_icon_rgba(), APP_ICON_SIZE, APP_ICON_SIZE).ok()
}

/// Returns or registers the 256x256 crab-GBA logo texture inside `ctx` for UI dialogs.
pub(crate) fn app_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let rgba = build_app_logo_rgba();
    let color_img = egui::ColorImage::from_rgba_unmultiplied(
        [APP_LOGO_SIZE as usize, APP_LOGO_SIZE as usize],
        &rgba,
    );
    ctx.load_texture("rba_crab_gba_logo", color_img, egui::TextureOptions::LINEAR)
}

#[cfg(test)]
mod tests {
    use super::{
        APP_ICON_SIZE, APP_LOGO_SIZE, app_window_icon, build_app_icon_rgba, build_app_logo_rgba,
        winit_window_icon,
    };

    #[test]
    fn app_icon_has_expected_dimensions_and_transparent_background() {
        let rgba = build_app_icon_rgba();
        assert_eq!(rgba.len(), (APP_ICON_SIZE * APP_ICON_SIZE * 4) as usize);
        let pixels = rgba.as_chunks::<4>().0;
        // All 4 corners must be transparent (background removed).
        assert_eq!(pixels[0][3], 0, "top-left corner must be transparent");
        assert_eq!(
            pixels[(APP_ICON_SIZE - 1) as usize][3],
            0,
            "top-right corner must be transparent"
        );
        let opaque_pixels = pixels.iter().filter(|px| px[3] > 200).count();
        let transparent_pixels = pixels.iter().filter(|px| px[3] == 0).count();
        assert!(
            opaque_pixels > 1200,
            "crab-GBA icon should have solid mascot silhouette (got {opaque_pixels})"
        );
        assert!(
            transparent_pixels > 600,
            "crab-GBA icon should have removed background (got {transparent_pixels})"
        );

        let logo_rgba = build_app_logo_rgba();
        assert_eq!(
            logo_rgba.len(),
            (APP_LOGO_SIZE * APP_LOGO_SIZE * 4) as usize
        );

        let icon_data = app_window_icon();
        assert_eq!(icon_data.width, APP_ICON_SIZE);
        assert_eq!(icon_data.height, APP_ICON_SIZE);
        assert!(winit_window_icon().is_some());
    }
}
