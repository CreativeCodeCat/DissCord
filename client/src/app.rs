//! Application state and the logic that reacts to server events and user actions.
//! Drawing lives in `ui.rs`.
//!
//! There is one account for the whole client. You log in (or register) first;
//! then you add servers, and each server is joined with that account's session token.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

use common::{is_valid_email, AuthReply, AuthRequest, ChatMessage, ClientMsg, ServerMsg, DEFAULT_ADDR};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::auth_client;
use crate::discover::{self, FoundServer};
use crate::net::{self, Connection, NetEvent};
use crate::util::now_secs;

/// Key under which the saved server list is stored by eframe.
pub const SERVERS_KEY: &str = "disscord_servers";
/// Key under which the "show channel create box" setting is stored by eframe.
pub const CREATE_BOX_KEY: &str = "disscord_show_create_box";
const MIN_PASSWORD_LEN: usize = 8;

/// A server the user added with the "+" button. Saved between runs.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedServer {
    pub name: String,
    pub addr: String,
    /// The server's icon as a base64 PNG, remembered so the rail can show it.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Resizes an image file to a 128x128 PNG and returns it base64-encoded.
fn icon_png_base64(path: &std::path::Path) -> Result<String, String> {
    use base64::Engine;
    let img = image::open(path).map_err(|e| format!("Could not read that image: {e}"))?;
    let img = img.resize_to_fill(128, 128, image::imageops::FilterType::Lanczos3);
    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| format!("Could not convert that image: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    if encoded.len() > common::MAX_SERVER_ICON_LEN {
        return Err("That image is too large even after shrinking it.".into());
    }
    Ok(encoded)
}

/// Key under which the saved login is stored by eframe.
pub const SESSION_KEY: &str = "disscord_session";

/// A login remembered between runs. The password is never kept. The token is kept in the
/// system keychain, so it is not written to the data file (`skip_serializing`), but an older
/// data file that still has it is read.
#[derive(Clone, Serialize, Deserialize)]
pub struct SavedSession {
    #[serde(default, skip_serializing)]
    pub token: String,
    pub name: String,
    /// Address of the server that was open, so it can be reopened automatically.
    pub server: Option<String>,
}

/// The logged-in account. The token is held in memory and, if "keep me logged in" is on,
/// also saved to disk.
pub struct Session {
    pub token: String,
}

/// The port used when an address is typed without one.
const DEFAULT_PORT: u16 = 7878;

/// Where the edit box keeps its text for a message.
pub fn edit_buffer(id: i64) -> egui::Id {
    egui::Id::NULL.with(("edit_buffer", id))
}

/// One entry in a channel's message list.
pub enum Entry {
    Chat(ChatMessage),
    /// Join/connect notices. `at` is the local time we received them.
    System { text: String, at: u64 },
}

/// Something a menu or button asked for. Drawing code queues these while it
/// runs; the app applies them once the frame is finished.
pub enum MenuAction {
    StartEdit { id: i64, text: String },
    /// Ask the user to confirm deleting a message.
    AskDelete(i64),
    Delete(i64),
    React { id: i64, emoji: String },
    SaveEdit(i64),
    CancelEdit,
    Mention(String),
    CopyText(String),
    MarkRead(String),
    CreateChannel,
    ToggleCreateBox,
    ToggleMembers,
    Logout,
    SelectServer(usize),
    RemoveServer(usize),
    OpenAddServer,
    OpenServerSettings,
    OpenDiscover,
    RetryServer,
    /// The server's certificate changed, and the user trusts the new one.
    TrustNewServerCert,
}

#[derive(Default)]
pub struct DissCordApp {
    // The account
    pub session: Option<Session>,
    pub register_mode: bool,
    pub login_name: String,
    pub login_email: String,
    pub login_password: String,
    pub login_error: Option<String>,
    /// "Keep me logged in on this computer" is ticked.
    pub remember: bool,
    /// A login loaded from disk, waiting to be checked with the auth service.
    pub saved_session: Option<SavedSession>,

    // Saved servers and the one currently selected in the rail
    pub servers: Vec<SavedServer>,
    pub selected: Option<usize>,

    // "Add a server" dialog
    pub show_add_server: bool,
    pub add_addr: String,
    pub add_error: Option<String>,

