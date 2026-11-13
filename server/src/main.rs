mod db;
mod discovery;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{
    AuthReply, AuthRequest, ChatMessage, ClientMsg, ServerMsg, UserStatus, DEFAULT_ADDR, DEFAULT_AUTH_ADDR,
    DISCOVERY_PORT, MAX_SERVER_DESCRIPTION_LEN, MAX_SERVER_ICON_LEN,
};
use db::Db;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

const MAX_TEXT_LEN: usize = 2000;
const MAX_SERVER_NAME_LEN: usize = 40;
const DEFAULT_DB_PATH: &str = "disscord.db";

struct State {
    db: Db,
    /// Logged-in users, one entry per connection.
    online: Vec<String>,
    /// The address other servers use to reach this one.
    advertise: String,
    /// How many times in a row each known server has failed to answer.
    peer_failures: HashMap<String, u32>,
}

impl State {
    fn remove_online(&mut self, user: &str) {
        if let Some(pos) = self.online.iter().position(|u| u == user) {
            self.online.remove(pos);
        }
    }

    /// Every member of the server, each marked online or offline.
    fn presence(&self) -> Vec<UserStatus> {
        let owner = self.db.owner().ok().flatten();
        self.db
            .member_names()
            .unwrap_or_default()
            .into_iter()
            .map(|name| {
                let online = self.online.contains(&name);
                let owner = owner.as_deref() == Some(name.as_str());
                UserStatus { name, online, owner }
            })
            .collect()
    }
}

type SharedState = Arc<Mutex<State>>;

/// Everything one server needs to start.
struct ServerConfig {
    addr: String,
    db_path: String,
    auth_addr: String,
    advertise: String,
    discovery_port: u16,
    exchange_secs: u64,
    peers: Vec<String>,
}

/// Usage: `server [ADDR] [DB_PATH] [AUTH_ADDR] [COUNT]`
///
/// With COUNT above 1, one process runs that many servers on consecutive ports, starting at
/// ADDR's port. Each gets its own database (DB_PATH with its port in the name) and its own
/// discovery port (the next one up from DISSCORD_DISCOVERY_PORT).
fn main() -> anyhow::Result<()> {
    // Read .env before the runtime starts any threads.
    common::env::load();
    tokio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| DEFAULT_ADDR.to_string());
    let db_path = args.next().unwrap_or_else(|| DEFAULT_DB_PATH.to_string());
    let auth_addr = args.next().unwrap_or_else(|| DEFAULT_AUTH_ADDR.to_string());
    let count: u16 = match args.next() {
        Some(c) => c.parse().map_err(|_| anyhow::anyhow!("COUNT must be a number"))?,
        None => 1,
    };
    anyhow::ensure!(count >= 1, "COUNT must be at least 1");

    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("ADDR must look like host:port"))?;
    let port: u16 = port.parse()?;
    // Servers to start from, for finding others across the internet: DISSCORD_PEERS=host:port,host:port
    let peers: Vec<String> = std::env::var("DISSCORD_PEERS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(String::from)
        .collect();
    let discovery_base: u16 = env_number("DISSCORD_DISCOVERY_PORT", DISCOVERY_PORT);
    let exchange_secs: u64 = env_number("DISSCORD_EXCHANGE_SECS", 30);

    // Every server in this group, by address.
    let group: Vec<String> = (0..count)
        .map(|i| {
            port.checked_add(i)
                .map(|p| format!("{host}:{p}"))
                .ok_or_else(|| anyhow::anyhow!("too many servers for this port"))
        })
        .collect::<anyhow::Result<_>>()?;

    let mut tasks = Vec::new();
    for i in 0..count {
        let server_port = port
            .checked_add(i)
            .ok_or_else(|| anyhow::anyhow!("too many servers for this port"))?;
        let server_addr = group[i as usize].clone();
        let config = ServerConfig {
            advertise: if count == 1 {
                std::env::var("DISSCORD_ADVERTISE").unwrap_or_else(|_| lan_address(&server_addr))
            } else {
                lan_address(&server_addr)
            },
            db_path: if count == 1 { db_path.clone() } else { numbered_path(&db_path, server_port) },
            addr: server_addr,
            auth_addr: auth_addr.clone(),
            discovery_port: discovery_base
                .checked_add(i)
                .ok_or_else(|| anyhow::anyhow!("too many servers for the discovery port"))?,
            exchange_secs,
            // The first server of a group starts from DISSCORD_PEERS and knows every other server in
            // the group. The others find the rest of the group through it.
            peers: if i == 0 {
                peers.iter().cloned().chain(group.iter().skip(1).cloned()).collect()
            } else {
                Vec::new()
            },
        };
        tasks.push(tokio::spawn(run_server(config)));
    }
    for task in tasks {
        task.await??;
    }
    Ok(())
}

