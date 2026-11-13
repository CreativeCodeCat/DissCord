//! All drawing code. State and behaviour live in `app.rs`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::NaiveDate;
use common::ChatMessage;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Frame, Margin, Response, RichText, Sense, TextEdit, Ui};

use crate::app::{DissCordApp, Entry, MenuAction};
use crate::discover::FoundServer;
use crate::theme::*;
use crate::util::{day_key, day_label, stamp_label, time_label};

/// Messages from the same author within this many seconds are grouped under one header.
const GROUP_GAP_SECS: u64 = 7 * 60;
/// Width of the avatar column plus its spacing, used to indent grouped messages.
const AVATAR_INDENT: f32 = 60.0;

pub fn root(app: &mut DissCordApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let mut actions = Vec::new();

    window_resize_edges(&ctx);
    top_bar(app, ui);
    if app.is_logged_in() {
        if app.show_server_settings && app.is_chat_ready() {
            // The settings page takes over the whole window, like Discord's.
            server_settings_page(app, ui);
        } else if app.show_discover {
            discover_page(app, ui);
        } else {
            // Once logged in, the server rail is shown and lets you pick a server.
            server_rail(app, ui, &mut actions);
            if app.is_chat_ready() {
                chat(app, ui, &mut actions);
            } else {
                server_status(app, ui, &mut actions);
            }
        }
    } else {
        login(app, ui);
    }
    add_server_dialog(app, &ctx);

    // Menus and buttons queued their choices while drawing; apply them now.
    for action in actions {
        app.apply(action, &ctx);
    }
}

/// The window buttons on the right of the header bar.
#[derive(Clone, Copy, PartialEq)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A header bar across the top of the window. Drag it to move the window, double-click it to
/// maximize or restore, and use the buttons on the right to minimize, maximize or close.
fn top_bar(app: &DissCordApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let title = match app.selected_name() {
        Some(name) if app.is_chat_ready() => format!("#{}  ·  {name}", app.channel),
        _ => "DissCord".to_string(),
    };

    egui::Panel::top("top_bar")
        .exact_size(32.0)
        .frame(Frame::new().fill(RAIL_BG))
        .show(ui, |ui| {
            // Added before the buttons, so the buttons take their own clicks.
            let bar = ui.max_rect();
            let drag = ui.interact(bar, ui.scope_id().with("title_drag"), Sense::click_and_drag());
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            } else if drag.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.painter().text(
                bar.center(),
                Align2::CENTER_CENTER,
                title,
                FontId::proportional(14.0),
                TEXT,
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if window_button(ui, WindowButton::Close) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                let maximize = if maximized { WindowButton::Restore } else { WindowButton::Maximize };
                if window_button(ui, maximize) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
                if window_button(ui, WindowButton::Minimize) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            });
        });
}

/// One window button (46 px wide). Returns true when clicked. Close turns red on hover.
fn window_button(ui: &mut Ui, kind: WindowButton) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(46.0, 32.0), Sense::click());
    let hovered = resp.hovered();
    let color = match (kind, hovered) {
        (WindowButton::Close, true) => {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, RED);
            TEXT_STRONG
        }
        (_, true) => {
            ui.painter().rect_filled(rect, CornerRadius::ZERO, HOVER_BG);
            TEXT_STRONG
        }
        _ => TEXT,
    };

    let painter = ui.painter();
    let c = rect.center();
    let stroke = egui::Stroke::new(1.2, color);
    let r = 5.0;
    match kind {
        WindowButton::Minimize => {
            painter.line_segment([c + egui::vec2(-r, 0.0), c + egui::vec2(r, 0.0)], stroke);
        }
        WindowButton::Maximize => {
            painter.rect_stroke(
                egui::Rect::from_center_size(c, egui::vec2(2.0 * r, 2.0 * r)),
                CornerRadius::ZERO,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        WindowButton::Restore => {
            // Two overlapping squares: the back one up and to the right.
            let back = egui::Rect::from_center_size(c + egui::vec2(2.0, -2.0), egui::vec2(2.0 * r - 2.0, 2.0 * r - 2.0));
            painter.rect_stroke(back, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
            let front = egui::Rect::from_center_size(c + egui::vec2(-1.0, 1.0), egui::vec2(2.0 * r - 2.0, 2.0 * r - 2.0));
            painter.rect_filled(front, CornerRadius::ZERO, if hovered { HOVER_BG } else { RAIL_BG });
            painter.rect_stroke(front, CornerRadius::ZERO, stroke, egui::StrokeKind::Inside);
        }
        WindowButton::Close => {
            painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
            painter.line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke);
        }
    }
    resp.clicked()
}

/// The window has no system frame, so dragging its edges is handled here.
/// Shows the matching resize cursor near an edge and starts a resize on press.
fn window_resize_edges(ctx: &egui::Context) {
    const EDGE: f32 = 6.0;
    let (rect, pos, pressed, maximized) = ctx.input(|i| {
        (
            i.content_rect(),
            i.pointer.latest_pos(),
            i.pointer.primary_pressed(),
            i.viewport().maximized.unwrap_or(false),
        )
    });
    if maximized {
        return;
    }
    let Some(p) = pos else { return };
    if !rect.expand(EDGE).contains(p) {
        return;
    }
    use egui::viewport::ResizeDirection as D;
    let north = p.y - rect.top() < EDGE;
    let south = rect.bottom() - p.y < EDGE;
    let west = p.x - rect.left() < EDGE;
    let east = rect.right() - p.x < EDGE;
    let (dir, cursor) = match (north, south, west, east) {
        (true, _, true, _) => (D::NorthWest, egui::CursorIcon::ResizeNorthWest),
        (true, _, _, true) => (D::NorthEast, egui::CursorIcon::ResizeNorthEast),
        (_, true, true, _) => (D::SouthWest, egui::CursorIcon::ResizeSouthWest),
        (_, true, _, true) => (D::SouthEast, egui::CursorIcon::ResizeSouthEast),
        (true, _, _, _) => (D::North, egui::CursorIcon::ResizeNorth),
        (_, true, _, _) => (D::South, egui::CursorIcon::ResizeSouth),
        (_, _, true, _) => (D::West, egui::CursorIcon::ResizeWest),
        (_, _, _, true) => (D::East, egui::CursorIcon::ResizeEast),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if pressed {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
    }
}

/// The column of server icons on the far left, with "+" to add a server.
fn server_rail(app: &DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    egui::Panel::left("rail")
        .exact_size(72.0)
        .frame(Frame::new().fill(RAIL_BG))
        .show(ui, |ui| {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                for (i, server) in app.servers.iter().enumerate() {
                    let selected = app.selected == Some(i);
                    let resp = server_button(ui, &server.name, server.icon.as_deref(), selected);
                    if resp.clicked() {
                        actions.push(MenuAction::SelectServer(i));
                    }
                    resp.context_menu(|ui| {
                        if selected && app.is_chat_ready() {
                            server_menu_items(ui, app, actions);
                        } else if menu_item(ui, "Remove server", true) {
                            actions.push(MenuAction::RemoveServer(i));
                        }
                    });
                    ui.add_space(8.0);
                }
                if add_server_button(ui).clicked() {
                    actions.push(MenuAction::OpenAddServer);
                }
                ui.add_space(8.0);
                if discover_button(ui).on_hover_text("Find servers").clicked() {
                    actions.push(MenuAction::OpenDiscover);
                }
            });
        });
}