    // "Find servers" window
    pub show_discover: bool,
    pub discovered: Vec<FoundServer>,
    pub discover_rx: Option<Receiver<FoundServer>>,
    pub discover_error: Option<String>,
    /// Addresses of rows whose description is shown in full.
    pub discover_expanded: HashSet<String>,

    // "Server settings" dialog (owner only)
    pub show_server_settings: bool,
    pub rename_input: String,
    pub description_input: String,
    /// The description the server last sent us.
    pub server_description: String,
    /// The icon the server last sent us, as a base64 PNG.
    pub server_icon: Option<String>,
    /// True when other servers know the open server.
    pub server_public: bool,
    /// Why the last icon change failed, shown in Server Settings.
    pub icon_error: Option<String>,

    // Connection to the selected server
    pub conn: Option<Connection>,
    /// True once the server has accepted our session token.
    pub welcomed: bool,
    /// Why the selected server could not be joined, or why we were disconnected.
    pub server_error: Option<String>,
    /// Address we connected to, shown by the server menu.
    pub connected_addr: String,

    // Content of the selected server
    pub user: String,
    pub channel: String,
    pub channels: Vec<String>,
    /// Names of members currently online, in the order the server sent them.
    pub online: Vec<String>,
    /// Names of members who are not connected right now.
    pub offline: Vec<String>,
    /// The owner of the open server (its first member).
    pub owner: Option<String>,
    /// Channels that received messages while we were not looking at them.
    pub unread: HashSet<String>,
    /// For each channel, how many messages it had when we last left it. Messages after this
    /// index are new since then, and get the red "NEW" line.
    pub read_marks: HashMap<String, usize>,
    /// Text in the header's search box. Filters the messages shown.
    pub search: String,
    /// The message being edited, by id. Its text is kept in egui's temporary data.
    pub editing: Option<i64>,
    /// A message waiting for the user to confirm its deletion.
    pub pending_delete: Option<i64>,
    pub messages: HashMap<String, Vec<Entry>>,

    // Inputs
    pub input: String,
    pub new_channel: String,
    pub focus_input: bool,
    pub focus_create: bool,

    /// Hides the right-hand member list.
    pub members_hidden: bool,
    /// Setting: keep the "Create channel" box visible in the channel sidebar. Saved between runs.
    pub show_create_box: bool,
    /// The text channel list is folded away (the "TEXT CHANNELS" heading was clicked).
    pub channels_collapsed: bool,
    /// The box was opened with "+" and stays open until a channel is created or the box loses focus.
    pub create_box_open: bool,

    /// Last notice, shown in the composer footer.
    pub status: String,
    /// True while the auth service cannot be reached. The top of the window shows a warning.
    pub auth_down: bool,
    /// The user closed the auth warning. It comes back the next time auth is down.
    pub auth_banner_closed: bool,
}

impl DissCordApp {
    /// Checks a saved login with the auth service and logs in if it is still valid.
    /// Called once at startup. If the auth service cannot be reached, the saved login is used
    /// anyway, so the chat server can still accept it.
    pub fn try_auto_login(&mut self, ctx: &egui::Context) {
        let Some(saved) = self.saved_session.take() else {
            return;
        };
        match auth_client::request(&AuthRequest::Verify { token: saved.token.clone() }) {
            Ok(AuthReply::Verified { name }) => {
                self.auth_down = false;
                self.user = name;
                self.session = Some(Session { token: saved.token });
                self.remember = true;
                self.reopen_server(saved.server.as_deref(), ctx);
            }
            Ok(AuthReply::Error { text }) => {
                self.login_error = Some(format!("Your saved login has ended: {text}"));
            }
            Ok(_) => {
                self.login_error = Some("Unexpected reply from the auth service.".into());
                self.saved_session = Some(saved);
            }
            Err(e) => {
                // The auth service cannot be reached, so the saved login is used as it is. The chat
                // server accepts it from its own record of recent logins, or says why it cannot.
                self.auth_down = true;
                self.user = saved.name.clone();
                self.session = Some(Session { token: saved.token.clone() });
                self.remember = true;
                self.status = format!("Auth service unreachable ({e}). Using your saved login.");
                self.reopen_server(saved.server.as_deref(), ctx);
                self.saved_session = Some(saved);
            }
        }
    }

