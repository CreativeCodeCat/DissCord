//! Colour themes and global egui styling. The chosen theme is kept in a global, and the
//! colours are read with `pal()`.

use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

use eframe::egui::{self, Color32, CornerRadius, Visuals};

/// The colours of one theme.
pub struct Palette {
    pub dark: bool,
    /// Main chat background.
    pub app_bg: Color32,
    /// Channel and member sidebars.
    pub sidebar_bg: Color32,
    /// Far-left server rail and login backdrop.
    pub rail_bg: Color32,
    /// Bottom-left user bar.
    pub user_bar_bg: Color32,
    /// Message composer and text fields.
    pub input_bg: Color32,
    pub hover_bg: Color32,
    pub selected_bg: Color32,

    pub text: Color32,
    pub text_muted: Color32,
    pub text_strong: Color32,

    pub accent: Color32,
    /// Text links (text and underline).
    pub link: Color32,
    pub green: Color32,
    pub red: Color32,
    /// Text on coloured buttons (the accent, green and red buttons).
    pub on_color: Color32,

    /// Colours used for avatars and author names.
    pub avatars: [Color32; 8],
}

/// The themes the app offers. Breeze Dark is the default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    #[default]
    BreezeDark,
    BreezeLight,
    CatppuccinMocha,
    CatppuccinLatte,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Theme::BreezeDark, Theme::BreezeLight, Theme::CatppuccinMocha, Theme::CatppuccinLatte];

    pub fn name(self) -> &'static str {
        match self {
            Theme::BreezeDark => "Breeze Dark",
            Theme::BreezeLight => "Breeze Light",
            Theme::CatppuccinMocha => "Catppuccin Mocha",
            Theme::CatppuccinLatte => "Catppuccin Latte",
        }
    }

    fn palette(self) -> &'static Palette {
        match self {
            Theme::BreezeDark => &BREEZE_DARK,
            Theme::BreezeLight => &BREEZE_LIGHT,
            Theme::CatppuccinMocha => &CATPPUCCIN_MOCHA,
            Theme::CatppuccinLatte => &CATPPUCCIN_LATTE,
        }
    }
}

/// The theme in use, as an index into `Theme::ALL`.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The theme in use.
pub fn current() -> Theme {
    Theme::ALL[CURRENT.load(Ordering::Relaxed) as usize % Theme::ALL.len()]
}

/// Chooses the theme. Call `apply` afterwards so the whole app is redrawn in it.
pub fn set(theme: Theme) {
    let index = Theme::ALL.iter().position(|t| *t == theme).unwrap_or(0);
    CURRENT.store(index as u8, Ordering::Relaxed);
}

/// The colours of the theme in use.
pub fn pal() -> &'static Palette {
    current().palette()
}

// KDE Breeze Dark, with the Discord-style layout of the other themes.
const BREEZE_DARK: Palette = Palette {
    dark: true,
    app_bg: Color32::from_rgb(0x2A, 0x2E, 0x32),
    sidebar_bg: Color32::from_rgb(0x23, 0x26, 0x29),
    rail_bg: Color32::from_rgb(0x1B, 0x1E, 0x20),
    user_bar_bg: Color32::from_rgb(0x1B, 0x1E, 0x20),
    input_bg: Color32::from_rgb(0x31, 0x36, 0x3B),
    hover_bg: Color32::from_rgb(0x31, 0x36, 0x3B),
    selected_bg: Color32::from_rgb(0x3D, 0x43, 0x48),
    text: Color32::from_rgb(0xD6, 0xD9, 0xDC),
    text_muted: Color32::from_rgb(0x7F, 0x8C, 0x8D),
    text_strong: Color32::from_rgb(0xFC, 0xFC, 0xFC),
    accent: Color32::from_rgb(0x3D, 0xAE, 0xE9),
    link: Color32::from_rgb(0x1D, 0x99, 0xF3),
    green: Color32::from_rgb(0x27, 0xAE, 0x60),
    red: Color32::from_rgb(0xDA, 0x44, 0x53),
    on_color: Color32::from_rgb(0x1B, 0x1E, 0x20),
    avatars: [
        Color32::from_rgb(0x3D, 0xAE, 0xE9),
        Color32::from_rgb(0x9B, 0x59, 0xB6),
        Color32::from_rgb(0x27, 0xAE, 0x60),
        Color32::from_rgb(0xFD, 0xBC, 0x4B),
        Color32::from_rgb(0xDA, 0x44, 0x53),
        Color32::from_rgb(0xF6, 0x74, 0x00),
        Color32::from_rgb(0x1D, 0x99, 0xF3),
        Color32::from_rgb(0x8E, 0x44, 0xAD),
    ],
};

