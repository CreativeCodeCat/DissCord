//! Catppuccin Mocha colour palette and global egui styling.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Visuals};

/// Main chat background.
pub const APP_BG: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x2E);
/// Channel and member sidebars.
pub const SIDEBAR_BG: Color32 = Color32::from_rgb(0x18, 0x18, 0x25);
/// Far-left server rail and login backdrop.
pub const RAIL_BG: Color32 = Color32::from_rgb(0x11, 0x11, 0x1B);
/// Bottom-left user bar.
pub const USER_BAR_BG: Color32 = Color32::from_rgb(0x11, 0x11, 0x1B);
/// Message composer and text fields.
pub const INPUT_BG: Color32 = Color32::from_rgb(0x31, 0x32, 0x44);
pub const HOVER_BG: Color32 = Color32::from_rgb(0x31, 0x32, 0x44);
pub const SELECTED_BG: Color32 = Color32::from_rgb(0x45, 0x47, 0x5A);

pub const TEXT: Color32 = Color32::from_rgb(0xBA, 0xC2, 0xDE);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x7F, 0x84, 0x9C);
pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xCD, 0xD6, 0xF4);

pub const ACCENT: Color32 = Color32::from_rgb(0xCB, 0xA6, 0xF7);
/// Text links (text and underline).
pub const LINK: Color32 = Color32::from_rgb(0x89, 0xB4, 0xFA);
pub const GREEN: Color32 = Color32::from_rgb(0xA6, 0xE3, 0xA1);
pub const RED: Color32 = Color32::from_rgb(0xF3, 0x8B, 0xA8);

/// Colours used for avatars and author names.
const AVATAR_PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x89, 0xB4, 0xFA), // Blue
    Color32::from_rgb(0xF5, 0xC2, 0xE7), // Pink
    Color32::from_rgb(0xA6, 0xE3, 0xA1), // Green
    Color32::from_rgb(0xF9, 0xE2, 0xAF), // Yellow
    Color32::from_rgb(0xF3, 0x8B, 0xA8), // Red
    Color32::from_rgb(0xFA, 0xB3, 0x87), // Peach
    Color32::from_rgb(0x74, 0xC7, 0xEC), // Sapphire
    Color32::from_rgb(0xCB, 0xA6, 0xF7), // Mauve
];

/// A stable colour for a user name, so the same person always looks the same.
pub fn name_color(name: &str) -> Color32 {
    let hash = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    AVATAR_PALETTE[hash as usize % AVATAR_PALETTE.len()]
}

/// Loads the bundled Open Sans for all text. egui's own fonts stay behind it as fallbacks.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "open_sans".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/OpenSans-Regular.ttf"))),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "open_sans".into());
    ctx.set_fonts(fonts);
}

/// Applies the dark Discord-like theme to the whole app.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    // Small captions (labels like "NAME") default to 9 pt, which is too small for this font.
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        );
    });
    let mut v = Visuals::dark();
    v.panel_fill = APP_BG;
    v.window_fill = SIDEBAR_BG;
    v.extreme_bg_color = INPUT_BG;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = LINK;

    v.widgets.noninteractive.bg_fill = APP_BG;
    v.widgets.inactive.weak_bg_fill = INPUT_BG;
    v.widgets.hovered.weak_bg_fill = HOVER_BG;
    v.widgets.active.weak_bg_fill = SELECTED_BG;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
    ] {
        w.corner_radius = CornerRadius::same(6);
    }

    ctx.set_visuals(v);
}