    /// Opens the server that was open last time, if it is still in the list.
    fn reopen_server(&mut self, addr: Option<&str>, ctx: &egui::Context) {
        let index = addr.and_then(|addr| self.servers.iter().position(|s| s.addr == addr));
        if let Some(index) = index {
            self.select_server(index, ctx);
        }
    }

    /// What eframe should store for the next run.
    pub fn stored_session(&self) -> Option<SavedSession> {
        match &self.session {
            Some(session) if self.remember => Some(SavedSession {
                token: session.token.clone(),
                name: self.user.clone(),
                server: self.selected.and_then(|i| self.servers.get(i)).map(|s| s.addr.clone()),
            }),
            Some(_) => None,
            // Not logged in (for example, the auth service was unreachable at startup): keep what we had.
            None => self.saved_session.clone(),
        }
    }

    /// Logged in to an account (independent of any server).
    pub fn is_logged_in(&self) -> bool {
        self.session.is_some()
    }

    /// Joined the selected server and ready to show its channels.
    pub fn is_chat_ready(&self) -> bool {
        self.conn.is_some() && self.welcomed
    }

    /// Name of the selected server, for the sidebar header.
    pub fn selected_name(&self) -> Option<&str> {
        self.selected
            .and_then(|i| self.servers.get(i))
            .map(|s| s.name.as_str())
    }

    fn send(&self, msg: ClientMsg) {
        if let Some(conn) = &self.conn {
            let _ = conn.outgoing.send(msg);
        }
    }

    /// Registers or logs in with the auth service. On success we hold a session for every server.
    pub fn log_in(&mut self) {
        let email = self.login_email.trim().to_string();
        let password = self.login_password.clone();

        let request = if self.register_mode {
            let name = self.login_name.trim().to_string();
            if name.is_empty() {
                self.login_error = Some("Enter a display name.".into());
                return;
            }
            if !is_valid_email(&email) {
                self.login_error = Some("Enter a valid email address.".into());
                return;
            }
            if password.chars().count() < MIN_PASSWORD_LEN {
                self.login_error =
                    Some(format!("Password must be at least {MIN_PASSWORD_LEN} characters."));
                return;
            }
            AuthRequest::Register { name, email, password }
        } else {
            if email.is_empty() || password.is_empty() {
                self.login_error = Some("Enter your email and password.".into());
                return;
            }
            AuthRequest::Login { email, password }
        };

        match auth_client::request(&request) {
            Ok(AuthReply::Session { token, name }) => {
                self.auth_down = false;
                self.user = name.clone();
                self.session = Some(Session { token });
                self.login_error = None;
                self.login_password.clear();
                self.login_name.clear();
            }
            Ok(AuthReply::Error { text }) => self.login_error = Some(text),
            Ok(_) => self.login_error = Some("Unexpected reply from the auth service.".into()),
            Err(e) => {
                self.auth_down = true;
                self.login_error = Some(e);
            }
        }
    }

    /// Ends the session on the auth service and returns to the login screen.
    pub fn logout(&mut self) {
        self.saved_session = None;
        if let Some(session) = self.session.take() {
            let _ = auth_client::request(&AuthRequest::Logout { token: session.token });
            crate::keychain::forget(&self.user);
        }
        self.disconnect();
        self.selected = None;
        self.user.clear();
        self.login_password.clear();
    }

    /// Drops the connection to the current server and clears its content.
    fn disconnect(&mut self) {
        self.conn = None; // dropping the connection shuts the socket down
        self.welcomed = false;
        self.clear_session();
    }

    fn clear_session(&mut self) {
        self.channels.clear();
        self.online.clear();
        self.offline.clear();
        self.owner = None;
        self.server_description.clear();
        self.server_icon = None;
        self.server_public = false;
        self.unread.clear();
        self.read_marks.clear();
        self.search.clear();
        self.messages.clear();
        self.input.clear();
        self.status.clear();
    }

    /// Whether the logged-in user owns the open server.
    pub fn is_owner(&self) -> bool {
        self.owner.as_deref() == Some(self.user.as_str())
    }

