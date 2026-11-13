mod app;
mod auth_client;
mod discover;
mod net;
mod theme;
mod trust;
mod ui;
mod util;

use app::{DissCordApp, SavedSession, SERVERS_KEY, SESSION_KEY};
use eframe::egui;

fn main() -> eframe::Result {
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
            theme::apply(&cc.egui_ctx);
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let servers = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, SERVERS_KEY))
                .unwrap_or_default();
            let show_create_box = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, app::CREATE_BOX_KEY))
                .unwrap_or(false);
            let saved_session: Option<SavedSession> = cc
                .storage
                .and_then(|storage| eframe::get_value(storage, SESSION_KEY))
                .flatten();
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
                show_create_box,
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
        self.drain_network();
        ui::root(self, ui);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SERVERS_KEY, &self.servers);
        eframe::set_value(storage, trust::PINS_KEY, &*trust::PINS.lock().unwrap());
        eframe::set_value(storage, app::CREATE_BOX_KEY, &self.show_create_box);
        eframe::set_value(storage, SESSION_KEY, &self.stored_session());
    }
}