/// A server icon: first letter of the name on a colour. The selected one is rounder with a bar.
fn server_button(ui: &mut Ui, name: &str, icon: Option<&str>, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::click());
    let hovered = resp.hovered();
    let (fill, radius) = if selected || hovered {
        (name_color(name), 16)
    } else {
        (INPUT_BG, 24)
    };
    paint_server_icon(ui, rect, name, icon, fill, radius, 20.0);
    if selected {
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left() - 14.0, rect.center().y - 20.0),
            egui::vec2(4.0, 40.0),
        );
        ui.painter().rect_filled(bar, CornerRadius::same(2), TEXT_STRONG);
    }
    resp.on_hover_text(name)
}

/// Draws a server's icon in `rect`: its uploaded picture if it has one, otherwise a coloured
/// square with the first letter of the name.
fn paint_server_icon(
    ui: &Ui,
    rect: egui::Rect,
    name: &str,
    icon: Option<&str>,
    fill: Color32,
    radius: u8,
    letter_size: f32,
) {
    if let Some((data, bytes)) = icon.and_then(|data| Some((data, decode_icon(data)?))) {
        // The uri changes with the picture, so a new icon is loaded rather than a cached old one.
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        let uri = format!("bytes://server-icon/{:x}.png", hasher.finish());
        egui::Image::from_bytes(uri, bytes)
            .corner_radius(CornerRadius::same(radius))
            .paint_at(ui, rect);
        return;
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(letter_size),
        TEXT_STRONG,
    );
}

/// Turns a stored base64 icon back into PNG bytes. None if it is not valid base64.
fn decode_icon(png_base64: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(png_base64).ok()
}

fn add_server_button(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::click());
    let hovered = resp.hovered();
    let (fill, radius, text_color) = if hovered {
        (GREEN, 16, TEXT_STRONG)
    } else {
        (INPUT_BG, 24, GREEN)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "+",
        FontId::proportional(26.0),
        text_color,
    );
    resp.on_hover_text("Add a server")
}

/// The server name bar at the top of the channel sidebar. Clicking it opens the server menu.
fn server_header(app: &DissCordApp, ui: &mut Ui, title: &str, actions: &mut Vec<MenuAction>) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), Sense::click());
    let bg = if resp.is_pointer_button_down_on() {
        SELECTED_BG
    } else if resp.hovered() || egui::Popup::is_id_open(ui.ctx(), resp.id) {
        HOVER_BG
    } else {
        SIDEBAR_BG
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, bg);
    // The name starts after the house when this server is public, so the house sits on the left.
    let mut title_x = 16.0;
    if app.server_public {
        // A small house before the name: other servers know this one.
        let house = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 23.0, rect.center().y),
            egui::vec2(14.0, 14.0),
        );
        egui::Image::new(egui::include_image!("../assets/house.svg")).paint_at(ui, house);
        title_x = 38.0;
    }
    painter.text(
        rect.left_center() + egui::vec2(title_x, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(16.0),
        TEXT_STRONG,
    );
    // Chevron on the right, pointing down like Discord's.
    let c = rect.right_center() - egui::vec2(18.0, 0.0);
    painter.add(egui::Shape::convex_polygon(
        vec![c + egui::vec2(-5.0, -2.5), c + egui::vec2(5.0, -2.5), c + egui::vec2(0.0, 2.5)],
        TEXT,
        egui::Stroke::NONE,
    ));

    // Same line as the chat header's bottom edge, so the two line up.
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.painter().hline(rect.x_range(), rect.bottom(), stroke);

    egui::Popup::menu(&resp)
        .width(rect.width() - 16.0)
        .show(|ui| server_menu_items(ui, app, actions));
}

/// One full-width row in a Discord-style menu. Returns true when clicked.
/// `danger` rows are red, and their hover colour is red too.
fn menu_item(ui: &mut Ui, label: &str, danger: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
    let (fill, text) = match (resp.hovered(), danger) {
        (true, true) => (Some(RED), TEXT_STRONG),
        (true, false) => (Some(ACCENT), TEXT_STRONG),
        (false, true) => (None, RED),
        (false, false) => (None, TEXT),
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        text,
    );
    resp.clicked()
}

fn menu_divider(ui: &mut Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, SELECTED_BG);
    ui.add_space(4.0);
}

/// Items shared by the server header's menu and the server icon's right-click menu.
fn server_menu_items(ui: &mut Ui, app: &DissCordApp, actions: &mut Vec<MenuAction>) {
    ui.spacing_mut().item_spacing.y = 2.0;
    if app.is_owner() && menu_item(ui, "Server settings", false) {
        actions.push(MenuAction::OpenServerSettings);
    }
    if menu_item(ui, "Create channel", false) {
        actions.push(MenuAction::CreateChannel);
    }
    menu_divider(ui);
    let toggle = if app.members_hidden { "Show member list" } else { "Hide member list" };
    if menu_item(ui, toggle, false) {
        actions.push(MenuAction::ToggleMembers);
    }
    if menu_item(ui, "Copy server address", false) {
        actions.push(MenuAction::CopyText(app.connected_addr.clone()));
    }
    menu_divider(ui);
    if let Some(index) = app.selected {
        if menu_item(ui, "Leave server", true) {
            actions.push(MenuAction::RemoveServer(index));
        }
    }
}

fn login(app: &mut DissCordApp, ui: &mut Ui) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(APP_BG))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.12).max(0.0));
                Frame::new()
                    .fill(SIDEBAR_BG)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        login_form(app, ui);
                    });
            });
        });
}

/// Log in or register. The account works on every server, so no server is asked for here.
fn login_form(app: &mut DissCordApp, ui: &mut Ui) {
    let heading = if app.register_mode { "Create an account" } else { "Welcome back!" };
    ui.label(RichText::new(heading).size(24.0).strong().color(TEXT_STRONG));
    ui.label(RichText::new("One account works on every DissCord server.").color(TEXT_MUTED));
    ui.add_space(16.0);

    let mut submit = false;
    if app.register_mode {
        field_label(ui, "DISPLAY NAME");
        let resp = ui.add(
            TextEdit::singleline(&mut app.login_name)
                .hint_text("your name")
                .desired_width(f32::INFINITY),
        );
        submit |= enter_pressed(ui, &resp);
        ui.add_space(12.0);
    }

    field_label(ui, "EMAIL");
    let resp = ui.add(
        TextEdit::singleline(&mut app.login_email)
            .hint_text("you@example.com")
            .desired_width(f32::INFINITY),
    );
    submit |= enter_pressed(ui, &resp);
    ui.add_space(12.0);

    field_label(ui, "PASSWORD");
    let resp = ui.add(
        TextEdit::singleline(&mut app.login_password)
            .password(true)
            .hint_text("password")
            .desired_width(f32::INFINITY),
    );
    submit |= enter_pressed(ui, &resp);

    ui.add_space(12.0);
    ui.checkbox(
        &mut app.remember,
        RichText::new("Keep me logged in on this computer").color(TEXT_MUTED),
    );

    ui.add_space(16.0);
    let label = if app.register_mode { "Register" } else { "Log in" };
    if primary_button(ui, label).clicked() {
        submit = true;
    }

    ui.add_space(12.0);
    ui.horizontal(|ui| {
        let prompt = if app.register_mode {
            "Already have an account?"
        } else {
            "Need an account?"
        };
        ui.label(RichText::new(prompt).color(TEXT_MUTED));
        let link = if app.register_mode { "Log in" } else { "Register" };
        if ui.link(link).clicked() {
            app.register_mode = !app.register_mode;
            app.login_error = None;
        }
    });

    if let Some(err) = &app.login_error {
        ui.add_space(8.0);
        ui.label(RichText::new(err).color(RED));
    }

    if submit {
        app.log_in();
    }
}