/// Runs one server: its database, its finding-others tasks, and its listener.
async fn run_server(config: ServerConfig) -> anyhow::Result<()> {
    let ServerConfig {
        addr,
        db_path,
        auth_addr,
        advertise,
        discovery_port,
        exchange_secs,
        peers,
    } = config;
    let auth_addr = Arc::new(auth_addr);

    let db = Db::open(&db_path)?;
    // This server's own certificate. It is made the first time the server runs.
    let (cert, key) = match (db.info_value("tls_cert")?, db.info_value("tls_key")?) {
        (Some(cert), Some(key)) => (cert, key),
        _ => {
            let identity = common::tls::new_identity()?;
            db.set_server_info("tls_cert", &identity.0)?;
            db.set_server_info("tls_key", &identity.1)?;
            identity
        }
    };
    let acceptor = common::tls::acceptor(&cert, &key)?;
    let state: SharedState = Arc::new(Mutex::new(State {
        db,
        online: Vec::new(),
        advertise: advertise.clone(),
        peer_failures: HashMap::new(),
    }));
    {
        let s = state.lock().unwrap();
        for peer in peers.iter().filter(|a| discovery::valid_addr(a) && a.as_str() != advertise) {
            let _ = s.db.add_peer(peer);
        }
    }

    // Finding other servers: LAN beacons, and asking known servers for their lists.
    tokio::spawn(discovery::beacons(state.clone(), discovery_port));
    tokio::spawn(discovery::exchange(state.clone(), Duration::from_secs(exchange_secs)));

    // Every connection subscribes to this and forwards events for its own channel.
    let (tx, _) = broadcast::channel::<ServerMsg>(256);

    let listener = TcpListener::bind(&addr).await?;
    println!("DissCord server listening on {addr} (TLS, known to others as {advertise}), storing data in {db_path}, accounts from {auth_addr}");

    loop {
        let (socket, peer) = listener.accept().await?;
        let state = state.clone();
        let tx = tx.clone();
        let auth_addr = auth_addr.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            println!("connection from {peer}");
            let socket = match acceptor.accept(socket).await {
                Ok(socket) => socket,
                Err(e) => {
                    eprintln!("{peer}: TLS handshake failed: {e}");
                    return;
                }
            };
            if let Err(e) = handle_connection(socket, state, tx, auth_addr).await {
                if !common::tls::is_quiet_close(&e) {
                    eprintln!("{peer}: {e}");
                }
            }
            println!("{peer} disconnected");
        });
    }
}

/// `disscord.db` with a port in its name: `disscord-7879.db`.
fn numbered_path(db_path: &str, port: u16) -> String {
    let path = std::path::Path::new(db_path);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "disscord".into());
    let name = match path.extension() {
        Some(ext) => format!("{stem}-{port}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{port}"),
    };
    path.with_file_name(name).to_string_lossy().into_owned()
}

