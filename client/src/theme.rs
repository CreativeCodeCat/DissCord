//! Discord-style colour palette and global egui styling.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Visuals};

/// Main chat background.
pub const APP_BG: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
/// Channel and member sidebars.
pub const SIDEBAR_BG: Color32 = Color32::from_rgb(0x2B, 0x2D, 0x31);
/// Far-left server rail and login backdrop.
pub const RAIL_BG: Color32 = Color32::from_rgb(0x1E, 0x1F, 0x22);
/// Bottom-left user bar.
pub const USER_BAR_BG: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
/// Message composer and text fields.
pub const INPUT_BG: Color32 = Color32::from_rgb(0x38, 0x3A, 0x40);
pub const HOVER_BG: Color32 = Color32::from_rgb(0x35, 0x37, 0x3C);
pub const SELECTED_BG: Color32 = Color32::from_rgb(0x40, 0x42, 0x48);

pub const TEXT: Color32 = Color32::from_rgb(0xDB, 0xDE, 0xE1);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x94, 0x9B, 0xA4);
pub const TEXT_STRONG: Color32 = Color32::WHITE;

pub const ACCENT: Color32 = Color32::from_rgb(0x58, 0x65, 0xF2);
/// Text links (text and underline).
pub const LINK: Color32 = Color32::from_rgb(0x8E, 0xA1, 0xE1);
pub const GREEN: Color32 = Color32::from_rgb(0x23, 0xA5, 0x5A);
pub const RED: Color32 = Color32::from_rgb(0xF2, 0x3F, 0x43);

/// Colours used for avatars and author names.
const AVATAR_PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x58, 0x65, 0xF2),
    Color32::from_rgb(0xEB, 0x45, 0x9E),
    Color32::from_rgb(0x57, 0xF2, 0x87),
    Color32::from_rgb(0xFE, 0xE7, 0x5C),
    Color32::from_rgb(0xED, 0x42, 0x45),
    Color32::from_rgb(0xF5, 0x77, 0x31),
    Color32::from_rgb(0x00, 0xA8, 0xFC),
    Color32::from_rgb(0x9B, 0x84, 0xEE),
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
