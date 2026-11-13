//! Application state and the logic that reacts to server events and user actions.
//! Drawing lives in `ui.rs`.
//!
//! There is one account for the whole client. You log in (or register) first;
//! then you add servers, and each server is joined with that account's session token.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use common::{is_valid_email, AuthReply, AuthRequest, ChatMessage, ClientMsg, DirectMessage, ServerMsg, DEFAULT_ADDR};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::auth_client;
use crate::discover::{self, FoundServer};
use crate::net::{self, Connection, NetEvent};
use crate::util::now_secs;

/// Key under which the saved server list is stored by eframe.
pub const SERVERS_KEY: &str = "disscord_servers";
/// Key under which the colour theme is stored by eframe.
pub const THEME_KEY: &str = "disscord_theme";
pub const SKIN_TONE_KEY: &str = "disscord_skin_tone";
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
    /// Choose the skin tone for the reaction picker: 0 is the default, 1 to 5 the tones.
    SetSkinTone(usize),
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
    ToggleMembers,
    Logout,
    /// Open a direct message with the member with this display name.
    OpenDm(String),
    /// Show the Direct messages page.
    OpenHome,
    /// Open a direct message on the server with this address.
    OpenDmOn { addr: String, channel: String },
    SelectServer(usize),
    RemoveServer(usize),
    OpenAddServer,
    OpenServerSettings,
    OpenUserSettings,
    OpenDiscover,
    RetryServer,
    /// The server's certificate changed, and the user trusts the new one.
    TrustNewServerCert,
}

/// A background connection to one saved server, kept open so its direct messages can be listed
/// and their activity noticed while another server is the one being viewed.
pub struct DmLink {
    pub addr: String,
    pub conn: Connection,
    /// The server's own name, from its welcome. Empty until then.
    pub server_name: String,
    pub dms: Vec<DirectMessage>,
    /// Conversations with new activity that are not open.
    pub unread: HashSet<String>,
}

/// How long to wait before trying a server again after its direct messages could not be reached.
const DM_RETRY: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct DissCordApp {
    /// Background connections for direct messages, one per saved server.
    pub dm_links: Vec<DmLink>,
    /// Connections being made, by server address. Each delivers its result when ready.
    pub dm_connecting: HashMap<String, Receiver<Result<Connection, String>>>,
    /// Servers that could not be reached, and when to try them again.
    pub dm_retry_at: HashMap<String, Instant>,
    /// A conversation to open once the server we just switched to has welcomed us.
    pub pending_dm: Option<(String, String)>,
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
    /// The Direct messages page is open instead of a server's channels.
    pub home: bool,
    pub discovered: Vec<FoundServer>,
    pub discover_rx: Option<Receiver<FoundServer>>,
    pub discover_error: Option<String>,
    /// Addresses of rows whose description is shown in full.
    pub discover_expanded: HashSet<String>,

    // "Server settings" dialog (owner only)
    pub show_server_settings: bool,
    /// The user settings page, opened from the gear beside your name.
    pub show_user_settings: bool,
    /// The account's email, as the auth service reports it. `None` when it is not on record.
    pub account_email: Option<String>,
    /// Why the account's email could not be loaded, if it could not.
    pub account_email_error: Option<String>,
    /// The email form on the Account page: whether it is open, and what has been typed.
    pub email_editing: bool,
    pub email_input: String,
    pub email_password: String,
    pub email_error: Option<String>,
    /// The account's display name and username, as the auth service reports them.
    pub account_username: Option<String>,
    /// The name and username forms on the Account page: whether each is open, and what is typed.
    pub name_editing: bool,
    pub name_input: String,
    pub username_editing: bool,
    pub username_input: String,
    pub profile_error: Option<String>,
    /// Changes waiting for the Save button on the Account page. Nothing is sent until then.
    pub name_draft: Option<String>,
    pub username_draft: Option<String>,
    pub email_draft: Option<(String, String)>,
    pub theme_draft: Option<crate::theme::Theme>,
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
    /// Your direct messages on this server.
    pub dms: Vec<DirectMessage>,
    /// Names of members currently online, in the order the server sent them.
    pub online: Vec<String>,
    /// Names of members who are not connected right now.
    pub offline: Vec<String>,
    /// The owner of the open server (its first member).
    pub owner: Option<String>,
    /// The hidden account id of this user and of the owner. Own messages and the owner are matched by
    /// id, so they still match after a name change.
    pub user_id: String,
    pub owner_id: Option<String>,
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
    /// The text channel list is folded away (the "TEXT CHANNELS" heading was clicked).
    pub channels_collapsed: bool,
    /// The box was opened with "+" and stays open until a channel is created or the box loses focus.
    pub create_box_open: bool,
    /// Setting: the colour theme. Saved between runs.
    pub theme: crate::theme::Theme,
    /// Setting: the skin tone the reaction picker uses (0 is the default). Saved between runs.
    pub skin_tone: usize,

    /// Last notice, shown in the composer footer.
    pub status: String,
    /// True while the auth service cannot be reached. The top of the window shows a warning.
    pub auth_down: bool,
    /// The user closed the auth warning. It comes back the next time auth is down.
    pub auth_banner_closed: bool,
}