// KDE Breeze Light, with the same layout.
const BREEZE_LIGHT: Palette = Palette {
    dark: false,
    app_bg: Color32::from_rgb(0xFC, 0xFC, 0xFC),
    sidebar_bg: Color32::from_rgb(0xEF, 0xF0, 0xF1),
    rail_bg: Color32::from_rgb(0xDD, 0xE0, 0xE3),
    user_bar_bg: Color32::from_rgb(0xDD, 0xE0, 0xE3),
    input_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hover_bg: Color32::from_rgb(0xE3, 0xE5, 0xE7),
    selected_bg: Color32::from_rgb(0xD0, 0xD4, 0xD8),
    text: Color32::from_rgb(0x3B, 0x3F, 0x42),
    text_muted: Color32::from_rgb(0x7F, 0x8C, 0x8D),
    text_strong: Color32::from_rgb(0x14, 0x16, 0x18),
    accent: Color32::from_rgb(0x3D, 0xAE, 0xE9),
    link: Color32::from_rgb(0x29, 0x80, 0xB9),
    green: Color32::from_rgb(0x27, 0xAE, 0x60),
    red: Color32::from_rgb(0xDA, 0x44, 0x53),
    on_color: Color32::from_rgb(0x14, 0x16, 0x18),
    avatars: [
        Color32::from_rgb(0x29, 0x80, 0xB9),
        Color32::from_rgb(0x8E, 0x44, 0xAD),
        Color32::from_rgb(0x27, 0xAE, 0x60),
        Color32::from_rgb(0xC9, 0x8A, 0x00),
        Color32::from_rgb(0xDA, 0x44, 0x53),
        Color32::from_rgb(0xF6, 0x74, 0x00),
        Color32::from_rgb(0x1A, 0xA3, 0xB0),
        Color32::from_rgb(0x9B, 0x59, 0xB6),
    ],
};

const CATPPUCCIN_MOCHA: Palette = Palette {
    dark: true,
    app_bg: Color32::from_rgb(0x1E, 0x1E, 0x2E),
    sidebar_bg: Color32::from_rgb(0x18, 0x18, 0x25),
    rail_bg: Color32::from_rgb(0x11, 0x11, 0x1B),
    user_bar_bg: Color32::from_rgb(0x11, 0x11, 0x1B),
    input_bg: Color32::from_rgb(0x31, 0x32, 0x44),
    hover_bg: Color32::from_rgb(0x31, 0x32, 0x44),
    selected_bg: Color32::from_rgb(0x45, 0x47, 0x5A),
    text: Color32::from_rgb(0xBA, 0xC2, 0xDE),
    text_muted: Color32::from_rgb(0x7F, 0x84, 0x9C),
    text_strong: Color32::from_rgb(0xCD, 0xD6, 0xF4),
    accent: Color32::from_rgb(0xCB, 0xA6, 0xF7),
    link: Color32::from_rgb(0x89, 0xB4, 0xFA),
    green: Color32::from_rgb(0xA6, 0xE3, 0xA1),
    red: Color32::from_rgb(0xF3, 0x8B, 0xA8),
    on_color: Color32::from_rgb(0x11, 0x11, 0x1B),
    avatars: [
        Color32::from_rgb(0x89, 0xB4, 0xFA),
        Color32::from_rgb(0xF5, 0xC2, 0xE7),
        Color32::from_rgb(0xA6, 0xE3, 0xA1),
        Color32::from_rgb(0xF9, 0xE2, 0xAF),
        Color32::from_rgb(0xF3, 0x8B, 0xA8),
        Color32::from_rgb(0xFA, 0xB3, 0x87),
        Color32::from_rgb(0x74, 0xC7, 0xEC),
        Color32::from_rgb(0xCB, 0xA6, 0xF7),
    ],
};