    /// Opens "Find servers" and asks the selected server which other servers it knows.
    pub fn open_discover(&mut self, ctx: &egui::Context) {
        self.show_discover = true;
        self.discovered.clear();
        self.discover_error = None;
        let Some(addr) = self.selected.and_then(|i| self.servers.get(i)).map(|s| s.addr.clone()) else {
            self.discover_rx = None;
            self.discover_error = Some("Add a server first. Searching starts from a server you know.".into());
            return;
        };
        let (tx, rx) = std::sync::mpsc::channel();
        discover::start(addr, tx, ctx.clone());
        self.discover_rx = Some(rx);
    }

    /// Takes in any servers the search has found since the last frame.
    pub fn drain_discovered(&mut self) {
        let Some(rx) = &self.discover_rx else {
            return;
        };
        while let Ok(found) = rx.try_recv() {
            if !self.discovered.iter().any(|d| d.addr == found.addr) {
                self.discovered.push(found);
            }
        }
    }

    /// Saves a server found by searching, so it appears in the server rail.
    pub fn add_discovered(&mut self, found: &FoundServer) {
        if self.servers.iter().any(|s| s.addr == found.addr) {
            return;
        }
        self.servers.push(SavedServer {
            name: found.name.clone(),
            addr: found.addr.clone(),
            icon: found.icon.clone(),
        });
    }

    /// Asks the server to save the name and description. The server only accepts this from the owner.
    pub fn update_server(&mut self) {
        let name = self.rename_input.trim().to_string();
        if name.is_empty() {
            return;
        }
        let description = self.description_input.trim().to_string();
        self.send(ClientMsg::UpdateServer { name, description });
    }

    /// Records the server's icon, for the open session and the saved server list.
    fn set_server_icon(&mut self, icon: Option<String>) {
        if let Some(server) = self.selected.and_then(|i| self.servers.get_mut(i)) {
            server.icon = icon.clone();
        }
        self.server_icon = icon;
    }

    /// Lets the owner pick a picture, shrinks it to a 128x128 PNG and sends it to the server.
    pub fn pick_server_icon(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg"])
            .pick_file()
        else {
            return;
        };
        match icon_png_base64(&path) {
            Ok(png_base64) => {
                self.icon_error = None;
                self.send(ClientMsg::SetServerIcon { png_base64: Some(png_base64) });
            }
            Err(e) => self.icon_error = Some(e),
        }
    }

    /// Asks the server to remove its icon.
    pub fn remove_server_icon(&mut self) {
        self.icon_error = None;
        self.send(ClientMsg::SetServerIcon { png_base64: None });
    }

    /// Records the server's name and description, for the open session and the saved server list.
    fn set_server_profile(&mut self, name: &str, description: &str) {
        if let Some(server) = self.selected.and_then(|i| self.servers.get_mut(i)) {
            server.name = name.to_string();
        }
        self.server_description = description.to_string();
    }

    /// Joins the selected server with the account's session token.
    pub fn connect_selected(&mut self, ctx: &egui::Context) {
        let (Some(index), Some(session)) = (self.selected, self.session.as_ref()) else {
            return;
        };
        let token = session.token.clone();
        let server = self.servers[index].clone();

        self.disconnect();
        self.server_error = None;
        let repaint = ctx.clone();
        match net::connect(&server.addr, ClientMsg::Connect { token }, move || repaint.request_repaint()) {
            Ok(conn) => {
                self.conn = Some(conn);
                self.connected_addr = server.addr;
                self.channel = "general".into();
                self.status = "Connecting...".into();
                self.focus_input = true;
            }
            Err(e) => self.server_error = Some(format!("Could not connect to {}: {e}", server.name)),
        }
    }

    /// Adds the server from the "Add a server" dialog and joins it.
    pub fn add_server(&mut self, ctx: &egui::Context) {
        let addr = match self.add_addr.trim() {
            "" => DEFAULT_ADDR.to_string(),
            // A name or address without a port uses the default chat port.
            a if !a.contains(':') => format!("{a}:{DEFAULT_PORT}"),
            a => a.to_string(),
        };
        // The server sends its own name when we connect, so the address is the first label.
        let name = addr.clone();
        if self.servers.iter().any(|s| s.addr == addr) {
            self.add_error = Some("That server is already in your list.".into());
            return;
        }
        self.servers.push(SavedServer { name, addr, icon: None });
        self.show_add_server = false;
        self.add_addr.clear();
        self.add_error = None;
        self.select_server(self.servers.len() - 1, ctx);
    }