/// Shown when logged in but no server is open: add one, wait for a join, or retry.
fn server_status(app: &mut DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    egui::CentralPanel::default()
        .frame(Frame::new().fill(APP_BG))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.12).max(0.0));
                Frame::new()
                    .fill(SIDEBAR_BG)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        if app.servers.is_empty() {
                            ui.label(RichText::new("Add a server").size(24.0).strong().color(TEXT_STRONG));
                            ui.label(
                                RichText::new("Enter the address of a DissCord server to start chatting.")
                                    .color(TEXT_MUTED),
                            );
                            ui.add_space(16.0);
                            if primary_button(ui, "Add a server").clicked() {
                                actions.push(MenuAction::OpenAddServer);
                            }
                        } else if app.conn.is_some() {
                            let name = app.selected_name().unwrap_or("server").to_string();
                            ui.label(
                                RichText::new(format!("Joining {name}...")).size(24.0).strong().color(TEXT_STRONG),
                            );
                            ui.label(RichText::new("Checking your account with the server.").color(TEXT_MUTED));
                        } else if let Some(err) = app.server_error.clone() {
                            ui.label(
                                RichText::new("Couldn't join the server").size(24.0).strong().color(TEXT_STRONG),
                            );
                            ui.label(RichText::new(err).color(RED));
                            ui.add_space(16.0);
                            if app.selected.is_some() && primary_button(ui, "Try again").clicked() {
                                actions.push(MenuAction::RetryServer);
                            }
                        } else {
                            ui.label(RichText::new("Pick a server").size(24.0).strong().color(TEXT_STRONG));
                            ui.label(
                                RichText::new("Choose one from the icons on the left, or add a new one with +.")
                                    .color(TEXT_MUTED),
                            );
                        }

                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Signed in as {}", app.user)).small().color(TEXT_MUTED));
                            if ui.link("Log out").clicked() {
                                actions.push(MenuAction::Logout);
                            }
                        });
                    });
            });
        });
}

/// A full-width accent button.
fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let button = egui::Button::new(RichText::new(label).strong().color(TEXT_STRONG))
        .fill(ACCENT)
        .min_size(egui::vec2(ui.available_width(), 40.0));
    ui.add(button)
}

/// The "Add a server" dialog, opened from the "+" button in the rail.
/// The compass button in the rail that opens "Find servers".
fn discover_button(ui: &mut Ui) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::click());
    let (fill, radius, color) = if resp.hovered() {
        (ACCENT, 16, TEXT_STRONG)
    } else {
        (INPUT_BG, 24, GREEN)
    };
    ui.painter().rect_filled(rect, CornerRadius::same(radius), fill);
    paint_icon(ui.painter(), rect.center(), Icon::Explore, color);
    resp
}

/// Full-page "Find servers", opened from the rail's compass button. Esc or the X goes back.
/// Lists the servers found through the server you have open, each with an Add button.
fn discover_page(app: &mut DissCordApp, ui: &mut Ui) {
    app.drain_discovered();
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut to_add: Option<usize> = None;
    let mut toggle: Option<String> = None;

    egui::Panel::left("discover_nav")
        .exact_size(232.0)
        .frame(Frame::new().fill(SIDEBAR_BG).inner_margin(Margin::symmetric(12, 24)))
        .show(ui, |ui| {
            ui.label(RichText::new("DISCOVER").small().strong().color(TEXT_MUTED));
            ui.add_space(8.0);
            nav_selected(ui, "Find servers");
        });

    egui::CentralPanel::default()
        .frame(Frame::new().fill(APP_BG).inner_margin(Margin::symmetric(40, 24)))
        .show(ui, |ui| {
            close |= close_with_hint(ui);

            ui.label(RichText::new("Find servers").size(20.0).strong().color(TEXT_STRONG));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Servers found through the servers you know. Pick one to add it.")
                    .color(TEXT),
            );
            ui.add_space(20.0);

            if let Some(err) = &app.discover_error {
                ui.label(RichText::new(err).color(RED));
            }
            if app.discover_error.is_none() && app.discovered.is_empty() {
                ui.label(RichText::new("Searching...").color(TEXT_MUTED));
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, found) in app.discovered.iter().enumerate() {
                        let saved = app.servers.iter().any(|s| s.addr == found.addr);
                        let expanded = app.discover_expanded.contains(&found.addr);
                        match discovered_row(ui, found, saved, expanded) {
                            RowAction::Add => to_add = Some(i),
                            RowAction::Toggle => toggle = Some(found.addr.clone()),
                            RowAction::None => {}
                        }
                        ui.add_space(8.0);
                    }
                });
        });

    if let Some(addr) = toggle {
        if !app.discover_expanded.remove(&addr) {
            app.discover_expanded.insert(addr);
        }
    }
    if let Some(i) = to_add {
        let found = app.discovered[i].clone();
        app.add_discovered(&found);
    }
    if close {
        app.show_discover = false;
    }
}

/// The round X with its ESC hint, at the top right of a page. Returns true when clicked.
/// The round X with its ESC hint underneath, at the top right of a page. Returns true when clicked.
/// The round X at the top right of a page, with its ESC hint underneath. Returns true when clicked.
fn close_with_hint(ui: &mut Ui) -> bool {
    let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 52.0), Sense::hover());
    let button = egui::Rect::from_min_size(
        egui::pos2(row.right() - 36.0, row.top()),
        egui::vec2(36.0, 36.0),
    );
    let resp = ui.interact(button, ui.scope_id().with("page_close"), Sense::click());
    let painter = ui.painter();
    let color = if resp.hovered() {
        painter.circle_filled(button.center(), 18.0, HOVER_BG);
        TEXT_STRONG
    } else {
        TEXT_MUTED
    };
    let c = button.center();
    let r = 8.0;
    let stroke = egui::Stroke::new(2.0, color);
    painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
    painter.line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke);
    painter.text(
        egui::pos2(c.x, row.top() + 44.0),
        Align2::CENTER_CENTER,
        "ESC",
        FontId::proportional(11.0),
        TEXT_MUTED,
    );
    resp.clicked()
}

/// One server in the Find servers list. Returns true when its Add button was clicked.
/// One server in the Find servers list. Returns true when its Add button was clicked.
/// What a click on a Find servers row asked for.
enum RowAction {
    None,
    Add,
    Toggle,
}