async fn handle_connection<S>(
    socket: S,
    state: SharedState,
    tx: broadcast::Sender<ServerMsg>,
    auth_addr: Arc<String>,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(socket);
    let mut lines = BufReader::new(reader).lines();
    let mut rx = tx.subscribe();

    let Some(first) = lines.next_line().await? else {
        return Ok(());
    };
    // Public requests need no login: this server's details, or the servers it knows about.
    match serde_json::from_str::<ClientMsg>(&first) {
        Ok(ClientMsg::Info) => {
            send(&mut writer, &public_info(&state)).await?;
            return Ok(());
        }
        Ok(ClientMsg::Peers { known }) => {
            let addrs = share_peers(&state, known);
            send(&mut writer, &ServerMsg::Peers { addrs }).await?;
            return Ok(());
        }
        _ => {}
    }

    // Handshake: otherwise the first message must be a Connect carrying a session token.
    let outcome = match serde_json::from_str::<ClientMsg>(&first) {
        Ok(ClientMsg::Connect { token }) => verify_session(&state, &auth_addr, &token).await,
        _ => Err("connect with a session token first".to_string()),
    };
    let user = match outcome {
        Ok(name) => name,
        Err(text) => {
            send(&mut writer, &ServerMsg::Error { text }).await?;
            return Ok(());
        }
    };

    // The same account may be connected more than once (for example from two windows). Each
    // connection adds one entry to `online`, and the member shows as online while any is open.
    let (users, server_name, server_description, server_icon) = {
        let mut s = state.lock().unwrap();
        s.db.add_member(&user, now_secs())?;
        s.online.push(user.clone());
        (s.presence(), s.db.server_name()?, s.db.server_description()?, s.db.server_icon()?)
    };

    let mut channel = "general".to_string();
    // Removes the user and broadcasts presence on every exit path, including errors.
    let _online = OnlineGuard { state: state.clone(), tx: tx.clone(), user: user.clone() };
    let server_public = state.lock().unwrap().db.is_public().unwrap_or(false);
    let welcome = ServerMsg::Welcome {
        name: user.clone(),
        server_name,
        server_description,
        server_icon,
        server_public,
    };
    send(&mut writer, &welcome).await?;
    let _ = tx.send(ServerMsg::Presence { users });
    let _ = tx.send(ServerMsg::System {
        channel: channel.clone(),
        text: format!("{user} connected"),
    });

    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let msg = match serde_json::from_str::<ClientMsg>(&line) {
                    Ok(m) => m,
                    Err(e) => {
                        send(&mut writer, &ServerMsg::Error { text: format!("bad message: {e}") }).await?;
                        continue;
                    }
                };

                match msg {
                    ClientMsg::Connect { .. } => {
                        send(&mut writer, &ServerMsg::Error { text: "already logged in".into() }).await?;
                    }

                    ClientMsg::Info => {
                        send(&mut writer, &public_info(&state)).await?;
                    }

                    ClientMsg::Peers { known } => {
                        let addrs = share_peers(&state, known);
                        send(&mut writer, &ServerMsg::Peers { addrs }).await?;
                    }

                    ClientMsg::ListChannels => {
                        let channels = state.lock().unwrap().db.channel_names()?;
                        send(&mut writer, &ServerMsg::Channels { channels }).await?;
                    }

                    ClientMsg::SetServerIcon { png_base64 } => {
                        let png_base64 = png_base64.filter(|p| !p.is_empty());
                        if png_base64.as_ref().is_some_and(|p| p.len() > MAX_SERVER_ICON_LEN) {
                            send(&mut writer, &ServerMsg::Error {
                                text: "server icon is too large".into(),
                            })
                            .await?;
                            continue;
                        }
                        let result = {
                            let s = state.lock().unwrap();
                            if s.db.owner()?.as_deref() == Some(user.as_str()) {
                                s.db.set_server_icon(png_base64.as_deref())?;
                                Ok(())
                            } else {
                                Err("only the owner can change this server")
                            }
                        };
                        match result {
                            Ok(()) => {
                                let _ = tx.send(ServerMsg::ServerIcon { png_base64 });
                            }
                            Err(text) => send(&mut writer, &ServerMsg::Error { text: text.into() }).await?,
                        }
                    }

                    ClientMsg::UpdateServer { name, description } => {
                        let name = name.trim().to_string();
                        let description = description.trim().to_string();
                        if name.is_empty() || name.chars().count() > MAX_SERVER_NAME_LEN {
                            send(&mut writer, &ServerMsg::Error {
                                text: format!("server name must be 1-{MAX_SERVER_NAME_LEN} characters"),
                            })
                            .await?;
                            continue;
                        }
                        if description.chars().count() > MAX_SERVER_DESCRIPTION_LEN {
                            send(&mut writer, &ServerMsg::Error {
                                text: format!("description longer than {MAX_SERVER_DESCRIPTION_LEN} characters"),
                            })
                            .await?;
                            continue;
                        }
                        let result = {
                            let s = state.lock().unwrap();
                            if s.db.owner()?.as_deref() == Some(user.as_str()) {
                                s.db.set_server_name(&name)?;
                                s.db.set_server_description(&description)?;
                                Ok(())
                            } else {
                                Err("only the owner can change this server")
                            }
                        };
                        match result {
                            Ok(()) => {
                                let _ = tx.send(ServerMsg::ServerUpdated { name, description });
                            }
                            Err(text) => send(&mut writer, &ServerMsg::Error { text: text.into() }).await?,
                        }
                    }

                    ClientMsg::Join { channel: requested } => {
                        let requested = normalize_channel(&requested);
                        if requested.is_empty() {
                            send(&mut writer, &ServerMsg::Error { text: "invalid channel name".into() }).await?;
                            continue;
                        }
                        let history = {
                            let s = state.lock().unwrap();
                            s.db.ensure_channel(&requested)?;
                            s.db.history(&requested)?
                        };
                        channel = requested.clone();
                        send(&mut writer, &ServerMsg::Joined { channel: requested, history }).await?;
                    }

                    ClientMsg::Say { text } => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        if text.chars().count() > MAX_TEXT_LEN {
                            send(&mut writer, &ServerMsg::Error {
                                text: format!("message longer than {MAX_TEXT_LEN} characters"),
                            })
                            .await?;
                            continue;
                        }
                        let mut message = ChatMessage {
                            id: 0,
                            channel: channel.clone(),
                            author: user.clone(),
                            text,
                            timestamp: now_secs(),
                            edited: false,
                        };
                        message.id = state.lock().unwrap().db.save_message(&message)?;
                        let _ = tx.send(ServerMsg::Message { message });
                    }
                    ClientMsg::Delete { id } => {
                        let deleted = state.lock().unwrap().db.delete_message(id, &user)?;
                        match deleted {
                            Some(channel) => {
                                let _ = tx.send(ServerMsg::MessageDeleted { channel, id });
                            }
                            None => {
                                send(&mut writer, &ServerMsg::Error {
                                    text: "you can only delete your own messages".into(),
                                })
                                .await?;
                            }
                        }
                    }
                    ClientMsg::Edit { id, text } => {
                        let text = text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        if text.chars().count() > MAX_TEXT_LEN {
                            send(&mut writer, &ServerMsg::Error {
                                text: format!("message longer than {MAX_TEXT_LEN} characters"),
                            })
                            .await?;
                            continue;
                        }
                        let edited = state.lock().unwrap().db.edit_message(id, &user, &text)?;
                        match edited {
                            Some(channel) => {
                                let _ = tx.send(ServerMsg::MessageEdited { channel, id, text });
                            }
                            None => {
                                send(&mut writer, &ServerMsg::Error {
                                    text: "you can only edit your own messages".into(),
                                })
                                .await?;
                            }
                        }
                    }
                }
            }

            event = rx.recv() => {
                match event {
                    Ok(ServerMsg::Message { message }) => {
                        if message.channel == channel {
                            send(&mut writer, &ServerMsg::Message { message }).await?;
                        } else {
                            // Tell the client there is unread activity, without the content.
                            send(&mut writer, &ServerMsg::Activity { channel: message.channel }).await?;
                        }
                    }
                    Ok(ServerMsg::System { channel: c, text }) if c == channel => {
                        send(&mut writer, &ServerMsg::System { channel: c, text }).await?;
                    }
                    Ok(ServerMsg::MessageEdited { channel: c, id, text }) if c == channel => {
                        send(&mut writer, &ServerMsg::MessageEdited { channel: c, id, text }).await?;
                    }
                    Ok(ServerMsg::MessageDeleted { channel: c, id }) if c == channel => {
                        send(&mut writer, &ServerMsg::MessageDeleted { channel: c, id }).await?;
                    }
                    Ok(presence @ ServerMsg::Presence { .. }) => {
                        send(&mut writer, &presence).await?;
                    }
                    Ok(icon @ ServerMsg::ServerIcon { .. }) => {
                        send(&mut writer, &icon).await?;
                    }
                    Ok(updated @ ServerMsg::ServerUpdated { .. }) => {
                        send(&mut writer, &updated).await?;
                    }
                    Ok(_) => {}
                    // Slow client fell behind; skip the dropped events and keep going.
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    let _ = tx.send(ServerMsg::System {
        channel,
        text: format!("{user} disconnected"),
    });
    Ok(())
}

/// What a server tells anyone who asks, without logging in.
fn public_info(state: &SharedState) -> ServerMsg {
    let s = state.lock().unwrap();
    ServerMsg::Info {
        name: s.db.server_name().unwrap_or_default(),
        description: s.db.server_description().unwrap_or_default(),
        icon: s.db.server_icon().ok().flatten(),
        online: s.online.iter().collect::<std::collections::HashSet<_>>().len() as u32,
        public: s.db.is_public().unwrap_or(false),
    }
}

/// Takes in the servers a caller knows about, then answers with the servers this one knows,
/// including itself.
fn share_peers(state: &SharedState, known: Vec<String>) -> Vec<String> {
    let s = state.lock().unwrap();
    // Only other servers send a list of addresses; a client just asks.
    if !known.is_empty() {
        let _ = s.db.mark_public();
    }
    for addr in known {
        if addr != s.advertise && discovery::valid_addr(&addr) {
            let _ = s.db.add_peer(&addr);
        }
    }
    let mut addrs = vec![s.advertise.clone()];
    addrs.extend(
        s.db.peers()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| *a != s.advertise),
    );
    addrs
}