    pub fn select_server(&mut self, index: usize, ctx: &egui::Context) {
        if index >= self.servers.len() || self.selected == Some(index) {
            return;
        }
        self.selected = Some(index);
        self.connect_selected(ctx);
    }

    pub fn remove_server(&mut self, index: usize) {
        if index >= self.servers.len() {
            return;
        }
        if self.selected == Some(index) {
            self.disconnect();
            self.selected = None;
        } else if let Some(sel) = self.selected.filter(|&s| s > index) {
            self.selected = Some(sel - 1);
        }
        let addr = self.servers[index].addr.clone();
        crate::trust::forget(&format!("server:{addr}"));
        self.servers.remove(index);
    }

    /// Applies everything the network thread has delivered since the last frame.
    pub fn drain_network(&mut self) {
        let mut events = Vec::new();
        if let Some(conn) = &self.conn {
            while let Ok(ev) = conn.incoming.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            match ev {
                NetEvent::Server(msg) => self.on_server(msg),
                NetEvent::Disconnected(reason) => {
                    let was_welcomed = self.welcomed;
                    self.disconnect();
                    let name = self.selected_name().unwrap_or("the server").to_string();
                    if was_welcomed {
                        self.server_error = Some(format!("Disconnected from {name} ({reason})."));
                    } else if self.server_error.is_none() {
                        // The server usually sent an Error just before closing; keep that message.
                        self.server_error = Some(format!("Could not join {name} ({reason})."));
                    }
                    return;
                }
            }
        }
    }