/// One server in the Find servers list. A click on the row shows its full description; a click
/// on Add saves the server.
fn discovered_row(ui: &mut Ui, found: &FoundServer, saved: bool, expanded: bool) -> RowAction {
    let mut add_rect: Option<egui::Rect> = None;
    let frame = Frame::new()
        .fill(RAIL_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::hover());
                paint_server_icon(
                    ui,
                    rect,
                    &found.name,
                    found.icon.as_deref(),
                    name_color(&found.name),
                    16,
                    20.0,
                );
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    // Leave room on the right for the Add button.
                    ui.set_width(ui.available_width() - 96.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&found.name).strong().color(TEXT_STRONG)).truncate(),
                        );
                        if found.public {
                            ui.add(
                                egui::Image::new(egui::include_image!("../assets/house.svg"))
                                    .fit_to_exact_size(egui::vec2(14.0, 14.0)),
                            )
                            .on_hover_text("Known by other servers");
                        }
                    });
                    if !found.description.is_empty() {
                        let label = egui::Label::new(RichText::new(&found.description).small().color(TEXT));
                        ui.add(if expanded { label.wrap() } else { label.truncate() });
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&found.addr).small().color(TEXT_MUTED));
                        ui.label(RichText::new("·").small().color(TEXT_MUTED));
                        ui.label(RichText::new(format!("{} online", found.online)).small().color(GREEN));
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if saved {
                        ui.label(RichText::new("Added").color(TEXT_MUTED));
                    } else {
                        let button = ui.add(egui::Button::new("Add").fill(ACCENT).min_size(egui::vec2(72.0, 32.0)));
                        add_rect = Some(button.rect);
                    }
                });
            });
        });

    // The whole row is clickable. The row sits on top of the Add button, so a click inside the
    // button's rectangle counts as Add and anywhere else toggles the description.
    let row = ui.interact(
        frame.response.rect,
        ui.scope_id().with(("row", &found.addr)),
        Sense::click(),
    );
    if !row.clicked() {
        return RowAction::None;
    }
    let on_add = match (add_rect, row.interact_pointer_pos()) {
        (Some(rect), Some(pos)) => rect.contains(pos),
        _ => false,
    };
    if on_add {
        RowAction::Add
    } else {
        RowAction::Toggle
    }
}

fn add_server_dialog(app: &mut DissCordApp, ctx: &egui::Context) {
    if !app.show_add_server {
        return;
    }
    let mut close = false;
    let mut submit = false;
    let mut find = false;
    egui::Window::new("Join a Server")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(440.0)
        .frame(Frame::new().fill(SIDEBAR_BG).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(24)))
        .show(ctx, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                close |= close_button(ui);
            });
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Join a Server").size(22.0).strong().color(TEXT_STRONG));
                ui.add_space(4.0);
                ui.label(RichText::new("Enter an address below to join an existing server").color(TEXT));
            });
            ui.add_space(20.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new("Server address").strong().color(TEXT_STRONG));
                ui.label(RichText::new("*").strong().color(RED));
            });
            ui.add_space(4.0);
            let resp = ui.add(
                TextEdit::singleline(&mut app.add_addr)
                    .hint_text("chat.example.com")
                    .margin(Margin::symmetric(10, 10))
                    .desired_width(f32::INFINITY),
            );
            submit |= enter_pressed(ui, &resp);
            ui.add_space(8.0);
            ui.label(RichText::new("Addresses should look like").small().color(TEXT_MUTED));
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                for example in ["chat.example.com", "myserver.example.net"] {
                    Frame::new()
                        .fill(HOVER_BG)
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.label(RichText::new(example).small().color(TEXT));
                        });
                }
            });

            if let Some(err) = &app.add_error {
                ui.add_space(8.0);
                ui.label(RichText::new(err).color(RED));
            }

            ui.add_space(20.0);
            let frame = Frame::new()
                .fill(RAIL_BG)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 10));
            let card = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Don't have an address?").strong().color(TEXT_STRONG));
                        ui.label(
                            RichText::new("Find servers on your network and the internet").small().color(TEXT),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("›").size(20.0).color(TEXT_MUTED));
                    });
                });
            });
            if ui.interact(card.response.rect, ui.scope_id().with("find_servers"), Sense::click()).clicked() {
                find = true;
            }

            ui.add_space(20.0);
            ui.horizontal(|ui| {
                if ui.link("Back").clicked() {
                    close = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = egui::Button::new(RichText::new("Join Server").strong().color(TEXT_STRONG))
                        .fill(ACCENT)
                        .min_size(egui::vec2(132.0, 40.0));
                    if ui.add(button).clicked() {
                        submit = true;
                    }
                });
            });
        });

    if close {
        app.show_add_server = false;
    }
    if find {
        app.show_add_server = false;
        app.open_discover(ctx);
    }
    if submit {
        app.add_server(ctx);
    }
}

/// Full-page server settings, opened from the server menu by the owner.
/// Esc or the close button goes back to the chat.
fn server_settings_page(app: &mut DissCordApp, ui: &mut Ui) {
    let server_name = app.selected_name().unwrap_or("Server").to_string();
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let mut save = false;
    let name = app.rename_input.trim();
    let description = app.description_input.trim();
    let dirty = (!name.is_empty() && name != server_name) || description != app.server_description;

    egui::Panel::left("settings_nav")
        .exact_size(232.0)
        .frame(Frame::new().fill(SIDEBAR_BG).inner_margin(Margin::symmetric(12, 24)))
        .show(ui, |ui| {
            ui.label(RichText::new(server_name.to_uppercase()).small().strong().color(TEXT_MUTED));
            ui.add_space(8.0);
            // Only the profile page exists so far.
            nav_selected(ui, "Server Profile");
        });

    egui::CentralPanel::default()
        .frame(Frame::new().fill(APP_BG).inner_margin(Margin::symmetric(40, 24)))
        .show(ui, |ui| {
            // The close button and its ESC hint sit at the far right, the hint centred under the X.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                close |= close_button(ui);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 14.0), Sense::hover());
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "ESC",
                    FontId::proportional(11.0),
                    TEXT_MUTED,
                );
            });

            ui.label(RichText::new("Server Profile").size(20.0).strong().color(TEXT_STRONG));
            ui.add_space(4.0);
            ui.label(
                RichText::new("Customise how your server appears to its members.").color(TEXT),
            );
            ui.add_space(24.0);

            field_label(ui, "SERVER ICON");
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(96.0, 96.0), Sense::hover());
        paint_server_icon(
            ui,
            rect,
            &server_name,
            app.server_icon.as_deref(),
            name_color(&server_name),
            16,
            36.0,
        );
        ui.add_space(12.0);
        ui.vertical(|ui| {
            if ui.add(egui::Button::new("Upload image").min_size(egui::vec2(140.0, 32.0))).clicked() {
                app.pick_server_icon();
            }
            ui.add_space(4.0);
            if app.server_icon.is_some() && ui.link("Remove icon").clicked() {
                app.remove_server_icon();
            }
            if let Some(err) = &app.icon_error {
                ui.label(RichText::new(err).small().color(RED));
            }
            ui.label(RichText::new("PNG or JPG. It is shrunk to 128x128.").small().color(TEXT_MUTED));
        });
    });
    ui.add_space(20.0);

    field_label(ui, "NAME");
            let resp = ui.add(
                TextEdit::singleline(&mut app.rename_input)
                    .hint_text("My server")
                    .desired_width(ui.available_width().min(560.0)),
            );
            save |= enter_pressed(ui, &resp);

            ui.add_space(20.0);
            field_label(ui, "DESCRIPTION");
            ui.label(
                RichText::new("Tell people what this server is about.").small().color(TEXT_MUTED),
            );
            ui.add_space(4.0);
            ui.add(
                TextEdit::multiline(&mut app.description_input)
                    .hint_text("Welcome to our community!")
                    .char_limit(common::MAX_SERVER_DESCRIPTION_LEN)
                    .desired_rows(4)
                    .desired_width(ui.available_width().min(560.0)),
            );

        });

    // Floating bar over the bottom of the page, shown only while there are unsaved edits.
    if dirty {
        egui::Area::new(ui.scope_id().with("unsaved_changes"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(116.0, -24.0))
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(RAIL_BG)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_max_width(640.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Careful – you have unsaved changes!")
                                    .strong()
                                    .color(TEXT_STRONG),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let button = egui::Button::new(
                                    RichText::new("Save Changes").strong().color(TEXT_STRONG),
                                )
                                .fill(GREEN)
                                .min_size(egui::vec2(0.0, 32.0));
                                save |= ui.add(button).clicked();
                                if ui.link("Reset").clicked() {
                                    app.rename_input = server_name.clone();
                                    app.description_input = app.server_description.clone();
                                }
                            });
                        });
                    });
            });
    }

    if save {
        app.update_server();
    }
    if close {
        app.show_server_settings = false;
    }
}