/// The address other servers should use to reach this one. A server bound to all interfaces
/// uses its LAN address; a server bound to one address uses that address.
fn lan_address(bind: &str) -> String {
    let Ok(sock) = bind.parse::<SocketAddr>() else {
        return bind.to_string();
    };
    if !sock.ip().is_unspecified() {
        return bind.to_string();
    }
    // Connecting a UDP socket sends nothing; it only picks the interface the LAN goes out on.
    let ip = StdUdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("10.255.255.255:1")?;
            s.local_addr()
        })
        .map(|a| a.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
    format!("{ip}:{}", sock.port())
}

/// Reads a number from an environment variable, or uses the default.
fn env_number<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Asks the auth service who a session token belongs to. Returns the display name.
/// Trust on first use: remembers the certificate fingerprint of a server or the auth service the
/// first time it is seen, and refuses one that has changed since.
fn pin_ok(state: &SharedState, kind: &str, addr: &str, fingerprint: &str) -> bool {
    let key = format!("pin:{kind}:{addr}");
    let s = state.lock().unwrap();
    let remembered = s.db.info_value(&key).ok().flatten();
    match common::tls::check_pin(remembered.as_deref(), fingerprint) {
        Ok(true) => {
            let _ = s.db.set_server_info(&key, fingerprint);
            true
        }
        Ok(false) => true,
        Err(e) => {
            eprintln!("{addr}: {e}");
            false
        }
    }
}