    fn on_server(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { name, server_name, server_description, server_icon, server_public } => {
                self.server_public = server_public;
                self.welcomed = true;
                self.user = name.clone();
                self.server_error = None;
                self.set_server_profile(&server_name, &server_description);
                self.set_server_icon(server_icon);
                self.status = format!("Connected as {name}");
                self.send(ClientMsg::ListChannels);
                self.send(ClientMsg::Join { channel: self.channel.clone() });
            }
            ServerMsg::Joined { channel, history } => {
                if channel != self.channel && self.messages.contains_key(&self.channel) {
                    let seen = self.messages.get(&self.channel).map_or(0, Vec::len);
                    self.read_marks.insert(self.channel.clone(), seen);
                }
                self.unread.remove(&channel);
                self.messages
                    .insert(channel.clone(), history.into_iter().map(Entry::Chat).collect());
                self.channel = channel;
                self.focus_input = true;
                // Refresh the list so channels created by anyone show up.
                self.send(ClientMsg::ListChannels);
            }
            ServerMsg::Message { message } => {
                // The server only sends messages for the channel we are viewing.
                self.messages
                    .entry(message.channel.clone())
                    .or_default()
                    .push(Entry::Chat(message));
            }
            ServerMsg::Reactions { channel, id, reactions } => {
                let entry = self.messages.get_mut(&channel).and_then(|list| {
                    list.iter_mut().find_map(|e| match e {
                        Entry::Chat(m) if m.id == id => Some(m),
                        _ => None,
                    })
                });
                if let Some(m) = entry {
                    m.reactions = reactions;
                }
            }
            ServerMsg::MessageDeleted { channel, id } => {
                if let Some(list) = self.messages.get_mut(&channel) {
                    list.retain(|e| !matches!(e, Entry::Chat(m) if m.id == id));
                }
                if self.editing == Some(id) {
                    self.editing = None;
                }
            }
            ServerMsg::MessageEdited { channel, id, text } => {
                let entry = self.messages.get_mut(&channel).and_then(|list| {
                    list.iter_mut().find_map(|e| match e {
                        Entry::Chat(m) if m.id == id => Some(m),
                        _ => None,
                    })
                });
                if let Some(m) = entry {
                    m.text = text;
                    m.edited = true;
                }
            }
            ServerMsg::Activity { channel } => {
                if channel != self.channel {
                    self.unread.insert(channel);
                }
            }
            ServerMsg::ServerUpdated { name, description } => self.set_server_profile(&name, &description),
            ServerMsg::ServerIcon { png_base64 } => self.set_server_icon(png_base64),
            // Only sent in answer to a search, never on the chat connection.
            ServerMsg::Info { .. } | ServerMsg::Peers { .. } => {}
            ServerMsg::Channels { channels } => self.channels = channels,
            ServerMsg::Presence { users } => {
                self.online = users.iter().filter(|u| u.online).map(|u| u.name.clone()).collect();
                self.offline = users.iter().filter(|u| !u.online).map(|u| u.name.clone()).collect();
                self.owner = users.iter().find(|u| u.owner).map(|u| u.name.clone());
            }
            ServerMsg::System { channel, text } => {
                self.messages
                    .entry(channel)
                    .or_default()
                    .push(Entry::System { text, at: now_secs() });
            }
            ServerMsg::Error { text } => {
                if self.welcomed {
                    self.status = format!("Error: {text}");
                } else {
                    self.server_error = Some(text);
                }
            }
        }
    }

    pub fn join(&mut self, channel: &str) {
        let channel = channel.trim().trim_start_matches('#').to_string();
        if channel.is_empty() || channel == self.channel {
            return;
        }
        self.send(ClientMsg::Join { channel });
    }

    pub fn create_channel(&mut self) {
        let name = std::mem::take(&mut self.new_channel);
        self.create_box_open = false;
        self.join(&name);
    }

    pub fn send_input(&mut self) {
        let text = self.input.trim().to_string();
        self.input.clear();
        if text.is_empty() {
            return;
        }
        match text.strip_prefix("/join ") {
            Some(channel) => self.join(channel),
            None => self.send(ClientMsg::Say { text }),
        }
    }

    /// Carries out a menu or button choice.
    pub fn apply(&mut self, action: MenuAction, ctx: &egui::Context) {
        match action {
            MenuAction::StartEdit { id, text } => {
                self.editing = Some(id);
                ctx.data_mut(|d| d.insert_temp(edit_buffer(id), text));
                ctx.memory_mut(|m| m.request_focus(edit_buffer(id)));
            }
            MenuAction::SaveEdit(id) => {
                if let Some(text) = ctx.data_mut(|d| d.get_temp::<String>(edit_buffer(id))) {
                    self.send(ClientMsg::Edit { id, text });
                }
                self.editing = None;
            }
            MenuAction::CancelEdit => self.editing = None,
            MenuAction::AskDelete(id) => self.pending_delete = Some(id),
            MenuAction::Delete(id) => self.send(ClientMsg::Delete { id }),
            MenuAction::React { id, emoji } => self.send(ClientMsg::React { id, emoji }),
            MenuAction::Mention(name) => {
                self.input.push_str(&format!("@{name} "));
                self.focus_input = true;
            }
            MenuAction::CopyText(text) => ctx.copy_text(text),
            MenuAction::MarkRead(channel) => {
                self.unread.remove(&channel);
                self.read_marks.remove(&channel);
            }
            MenuAction::CreateChannel => {
                self.focus_create = true;
                self.create_box_open = true;
            }
            MenuAction::ToggleCreateBox => self.show_create_box = !self.show_create_box,
            MenuAction::ToggleMembers => self.members_hidden = !self.members_hidden,
            MenuAction::Logout => self.logout(),
            MenuAction::TrustNewServerCert => {
                if let Some(index) = self.selected {
                    let key = format!("server:{}", self.servers[index].addr);
                    crate::trust::forget(&key);
                }
                self.server_error = None;
                self.connect_selected(ctx);
            }
            MenuAction::SelectServer(i) => self.select_server(i, ctx),
            MenuAction::RemoveServer(i) => self.remove_server(i),
            MenuAction::OpenAddServer => {
                self.show_add_server = true;
                self.add_error = None;
            }
            MenuAction::OpenServerSettings => {
                self.rename_input = self.selected_name().unwrap_or_default().to_string();
                self.description_input = self.server_description.clone();
                self.show_server_settings = true;
            }
            MenuAction::OpenDiscover => self.open_discover(ctx),
            MenuAction::RetryServer => self.connect_selected(ctx),
        }
    }
}