/// A selected row in the settings navigation.
fn nav_selected(ui: &mut Ui, label: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), SELECTED_BG);
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        TEXT_STRONG,
    );
}

/// The round X in the top-right of the settings page. Returns true when clicked.
fn close_button(ui: &mut Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), Sense::click());
    let color = if resp.hovered() {
        ui.painter().circle_filled(rect.center(), 18.0, HOVER_BG);
        TEXT_STRONG
    } else {
        TEXT_MUTED
    };
    let c = rect.center();
    let r = 8.0;
    let stroke = egui::Stroke::new(2.0, color);
    ui.painter().line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
    ui.painter().line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke);
    resp.clicked()
}

fn chat(app: &mut DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    // Order matters: side panels first, then top/bottom, then the centre.
    egui::Panel::left("channels")
        .default_size(250.0)
        .min_size(250.0)
        .max_size(420.0)
        .resizable(true)
        .frame(Frame::new().fill(SIDEBAR_BG))
        .show(ui, |ui| channel_sidebar(app, ui, actions));

    egui::Panel::top("header")
        .exact_size(48.0)
        .frame(Frame::new().fill(APP_BG))
        .show(ui, |ui| header(app, ui, actions));

    if !app.members_hidden {
        egui::Panel::right("members")
            .default_size(250.0)
            .min_size(250.0)
            .max_size(420.0)
            .resizable(true)
            .frame(Frame::new().fill(SIDEBAR_BG))
            .show(ui, |ui| member_list(app, ui, actions));
    }

    egui::Panel::bottom("composer")
        // No separator line here: it would run under the member list.
        .show_separator_line(false)
        .frame(Frame::new().fill(APP_BG).inner_margin(Margin::symmetric(16, 12)))
        .show(ui, |ui| composer(app, ui));

    egui::CentralPanel::default()
        .frame(Frame::new().fill(APP_BG))
        .show(ui, |ui| messages(app, ui, actions));
}

fn channel_sidebar(app: &mut DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    egui::Panel::bottom("user_bar")
        .exact_size(56.0)
        .frame(Frame::new().fill(SIDEBAR_BG).inner_margin(Margin::symmetric(8, 8)))
        .show(ui, |ui| user_bar(app, ui, actions));

    let title = app.selected_name().unwrap_or("DissCord").to_string();
    server_header(app, ui, &title, actions);

    let mut to_join: Option<String> = None;
    let mut create = false;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                // Clicking the arrow or the heading folds the channel list away or opens it again.
                let (rect, arrow) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::click());
                let arrow_icon = if app.channels_collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
                paint_icon(ui.painter(), rect.center(), arrow_icon, TEXT_MUTED);
                let heading = ui.add(
                    egui::Label::new(RichText::new("TEXT CHANNELS").small().strong().color(TEXT_MUTED))
                        .sense(Sense::click()),
                );
                if arrow.clicked() || heading.clicked() {
                    app.channels_collapsed = !app.channels_collapsed;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    if icon_button(ui, Icon::Plus, 20.0, false).on_hover_text("Create channel").clicked() {
                        actions.push(MenuAction::CreateChannel);
                    }
                });
            });
            ui.add_space(4.0);

            for name in &app.channels {
                let selected = *name == app.channel;
                // When folded, only the channel you are in stays visible.
                if app.channels_collapsed && !selected {
                    continue;
                }
                let unread = app.unread.contains(name);
                if channel_row(ui, name, selected, unread, actions) && !selected {
                    to_join = Some(name.clone());
                }
            }

            // Hidden unless the setting is on or "+" opened it.
            if app.show_create_box || app.create_box_open {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let resp = ui.add(
                        TextEdit::singleline(&mut app.new_channel)
                            .hint_text("Create channel")
                            .desired_width(ui.available_width() - 8.0),
                    );
                    if app.focus_create {
                        resp.request_focus();
                        app.focus_create = false;
                    }
                    if resp.lost_focus() {
                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            create = true;
                        } else {
                            app.create_box_open = false;
                        }
                    }
                });
            }
        });

    if let Some(name) = to_join {
        app.join(&name);
    }
    if create {
        app.create_channel();
    }
}

/// One clickable channel in the sidebar. Returns true when it was clicked.
/// Right-click, or the gear that shows on hover, opens the channel menu.
fn channel_row(
    ui: &mut Ui,
    name: &str,
    selected: bool,
    unread: bool,
    actions: &mut Vec<MenuAction>,
) -> bool {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 32.0), Sense::click());
    let rect = rect.shrink2(egui::vec2(8.0, 1.0));
    let hovered = resp.hovered();

    let bg = if selected {
        SELECTED_BG
    } else if hovered {
        HOVER_BG
    } else {
        Color32::TRANSPARENT
    };
    let color = if selected || unread {
        TEXT_STRONG
    } else if hovered {
        TEXT
    } else {
        TEXT_MUTED
    };

    // The gear sits on top of the row, so clicking it opens the menu instead of joining.
    let gear_rect = egui::Rect::from_center_size(rect.right_center() - egui::vec2(14.0, 0.0), egui::vec2(20.0, 20.0));
    let gear = ui.interact(gear_rect, ui.scope_id().with(("gear", name)), Sense::click());

    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), bg);
    painter.text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        format!("# {name}"),
        FontId::proportional(16.0),
        color,
    );
    if selected || hovered {
        paint_icon(painter, gear_rect.center(), Icon::Gear, TEXT_MUTED);
    } else if unread {
        painter.circle_filled(rect.right_center() - egui::vec2(12.0, 0.0), 4.0, TEXT_STRONG);
    }

    egui::Popup::menu(&gear).show(|ui| channel_menu_items(ui, name, unread, actions));
    resp.context_menu(|ui| channel_menu_items(ui, name, unread, actions));

    resp.clicked()
}

/// Small line icons, drawn by hand so no icon font is needed.
#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Threads,
    Bell,
    Pin,
    Members,
    Search,
    Mic,
    Headset,
    Gear,
    Plus,
    Gift,
    Gif,
    Sticker,
    Emoji,
    Apps,
    ChevronDown,
    ChevronRight,
    Explore,
}

/// Tooltip for buttons that exist visually but have no feature behind them yet.
const NOT_YET_TIP: &str = "Not available yet";
/// Tooltip for the microphone and headset buttons. Voice chat is not built yet.
const VOICE_TIP: &str = "Voice chat isn't available yet";

