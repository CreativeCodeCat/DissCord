mod app;
mod auth_client;
mod discover;
mod keychain;
mod emoji_images;
mod net;
mod theme;
mod trust;
mod ui;
mod util;

use app::{DissCordApp, SavedSession, SERVERS_KEY, SESSION_KEY};
use eframe::egui;

fn main() -> eframe::Result {
    common::env::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([820.0, 480.0])
            .with_title("DissCord")
            // The app draws its own header bar with the window buttons (see ui::top_bar).
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native(
        "DissCord",
        options,
        Box::new(|cc| {
            let theme = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, app::THEME_KEY))
                .unwrap_or_default();
            theme::set(theme);
            let skin_tone = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, app::SKIN_TONE_KEY))
                .unwrap_or(0);
            theme::apply(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let servers = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, SERVERS_KEY))
                .unwrap_or_default();
            let saved_session: Option<SavedSession> = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, SESSION_KEY))
                .flatten()
                .map(|mut saved: SavedSession| {
                    // The token is not in the data file. Read it from the keychain. A token left in
                    // the file by an older version is kept as it is, and moves to the keychain on save.
                    if saved.token.is_empty() {
                        saved.token = keychain::load(&saved.name).unwrap_or_default();
                    }
                    saved
                })
                .filter(|saved| !saved.token.is_empty());
            // Certificates remembered from earlier runs.
            if let Some(pins) = cc
                .storage
                .and_then(|storage| eframe::get_value::<std::collections::BTreeMap<String, String>>(storage, trust::PINS_KEY))
            {
                *trust::PINS.lock().unwrap() = pins;
            }
            let mut app = DissCordApp {
                servers,
                selected: None,
                remember: true,
                theme,
                skin_tone,
                saved_session,
                ..Default::default()
            };
            // Check a saved login now, so the first frame already shows the chat.
            app.try_auto_login(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}

impl eframe::App for DissCordApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Apply a theme change saved on the Account page.
        if theme::current() != self.theme {
            theme::set(self.theme);
            theme::apply(ui.ctx());
        }
        self.drain_network();
        ui::root(self, ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SERVERS_KEY, &self.servers);
        eframe::set_value(storage, trust::PINS_KEY, &*trust::PINS.lock().unwrap());
        eframe::set_value(storage, app::THEME_KEY, &self.theme);
        eframe::set_value(storage, app::SKIN_TONE_KEY, &self.skin_tone);
        let session = self.stored_session();
        if let Some(saved) = &session {
            if let Err(e) = keychain::store(&saved.name, &saved.token) {
                eprintln!("could not save your login to the keychain: {e}");
            }
        }
        eframe::set_value(storage, SESSION_KEY, &session);
    }
}