const CATPPUCCIN_LATTE: Palette = Palette {
    dark: false,
    app_bg: Color32::from_rgb(0xEF, 0xF1, 0xF5),
    sidebar_bg: Color32::from_rgb(0xE6, 0xE9, 0xEF),
    rail_bg: Color32::from_rgb(0xDC, 0xE0, 0xE8),
    user_bar_bg: Color32::from_rgb(0xDC, 0xE0, 0xE8),
    input_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hover_bg: Color32::from_rgb(0xCC, 0xD0, 0xDA),
    selected_bg: Color32::from_rgb(0xBC, 0xC0, 0xCC),
    text: Color32::from_rgb(0x5C, 0x5F, 0x77),
    text_muted: Color32::from_rgb(0x8C, 0x8F, 0xA1),
    text_strong: Color32::from_rgb(0x4C, 0x4F, 0x69),
    accent: Color32::from_rgb(0x88, 0x39, 0xEF),
    link: Color32::from_rgb(0x1E, 0x66, 0xF5),
    green: Color32::from_rgb(0x40, 0xA0, 0x2B),
    red: Color32::from_rgb(0xD2, 0x0F, 0x39),
    on_color: Color32::from_rgb(0xEF, 0xF1, 0xF5),
    avatars: [
        Color32::from_rgb(0x1E, 0x66, 0xF5),
        Color32::from_rgb(0xEA, 0x76, 0xCB),
        Color32::from_rgb(0x40, 0xA0, 0x2B),
        Color32::from_rgb(0xDF, 0x8E, 0x1D),
        Color32::from_rgb(0xD2, 0x0F, 0x39),
        Color32::from_rgb(0xFE, 0x64, 0x0B),
        Color32::from_rgb(0x20, 0x9F, 0xB5),
        Color32::from_rgb(0x88, 0x39, 0xEF),
    ],
};

/// A stable colour for a user name, so the same person always looks the same.
pub fn name_color(name: &str) -> Color32 {
    let hash = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    let palette = pal();
    palette.avatars[hash as usize % palette.avatars.len()]
}

/// Loads the bundled Open Sans for all text. Noto Sans (a subset with Latin, IPA and small capital
/// letters) comes next for characters Open Sans lacks, such as the small capitals in server names.
/// egui's own fonts stay behind them as fallbacks.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "open_sans".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/OpenSans-Regular.ttf"))),
    );
    fonts.font_data.insert(
        "noto_sans".into(),
        Arc::new(egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSans-Subset.ttf"))),
    );
    let family = fonts.families.entry(egui::FontFamily::Proportional).or_default();
    family.insert(0, "noto_sans".into());
    family.insert(0, "open_sans".into());
    ctx.set_fonts(fonts);
}

/// Applies the theme in use to the whole app.
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    // Small captions (labels like "NAME") default to 9 pt, which is too small for this font.
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::new(11.0, egui::FontFamily::Proportional),
        );
    });
    let p = pal();
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.app_bg;
    v.window_fill = p.sidebar_bg;
    v.extreme_bg_color = p.input_bg;
    v.override_text_color = Some(p.text);
    v.selection.bg_fill = p.accent;
    v.hyperlink_color = p.link;

    v.widgets.noninteractive.bg_fill = p.app_bg;
    v.widgets.inactive.weak_bg_fill = p.input_bg;
    v.widgets.hovered.weak_bg_fill = p.hover_bg;
    v.widgets.active.weak_bg_fill = p.selected_bg;
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