/// Draws `icon` centred on `c`, about 16 px across.
fn paint_icon(painter: &egui::Painter, c: egui::Pos2, icon: Icon, color: Color32) {
    use std::f32::consts::TAU;
    let stroke = egui::Stroke::new(1.6, color);
    let p = |x: f32, y: f32| c + egui::vec2(x, y);
    match icon {
        Icon::Threads => {
            let bubble = egui::Rect::from_center_size(c, egui::vec2(16.0, 13.0));
            painter.rect_stroke(bubble, CornerRadius::same(4), stroke, egui::StrokeKind::Inside);
            painter.text(c, Align2::CENTER_CENTER, "#", FontId::proportional(13.0), color);
        }
        Icon::Bell => {
            let bell = egui::Rect::from_min_max(p(-6.0, -7.0), p(6.0, 4.0));
            painter.rect_stroke(bell, CornerRadius::same(6), stroke, egui::StrokeKind::Inside);
            painter.line_segment([p(-8.0, 4.0), p(8.0, 4.0)], stroke);
            painter.circle_filled(p(0.0, 7.0), 2.0, color);
        }
        Icon::Pin => {
            painter.circle_filled(p(3.0, -3.0), 3.5, color);
            painter.line_segment([p(1.0, -1.0), p(-5.0, 5.0)], egui::Stroke::new(2.0, color));
        }
        Icon::Members => {
            painter.circle_stroke(p(-3.0, -3.0), 3.0, stroke);
            painter.circle_stroke(p(4.0, -2.0), 2.5, stroke);
            let left = egui::Rect::from_min_max(p(-8.0, 1.0), p(2.0, 7.0));
            let right = egui::Rect::from_min_max(p(1.0, 1.0), p(8.0, 7.0));
            painter.rect_stroke(left, CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(right, CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
        }
        Icon::Search => {
            painter.circle_stroke(p(-1.0, -1.0), 5.5, stroke);
            painter.line_segment([p(3.0, 3.0), p(7.0, 7.0)], egui::Stroke::new(2.0, color));
        }
        Icon::Mic => {
            let capsule = egui::Rect::from_min_max(p(-3.0, -7.0), p(3.0, 1.0));
            painter.rect_filled(capsule, CornerRadius::same(3), color);
            let cradle = egui::Rect::from_min_max(p(-5.0, -4.0), p(5.0, 4.0));
            painter.rect_stroke(cradle, CornerRadius::same(4), stroke, egui::StrokeKind::Inside);
            painter.line_segment([p(0.0, 4.0), p(0.0, 7.0)], stroke);
            painter.line_segment([p(-4.0, 7.0), p(4.0, 7.0)], stroke);
        }
        Icon::Headset => {
            let band = egui::Rect::from_min_max(p(-7.0, -7.0), p(7.0, 2.0));
            painter.rect_stroke(band, CornerRadius::same(7), stroke, egui::StrokeKind::Inside);
            painter.rect_filled(egui::Rect::from_min_max(p(-8.0, 0.0), p(-4.0, 6.0)), CornerRadius::same(2), color);
            painter.rect_filled(egui::Rect::from_min_max(p(4.0, 0.0), p(8.0, 6.0)), CornerRadius::same(2), color);
        }
        Icon::Gear => {
            for k in 0..8 {
                let a = k as f32 * TAU / 8.0;
                let dir = egui::vec2(a.cos(), a.sin());
                painter.line_segment([c + dir * 5.5, c + dir * 8.0], egui::Stroke::new(2.5, color));
            }
            painter.circle_stroke(c, 5.5, stroke);
        }
        Icon::Plus => {
            let thick = egui::Stroke::new(2.0, color);
            painter.line_segment([p(-6.0, 0.0), p(6.0, 0.0)], thick);
            painter.line_segment([p(0.0, -6.0), p(0.0, 6.0)], thick);
        }
        Icon::Gift => {
            let box_rect = egui::Rect::from_min_max(p(-7.0, -2.0), p(7.0, 7.0));
            painter.rect_stroke(box_rect, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            painter.line_segment([p(-8.0, -4.0), p(8.0, -4.0)], stroke);
            painter.line_segment([p(0.0, -4.0), p(0.0, 7.0)], stroke);
        }
        Icon::Gif => {
            painter.text(c, Align2::CENTER_CENTER, "GIF", FontId::proportional(9.0), color);
        }
        Icon::Sticker => {
            let face = egui::Rect::from_center_size(c, egui::vec2(15.0, 15.0));
            painter.rect_stroke(face, CornerRadius::same(4), stroke, egui::StrokeKind::Inside);
            painter.circle_filled(p(-3.0, -2.0), 1.3, color);
            painter.circle_filled(p(3.0, -2.0), 1.3, color);
        }
        Icon::Emoji => {
            painter.circle_stroke(c, 7.5, stroke);
            painter.circle_filled(p(-3.0, -2.0), 1.3, color);
            painter.circle_filled(p(3.0, -2.0), 1.3, color);
            let smile = vec![p(-3.5, 2.0), p(-1.5, 4.0), p(1.5, 4.0), p(3.5, 2.0)];
            painter.add(egui::Shape::line(smile, stroke));
        }
        Icon::Apps => {
            for (x, y) in [(-6.0, -6.0), (1.0, -6.0), (-6.0, 1.0), (1.0, 1.0)] {
                let tile = egui::Rect::from_min_size(p(x, y), egui::vec2(5.0, 5.0));
                painter.rect_stroke(tile, CornerRadius::same(1), stroke, egui::StrokeKind::Inside);
            }
        }
        Icon::ChevronDown => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(-4.0, -2.0), p(4.0, -2.0), p(0.0, 3.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
        Icon::Explore => {
            painter.circle_stroke(c, 7.5, stroke);
            let needle = vec![p(0.0, -5.0), p(2.5, 0.0), p(0.0, 5.0), p(-2.5, 0.0)];
            painter.add(egui::Shape::convex_polygon(needle, color, egui::Stroke::NONE));
        }
        Icon::ChevronRight => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(-2.0, -4.0), p(3.0, 0.0), p(-2.0, 4.0)],
                color,
                egui::Stroke::NONE,
            ));
        }
    }
}

/// A square icon button that highlights on hover. Returns its response.
fn icon_button(ui: &mut Ui, icon: Icon, size: f32, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::click());
    let color = if active || resp.hovered() { TEXT_STRONG } else { TEXT_MUTED };
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), HOVER_BG);
    }
    paint_icon(ui.painter(), rect.center(), icon, color);
    resp
}

/// Lays out one message row and highlights it while the pointer is over it.
fn hoverable(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let background = ui.painter().add(egui::Shape::Noop);
    let rect = ui.scope(add).response.rect;
    if ui.rect_contains_pointer(rect) {
        let full = egui::Rect::from_min_max(
            egui::pos2(ui.max_rect().left(), rect.top()),
            egui::pos2(ui.max_rect().right(), rect.bottom()),
        );
        ui.painter().set(background, egui::Shape::rect_filled(full, CornerRadius::ZERO, HOVER_BG));
    }
}

/// A full-width row with a short label in the middle, between two thin lines.
fn divider_label(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), Sense::hover());
    let painter = ui.painter();
    let label = painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.0),
        TEXT_MUTED,
    );
    let line = egui::Stroke::new(1.0, SELECTED_BG);
    let y = rect.center().y;
    painter.hline(rect.left() + 16.0..=(label.left() - 8.0).max(rect.left() + 16.0), y, line);
    painter.hline(rect.right() - 16.0..=(label.right() + 8.0).min(rect.right() - 16.0), y, line);
}

/// The red "NEW" line marking where messages arrived after you last left the channel.
fn new_messages_divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), Sense::hover());
    let painter = ui.painter();
    let pill = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 46.0, rect.center().y - 8.0),
        egui::vec2(36.0, 16.0),
    );
    painter.hline(rect.left() + 16.0..=(pill.left() - 6.0), rect.center().y, egui::Stroke::new(1.0, RED));
    painter.rect_filled(pill, CornerRadius::same(4), RED);
    painter.text(pill.center(), Align2::CENTER_CENTER, "NEW", FontId::proportional(10.0), TEXT_STRONG);
}

