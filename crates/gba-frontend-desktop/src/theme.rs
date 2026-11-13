use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
use std::sync::Arc;

// DESIGN TOKENS
// - bg #121212, surface #181818, tile #1e1e1e, hover/selected #2a2a2a, border #2a2a2a (1px)
// - text #e6e6e6, secondary #8a8a8a, disabled #5a5a5a
// - accent: the existing teal, the only accent color. No orange/yellow/red/blue anywhere.
// - radius 2px, no shadows, gradients, or glows
// - Font: Segoe UI 12px everywhere. Monospace only for FPS, CRC/SHA, addresses.
pub const COLOR_BG: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
pub const COLOR_SURFACE: Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
pub const COLOR_TILE: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x1E);
pub const COLOR_HOVER_SELECTED: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const COLOR_BORDER: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);

pub const COLOR_TEXT: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xE6);
pub const COLOR_TEXT_SECONDARY: Color32 = Color32::from_rgb(0x8A, 0x8A, 0x8A);
pub const COLOR_TEXT_DISABLED: Color32 = Color32::from_rgb(0x5A, 0x5A, 0x5A);

pub const ACCENT_TEAL: Color32 = Color32::from_rgb(46, 196, 182);
pub const ACCENT_TEAL_BG: Color32 = Color32::from_rgb(26, 62, 62);

pub const RADIUS_DEFAULT: CornerRadius = CornerRadius::same(2);
pub const STROKE_BORDER: Stroke = Stroke {
    width: 1.0,
    color: COLOR_BORDER,
};
pub const STROKE_TEAL: Stroke = Stroke {
    width: 1.0,
    color: ACCENT_TEAL,
};
pub const STROKE_TEAL_2PX: Stroke = Stroke {
    width: 2.0,
    color: ACCENT_TEAL,
};

// Aliases for compatibility
pub const BG_APP: Color32 = COLOR_BG;
pub const BG_PANEL: Color32 = COLOR_SURFACE;
pub const BG_CARD: Color32 = COLOR_TILE;
pub const BORDER_HAIRLINE: Color32 = COLOR_BORDER;
pub const TEXT_MUTED: Color32 = COLOR_TEXT_SECONDARY;

/// Configures the unified native desktop theme:
/// Segoe UI 12px font runtime loading (on Windows), 2px radius, no shadows/glows, 4px grid.
pub fn configure_theme(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();

    #[cfg(target_os = "windows")]
    {
        let segoe_path = std::path::Path::new(r"C:\Windows\Fonts\segoeui.ttf");
        if segoe_path.exists()
            && let Ok(bytes) = std::fs::read(segoe_path)
        {
            fonts
                .font_data
                .insert("segoe_ui".to_owned(), Arc::new(FontData::from_owned(bytes)));
            if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
                family.insert(0, "segoe_ui".to_owned());
            }
        }
    }

    ctx.set_fonts(fonts);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = COLOR_SURFACE;
    visuals.window_fill = COLOR_SURFACE;
    visuals.extreme_bg_color = COLOR_BG;
    visuals.faint_bg_color = COLOR_TILE;

    visuals.window_corner_radius = RADIUS_DEFAULT;
    visuals.menu_corner_radius = RADIUS_DEFAULT;
    visuals.window_stroke = STROKE_BORDER;

    visuals.widgets.noninteractive.bg_fill = COLOR_SURFACE;
    visuals.widgets.noninteractive.bg_stroke = STROKE_BORDER;
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT);
    visuals.widgets.noninteractive.corner_radius = RADIUS_DEFAULT;

    visuals.widgets.inactive.bg_fill = COLOR_SURFACE;
    visuals.widgets.inactive.bg_stroke = STROKE_BORDER;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT);
    visuals.widgets.inactive.corner_radius = RADIUS_DEFAULT;

    visuals.widgets.hovered.bg_fill = COLOR_HOVER_SELECTED;
    visuals.widgets.hovered.bg_stroke = STROKE_BORDER;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT);
    visuals.widgets.hovered.corner_radius = RADIUS_DEFAULT;

    visuals.widgets.active.bg_fill = COLOR_HOVER_SELECTED;
    visuals.widgets.active.bg_stroke = STROKE_BORDER;
    visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT);
    visuals.widgets.active.corner_radius = RADIUS_DEFAULT;

    visuals.widgets.open.bg_fill = COLOR_HOVER_SELECTED;
    visuals.widgets.open.bg_stroke = STROKE_BORDER;
    visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, COLOR_TEXT);
    visuals.widgets.open.corner_radius = RADIUS_DEFAULT;

    visuals.selection.bg_fill = COLOR_HOVER_SELECTED;
    visuals.selection.stroke = STROKE_TEAL;

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(4.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 2.0);
    style.spacing.indent = 16.0;

    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(12.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Monospace),
    );

    ctx.set_style(style);
}