/// Asks the auth service who a session token belongs to. Returns the display name.
async fn verify_session(state: &SharedState, auth_addr: &str, token: &str) -> Result<String, String> {
    let (mut stream, fingerprint) = common::tls::connect_async(auth_addr)
        .await
        .map_err(|e| format!("auth service unavailable: {e}"))?;
    if !pin_ok(state, "auth", auth_addr, &fingerprint) {
        return Err("the auth service's certificate has changed".into());
    }
    let mut request = serde_json::to_string(&AuthRequest::Verify { token: token.to_string() })
        .map_err(|e| e.to_string())?;
    request.push('\n');
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("auth service unavailable: {e}"))?;

    let mut lines = BufReader::new(stream).lines();
    let line = lines
        .next_line()
        .await
        .map_err(|e| format!("auth service unavailable: {e}"))?
        .ok_or_else(|| "auth service closed the connection".to_string())?;
    match serde_json::from_str::<AuthReply>(&line) {
        Ok(AuthReply::Verified { name }) => Ok(name),
        Ok(AuthReply::Error { text }) => Err(text),
        _ => Err("unexpected reply from auth service".into()),
    }
}

struct OnlineGuard {
    state: SharedState,
    tx: broadcast::Sender<ServerMsg>,
    user: String,
}

impl Drop for OnlineGuard {
    fn drop(&mut self) {
        let users = {
            let mut s = self.state.lock().unwrap();
            s.remove_online(&self.user);
            s.presence()
        };
        let _ = self.tx.send(ServerMsg::Presence { users });
    }
}

async fn send<W: AsyncWrite + Unpin>(writer: &mut W, msg: &ServerMsg) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    Ok(())
}

fn normalize_channel(name: &str) -> String {
    name.trim()
        .trim_start_matches('#')
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