/// Items in a user's menu (the gear under the name in the user bar).
fn user_menu_items(ui: &mut Ui, app: &DissCordApp, actions: &mut Vec<MenuAction>) {
    if menu_item(ui, "Copy my name", false) {
        actions.push(MenuAction::CopyText(app.user.clone()));
    }
    let box_label = if app.show_create_box {
        "Hide channel create box"
    } else {
        "Show channel create box"
    };
    if menu_item(ui, box_label, false) {
        actions.push(MenuAction::ToggleCreateBox);
    }
    menu_divider(ui);
    if menu_item(ui, "Log out", true) {
        actions.push(MenuAction::Logout);
    }
}

/// Items in a channel's menu (right-click, or the gear on its row).
fn channel_menu_items(ui: &mut Ui, name: &str, unread: bool, actions: &mut Vec<MenuAction>) {
    if unread && menu_item(ui, "Mark as read", false) {
        actions.push(MenuAction::MarkRead(name.to_string()));
    }
    if menu_item(ui, "Copy channel name", false) {
        actions.push(MenuAction::CopyText(format!("#{name}")));
    }
}

/// True when a message or its author contains the search text (already lowercased).
fn matches_query(entry: &Entry, query: &str) -> bool {
    match entry {
        Entry::Chat(m) => {
            m.text.to_lowercase().contains(query) || m.author.to_lowercase().contains(query)
        }
        Entry::System { .. } => false,
    }
}

/// Avatar, name, and the mic, headset and gear buttons at the bottom of the channel sidebar.
/// Your avatar, name and status in a rounded box at the bottom of the channel sidebar, with the
/// mic and headset (each with a chevron) and the settings gear on the right.
fn user_bar(app: &DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    // Room kept free on the right for the mic, headset, chevrons and gear.
    const ICON_BLOCK_WIDTH: f32 = 130.0;
    Frame::new()
        .fill(USER_BAR_BG)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                avatar(ui, &app.user, 32.0, Some(GREEN));
                ui.add_space(4.0);
                let name_width = (ui.available_width() - ICON_BLOCK_WIDTH).max(40.0);
                ui.vertical(|ui| {
                    ui.set_width(name_width);
                    ui.add(egui::Label::new(RichText::new(&app.user).strong().color(TEXT_STRONG)).truncate());
                    ui.label(RichText::new("Online").small().color(TEXT_MUTED));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let gear = icon_button(ui, Icon::Gear, 28.0, false).on_hover_text("User settings");
                    egui::Popup::menu(&gear).show(|ui| user_menu_items(ui, app, actions));
                    icon_button(ui, Icon::ChevronDown, 16.0, false).on_hover_text(VOICE_TIP);
                    icon_button(ui, Icon::Headset, 28.0, false).on_hover_text(VOICE_TIP);
                    icon_button(ui, Icon::ChevronDown, 16.0, false).on_hover_text(VOICE_TIP);
                    icon_button(ui, Icon::Mic, 28.0, false).on_hover_text(VOICE_TIP);
                });
            });
        });
}

/// The right-hand member list: the owner first, then everyone else online, then offline.
fn member_list(app: &DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(12.0);
            let owner = app.owner.as_deref();
            let is_owner = |name: &str| Some(name) == owner;

            if let Some(name) = owner {
                member_heading(ui, "OWNER", 1);
                let online = app.online.iter().any(|n| n == name);
                member_row(ui, app, name, online, actions);
                ui.add_space(12.0);
            }

            let online: Vec<&String> = app.online.iter().filter(|n| !is_owner(n)).collect();
            member_heading(ui, "ONLINE", online.len());
            for name in online {
                member_row(ui, app, name, true, actions);
            }

            let offline: Vec<&String> = app.offline.iter().filter(|n| !is_owner(n)).collect();
            ui.add_space(12.0);
            member_heading(ui, "OFFLINE", offline.len());
            for name in offline {
                member_row(ui, app, name, false, actions);
            }
        });
}

fn member_heading(ui: &mut Ui, title: &str, count: usize) {
    ui.horizontal(|ui| {
        ui.add_space(8.0); // same left edge as the member avatars
        ui.label(
            RichText::new(format!("{title} — {count}"))
                .small()
                .strong()
                .color(TEXT_MUTED),
        );
    });
    ui.add_space(4.0);
}

/// One member. Online members get a green dot and bright name; offline members are dimmed.
/// The row lights up when the pointer is over it.
/// One member. Online members get a green dot and bright name; offline members are dimmed.
/// One member. Online members get a green dot and bright name; offline members are dimmed.
/// The row lights up when the pointer is over it.
fn member_row(ui: &mut Ui, app: &DissCordApp, name: &str, online: bool, actions: &mut Vec<MenuAction>) {
    hoverable(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let status = if online { Some(GREEN) } else { None };
            avatar(ui, name, 32.0, status);
            ui.add_space(4.0);
            let color = match (online, name == app.user) {
                (true, true) => TEXT_STRONG,
                (true, false) => TEXT,
                (false, _) => TEXT_MUTED,
            };
            user_name(ui, name, RichText::new(name).color(color), actions);
            if app.owner.as_deref() == Some(name) {
                owner_crown(ui);
            }
        });
    });
    ui.add_space(2.0);
}

/// Crown shown next to the server owner (SVG, see `client/assets/crown.svg`).
fn owner_crown(ui: &mut Ui) {
    ui.add(
        egui::Image::new(egui::include_image!("../assets/crown.svg"))
            .fit_to_exact_size(egui::vec2(8.0, 6.0)),
    );
}

/// The channel name bar at the top of the chat, with its icons and a search box.
fn header(app: &mut DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    let hint = format!("Search {}", app.selected_name().unwrap_or("DissCord"));
    // One row, centred vertically in the bar: the channel name on the left, icons and search box on the right.
    ui.horizontal_centered(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new("#").size(22.0).color(TEXT_MUTED));
        ui.label(RichText::new(&app.channel).strong().size(16.0).color(TEXT_STRONG));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(12.0);
            Frame::new()
                .fill(RAIL_BG)
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.set_width(240.0);
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut app.search)
                                .frame(Frame::NONE)
                                .hint_text(hint)
                                .desired_width(206.0),
                        );
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::hover());
                        paint_icon(ui.painter(), rect.center(), Icon::Search, TEXT_MUTED);
                    });
                });
            ui.add_space(8.0);
            let toggle_tip = if app.members_hidden { "Show member list" } else { "Hide member list" };
            if icon_button(ui, Icon::Members, 32.0, !app.members_hidden).on_hover_text(toggle_tip).clicked() {
                actions.push(MenuAction::ToggleMembers);
            }
            icon_button(ui, Icon::Pin, 32.0, false).on_hover_text(NOT_YET_TIP);
            icon_button(ui, Icon::Bell, 32.0, false).on_hover_text(NOT_YET_TIP);
            icon_button(ui, Icon::Threads, 32.0, false).on_hover_text(NOT_YET_TIP);
        });
    });
}