/// A short, plain explanation of why a connection ended, from the technical reason.
fn friendly_disconnect(reason: &str) -> &'static str {
    let r = reason.to_lowercase();
    if r.contains("refused") {
        "The server isn't running or can't be reached. Check the address and try again."
    } else if r.contains("timed out") || r.contains("timeout") || r.contains("did not answer") {
        "The server stopped responding. Check your connection and try again."
    } else if r.contains("certificate") {
        "The server's security certificate has changed. See the message on the login screen."
    } else if r.contains("close_notify") || r.contains("unexpected-eof") || r.contains("closed") || r.contains("reset") {
        "The server closed the connection, perhaps because it restarted. Try again in a moment."
    } else {
        "Something interrupted the connection. Try again."
    }
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
            Ok(AuthReply::Verified { name, .. }) => {
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

    /// Fetches the account's email from the auth service for the Account page.
    pub fn load_account_email(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        match auth_client::request(&AuthRequest::Profile { token: session.token.clone() }) {
            Ok(AuthReply::Profile { name, username, email }) => {
                self.user = name;
                self.account_username = Some(username);
                self.account_email = email;
                self.account_email_error = None;
            }
            Ok(AuthReply::Error { text }) => self.account_email_error = Some(text),
            Ok(_) => self.account_email_error = Some("Unexpected reply from the auth service.".into()),
            Err(e) => self.account_email_error = Some(e),
        }
    }

    /// True when the Account page has changes waiting to be saved.
    pub fn has_account_changes(&self) -> bool {
        self.name_draft.is_some()
            || self.username_draft.is_some()
            || self.email_draft.is_some()
            || self.theme_draft.is_some()
    }

    /// Drops every change waiting on the Account page.
    pub fn reset_account_changes(&mut self) {
        self.name_draft = None;
        self.username_draft = None;
        self.email_draft = None;
        self.theme_draft = None;
        self.profile_error = None;
        self.email_error = None;
    }

    /// Sends the waiting changes. A change that fails stays waiting, with its error shown.
    pub fn save_account_changes(&mut self) {
        if let Some(name) = self.name_draft.take() {
            if name != self.user {
                self.profile_error = None;
                self.update_account_profile(Some(name.clone()), None);
                if self.profile_error.is_some() {
                    self.name_draft = Some(name);
                }
            }
        }
        if let Some(username) = self.username_draft.take() {
            if Some(&username) != self.account_username.as_ref() {
                self.profile_error = None;
                self.update_account_profile(None, Some(username.clone()));
                if self.profile_error.is_some() {
                    self.username_draft = Some(username);
                }
            }
        }
        if let Some((email, password)) = self.email_draft.take() {
            self.email_input = email.clone();
            self.email_password = password.clone();
            self.email_error = None;
            self.change_account_email();
            if self.email_error.is_some() {
                self.email_draft = Some((email, password));
            }
        }
        if let Some(theme) = self.theme_draft.take() {
            self.theme = theme;
        }
    }

    /// Changes the display name and/or the username. Usernames are lowercase.
    pub fn update_account_profile(&mut self, name: Option<String>, username: Option<String>) {
        let Some(session) = &self.session else {
            return;
        };
        let request = AuthRequest::UpdateProfile { token: session.token.clone(), name, username };
        match auth_client::request(&request) {
            Ok(AuthReply::Profile { name, username, email }) => {
                self.user = name;
                self.account_username = Some(username);
                self.account_email = email;
                self.profile_error = None;
                self.name_editing = false;
                self.username_editing = false;
            }
            Ok(AuthReply::Error { text }) => self.profile_error = Some(text),
            Ok(_) => self.profile_error = Some("Unexpected reply from the auth service.".into()),
            Err(e) => self.profile_error = Some(e),
        }
    }

    /// Changes the account's email. The password must be the current one.
    pub fn change_account_email(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let new_email = self.email_input.trim().to_string();
        let request = AuthRequest::ChangeEmail {
            token: session.token.clone(),
            password: self.email_password.clone(),
            new_email,
        };
        match auth_client::request(&request) {
            Ok(AuthReply::EmailChanged { email }) => {
                self.account_email = Some(email);
                self.account_email_error = None;
                self.email_editing = false;
                self.email_input.clear();
                self.email_password.clear();
                self.email_error = None;
            }
            Ok(AuthReply::Error { text }) => self.email_error = Some(text),
            Ok(_) => self.email_error = Some("Unexpected reply from the auth service.".into()),
            Err(e) => self.email_error = Some(e),
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
    /// How the open channel is named: `#name`, or `@name` for a direct message.
    pub fn channel_label(&self) -> String {
        match self.dms.iter().find(|d| d.channel == self.channel) {
            Some(dm) => format!("@ {}", dm.name),
            None if self.channel.starts_with("dm-") => "@ Direct message".into(),
            None => format!("#{}", self.channel),
        }
    }

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
        self.close_dm_links();
        // Tell the current server first, so it forgets the session even if the auth service is down.
        if let Some(conn) = self.conn.take() {
            conn.send_last(ClientMsg::Logout);
        }
        if let Some(session) = self.session.take() {
            let _ = auth_client::request(&AuthRequest::Logout { token: session.token });
            crate::keychain::forget(&self.user);
        }
        self.disconnect();
        self.selected = None;
        self.user.clear();
        self.user_id.clear();
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
        self.dms.clear();
        self.online.clear();
        self.offline.clear();
        self.owner = None;
        self.owner_id = None;
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
        self.owner_id.is_some() && self.owner_id.as_deref() == Some(self.user_id.as_str())
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

    /// Starts a background connection to every saved server that has none yet. Each connection is
    /// made on its own thread, so a slow or offline server does not hold up the window.
    pub fn ensure_dm_links(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let now = Instant::now();
        for server in &self.servers {
            let addr = server.addr.clone();
            if self.dm_links.iter().any(|l| l.addr == addr) || self.dm_connecting.contains_key(&addr) {
                continue;
            }
            if self.dm_retry_at.get(&addr).is_some_and(|at| *at > now) {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            self.dm_connecting.insert(addr.clone(), rx);
            let token = session.token.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let result = net::connect(&addr, ClientMsg::Connect { token }, move || repaint.request_repaint());
                let _ = tx.send(result);
            });
        }
    }

    /// Takes in the connections that have finished being made.
    pub fn poll_dm_links(&mut self) {
        let addrs: Vec<String> = self.dm_connecting.keys().cloned().collect();
        for addr in addrs {
            let Some(rx) = self.dm_connecting.get(&addr) else {
                continue;
            };
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => Err("the connection thread stopped".to_string()),
            };
            self.dm_connecting.remove(&addr);
            match result {
                Ok(conn) => {
                    self.dm_retry_at.remove(&addr);
                    self.dm_links.push(DmLink {
                        addr,
                        conn,
                        server_name: String::new(),
                        dms: Vec::new(),
                        unread: HashSet::new(),
                    });
                }
                Err(_) => {
                    self.dm_retry_at.insert(addr, Instant::now() + DM_RETRY);
                }
            }
        }
    }

    /// Applies what the background connections have delivered.
    pub fn drain_dm_links(&mut self) {
        let mut ask_refresh = Vec::new();
        let mut dropped = Vec::new();
        for link in &mut self.dm_links {
            let mut events = Vec::new();
            let mut ended = false;
            while let Ok(ev) = link.conn.incoming.try_recv() {
                match ev {
                    NetEvent::Server(msg) => events.push(msg),
                    NetEvent::Disconnected(_) => ended = true,
                }
            }
            for msg in events {
                match msg {
                    ServerMsg::Welcome { server_name, .. } => link.server_name = server_name,
                    ServerMsg::DirectMessages { list } => link.dms = list,
                    ServerMsg::Activity { channel } if channel.starts_with("dm-") => {
                        if !link.dms.iter().any(|d| d.channel == channel) {
                            ask_refresh.push(link.addr.clone());
                        }
                        link.unread.insert(channel);
                    }
                    _ => {}
                }
            }
            if ended {
                dropped.push(link.addr.clone());
            }
        }
        for addr in ask_refresh {
            self.send_to_link(&addr, ClientMsg::ListDirectMessages);
        }
        for addr in dropped {
            self.dm_links.retain(|l| l.addr != addr);
            self.dm_retry_at.insert(addr, Instant::now() + Duration::from_secs(5));
        }
    }

    fn send_to_link(&self, addr: &str, msg: ClientMsg) {
        if let Some(link) = self.dm_links.iter().find(|l| l.addr == addr) {
            let _ = link.conn.outgoing.send(msg);
        }
    }

    /// Asks every background connection for its direct messages again.
    pub fn refresh_dm_links(&self) {
        for link in &self.dm_links {
            let _ = link.conn.outgoing.send(ClientMsg::ListDirectMessages);
        }
    }

    /// Ends the background connections, for example at logout.
    pub fn close_dm_links(&mut self) {
        self.dm_links.clear();
        self.dm_connecting.clear();
        self.dm_retry_at.clear();
        self.pending_dm = None;
    }

    /// Opens a direct message on the server it belongs to, switching to that server if needed.
    pub fn open_dm_on(&mut self, addr: &str, channel: &str, ctx: &egui::Context) {
        if let Some(link) = self.dm_links.iter_mut().find(|l| l.addr == addr) {
            link.unread.remove(channel);
        }
        if self.conn.is_some() && self.connected_addr == addr {
            self.join(channel);
            return;
        }
        let Some(index) = self.servers.iter().position(|s| s.addr == addr) else {
            return;
        };
        self.pending_dm = Some((addr.to_string(), channel.to_string()));
        self.selected = Some(index);
        self.connect_selected(ctx);
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
        // Choosing a server always leaves the Direct messages page, even if it is already selected.
        self.home = false;
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
        self.dm_links.retain(|l| l.addr != addr);
        self.dm_connecting.remove(&addr);
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
                    // The full reason goes to the terminal for anyone debugging.
                    eprintln!("connection to {name} ended: {reason}");
                    let friendly = friendly_disconnect(&reason);
                    if was_welcomed {
                        self.server_error = Some(format!("Lost the connection to {name}. {friendly}"));
                    } else if self.server_error.is_none() {
                        // The server usually sent an Error just before closing; keep that message.
                        self.server_error = Some(format!("Couldn't join {name}. {friendly}"));
                    }
                    return;
                }
            }
        }
    }

    fn on_server(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Welcome { name, user_id, server_name, server_description, server_icon, server_public } => {
                self.server_public = server_public;
                self.welcomed = true;
                self.user = name.clone();
                self.user_id = user_id;
                self.server_error = None;
                self.set_server_profile(&server_name, &server_description);
                self.set_server_icon(server_icon);
                self.status = format!("Connected as {name}");
                self.send(ClientMsg::ListChannels);
                self.send(ClientMsg::Join { channel: self.channel.clone() });
                // A direct message chosen on another server opens once that server is ready.
                if let Some((addr, channel)) = self.pending_dm.take() {
                    if addr == self.connected_addr {
                        self.send(ClientMsg::Join { channel });
                    }
                }
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
                // A direct message someone just started with you is not in your list yet.
                if channel.starts_with("dm-") && !self.dms.iter().any(|d| d.channel == channel) {
                    self.send(ClientMsg::ListDirectMessages);
                }
                if channel != self.channel {
                    self.unread.insert(channel);
                }
            }
            ServerMsg::ServerUpdated { name, description } => self.set_server_profile(&name, &description),
            ServerMsg::ServerIcon { png_base64 } => self.set_server_icon(png_base64),
            // Only sent in answer to a search, never on the chat connection.
            ServerMsg::Info { .. } | ServerMsg::Peers { .. } => {}
            ServerMsg::Channels { channels } => self.channels = channels,
            ServerMsg::DirectMessages { list } => self.dms = list,
            ServerMsg::Presence { users } => {
                self.online = users.iter().filter(|u| u.online).map(|u| u.name.clone()).collect();
                self.offline = users.iter().filter(|u| !u.online).map(|u| u.name.clone()).collect();
                self.owner = users.iter().find(|u| u.owner).map(|u| u.name.clone());
                self.owner_id = users.iter().find(|u| u.owner).map(|u| u.id.clone());
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
            MenuAction::ToggleMembers => self.members_hidden = !self.members_hidden,
            MenuAction::Logout => self.logout(),
            MenuAction::OpenDm(name) => {
                // A new conversation opens on the Direct messages page.
                self.home = true;
                self.send(ClientMsg::OpenDm { name });
            }
            MenuAction::SetSkinTone(tone) => self.skin_tone = tone,
            MenuAction::TrustNewServerCert => {
                if let Some(index) = self.selected {
                    let key = format!("server:{}", self.servers[index].addr);
                    crate::trust::forget(&key);
                }
                self.server_error = None;
                self.connect_selected(ctx);
            }
            MenuAction::SelectServer(i) => self.select_server(i, ctx),
            MenuAction::OpenHome => {
                self.home = true;
                self.refresh_dm_links();
            }
            MenuAction::OpenDmOn { addr, channel } => self.open_dm_on(&addr, &channel, ctx),
            MenuAction::RemoveServer(i) => self.remove_server(i),
            MenuAction::OpenAddServer => {
                self.show_add_server = true;
                self.add_error = None;
            }
            MenuAction::OpenUserSettings => {
                self.show_user_settings = true;
                self.load_account_email();
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