/// The message box with the "+" on the left and gift, GIF, sticker and emoji buttons inside.
fn composer(app: &mut DissCordApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        icon_button(ui, Icon::Plus, 40.0, false).on_hover_text(NOT_YET_TIP);
        let box_width = ui.available_width() - 48.0;
        Frame::new()
            .fill(INPUT_BG)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                let inner = box_width - 24.0;
                ui.set_width(inner);
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        TextEdit::singleline(&mut app.input)
                            .frame(Frame::NONE)
                            .hint_text(format!("Message #{}", app.channel))
                            .desired_width(inner - 4.0 * 28.0),
                    );
                    if app.focus_input {
                        resp.request_focus();
                        app.focus_input = false;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        app.send_input();
                        resp.request_focus();
                    }
                    for icon in [Icon::Gift, Icon::Gif, Icon::Sticker, Icon::Emoji] {
                        icon_button(ui, icon, 24.0, false).on_hover_text(NOT_YET_TIP);
                    }
                });
            });
        icon_button(ui, Icon::Apps, 40.0, false).on_hover_text(NOT_YET_TIP);
    });
    if app.status.starts_with("Error") {
        ui.label(RichText::new(&app.status).small().color(RED));
    }
}

fn messages(app: &DissCordApp, ui: &mut Ui, actions: &mut Vec<MenuAction>) {
    let channel = app.channel.clone();
    let entries: &[Entry] = app.messages.get(&channel).map(Vec::as_slice).unwrap_or(&[]);
    let query = app.search.trim().to_lowercase();
    // Where the red "NEW" line goes: the first message that arrived after you left this channel.
    let new_at = app.read_marks.get(&channel).copied();

    egui::ScrollArea::vertical()
        .id_salt(("messages", &channel))
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.add_space(16.0);
            welcome(ui, &channel);

            if !query.is_empty() && !entries.iter().any(|e| matches_query(e, &query)) {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(RichText::new("No messages match your search.").color(TEXT_MUTED));
                });
            }

            let mut prev_day: Option<NaiveDate> = None;
            let mut prev_author: Option<&str> = None;
            let mut prev_at: u64 = 0;

            for (i, entry) in entries.iter().enumerate() {
                if !query.is_empty() && !matches_query(entry, &query) {
                    continue;
                }
                if query.is_empty() && new_at == Some(i) {
                    new_messages_divider(ui);
                    prev_author = None;
                }

                let at = match entry {
                    Entry::Chat(m) => m.timestamp,
                    Entry::System { at, .. } => *at,
                };

                let day = day_key(at);
                if prev_day != Some(day) {
                    day_divider(ui, at);
                    prev_author = None;
                    prev_day = Some(day);
                }

                match entry {
                    Entry::Chat(m) => {
                        let grouped = prev_author == Some(m.author.as_str())
                            && at.saturating_sub(prev_at) < GROUP_GAP_SECS;
                        if grouped {
                            compact_message(ui, app, m, actions);
                        } else {
                            full_message(ui, app, m, actions);
                        }
                        prev_author = Some(m.author.as_str());
                    }
                    Entry::System { text, .. } => {
                        system_notice(ui, text);
                        prev_author = None;
                    }
                }
                prev_at = at;
            }
            ui.add_space(12.0);
        });
}

fn welcome(ui: &mut Ui, channel: &str) {
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(format!("Welcome to #{channel}!"))
                    .size(28.0)
                    .strong()
                    .color(TEXT_STRONG),
            );
            ui.label(RichText::new(format!("This is the start of the #{channel} channel.")).color(TEXT_MUTED));
        });
    });
    ui.add_space(16.0);
}

/// The date between days of messages: a label centred on a thin line.
fn day_divider(ui: &mut Ui, at: u64) {
    ui.add_space(12.0);
    divider_label(ui, &day_label(at));
    ui.add_space(8.0);
}

/// A message with avatar, author, and timestamp. Starts a new group.
fn full_message(ui: &mut Ui, app: &DissCordApp, m: &ChatMessage, actions: &mut Vec<MenuAction>) {
    ui.add_space(12.0);
    hoverable(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            avatar(ui, &m.author, 40.0, None);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    user_name(
                        ui,
                        &m.author,
                        RichText::new(&m.author).strong().color(name_color(&m.author)),
                        actions,
                    );
                    ui.label(RichText::new(stamp_label(m.timestamp)).small().color(TEXT_MUTED));
                });
                message_text(ui, app, m, actions);
            });
        });
    });
}

/// A follow-up message from the same author, indented under the group header.
fn compact_message(ui: &mut Ui, app: &DissCordApp, m: &ChatMessage, actions: &mut Vec<MenuAction>) {
    ui.add_space(2.0);
    hoverable(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add_space(AVATAR_INDENT + 16.0);
            ui.vertical(|ui| {
                message_text(ui, app, m, actions);
            });
        });
    });
}

/// The text of a message. Your own messages can be edited from their right-click menu. While a
/// message is being edited, its text becomes a box: Enter saves, Escape cancels.
fn message_text(ui: &mut Ui, app: &DissCordApp, m: &ChatMessage, actions: &mut Vec<MenuAction>) {
    if m.id != 0 && app.editing == Some(m.id) {
        edit_box(ui, m.id, actions);
        return;
    }
    let resp = ui
        .horizontal(|ui| {
            let resp = ui.add(egui::Label::new(RichText::new(&m.text).color(TEXT)).sense(Sense::click()));
            if m.edited {
                ui.label(RichText::new("(edited)").small().color(TEXT_MUTED));
            }
            resp
        })
        .inner
        .on_hover_text(time_label(m.timestamp));
    if m.id != 0 && m.author == app.user {
        resp.context_menu(|ui| {
            if ui.button("Edit message").clicked() {
                actions.push(MenuAction::StartEdit { id: m.id, text: m.text.clone() });
                ui.close();
            }
            if ui.button("Delete message").clicked() {
                actions.push(MenuAction::Delete(m.id));
                ui.close();
            }
        });
    }
}

/// The edit box for one message. Its text lives in egui's temporary data, keyed by the message id.
fn edit_box(ui: &mut Ui, id: i64, actions: &mut Vec<MenuAction>) {
    let buf_id = crate::app::edit_buffer(id);
    let mut text = ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_default();
    let resp = ui.add(
        egui::TextEdit::singleline(&mut text)
            .id(buf_id)
            .desired_width(f32::INFINITY),
    );
    ui.data_mut(|d| d.insert_temp(buf_id, text));
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        actions.push(MenuAction::CancelEdit);
    } else if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        actions.push(MenuAction::SaveEdit(id));
    }
}

fn system_notice(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new(text).italics().color(TEXT_MUTED));
    });
}

/// A user's name. Right-click opens the user menu (mention or copy the name).
fn user_name(ui: &mut Ui, name: &str, text: RichText, actions: &mut Vec<MenuAction>) {
    let resp = ui.add(egui::Label::new(text).sense(Sense::click()));
    resp.context_menu(|ui| {
        if ui.button("Mention").clicked() {
            actions.push(MenuAction::Mention(name.to_string()));
        }
        if ui.button("Copy name").clicked() {
            actions.push(MenuAction::CopyText(name.to_string()));
        }
    });
}

/// Coloured circle with the first letter of the name.
fn avatar(ui: &mut Ui, name: &str, size: f32, status: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), size / 2.0, name_color(name));

    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        FontId::proportional(size * 0.45),
        TEXT_STRONG,
    );

    if let Some(color) = status {
        let dot = rect.right_bottom() - egui::vec2(size * 0.18, size * 0.18);
        painter.circle_filled(dot, size * 0.17, SIDEBAR_BG);
        painter.circle_filled(dot, size * 0.12, color);
    }
}

/// True when Enter was pressed while this text field had focus.
fn enter_pressed(ui: &Ui, resp: &Response) -> bool {
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).small().strong().color(TEXT_MUTED));
    });
}
