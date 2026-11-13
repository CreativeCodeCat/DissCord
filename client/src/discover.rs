//! Finding servers. Starts from a server you know, asks it for the servers it knows, then asks
//! each of those for its details. Nothing here needs a login, and no central list is used.

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::Sender;
use std::time::Duration;

use common::{ClientMsg, ServerMsg};
use eframe::egui;

use crate::trust;

const TIMEOUT: Duration = Duration::from_secs(3);

/// A server that answered a discovery request.
#[derive(Clone)]
pub struct FoundServer {
    pub addr: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    /// How many people are connected to it right now.
    pub online: u32,
    /// True once other servers know it.
    pub public: bool,
}

/// Sends one request over TLS and reads one reply line.
fn ask(addr: &str, msg: &ClientMsg) -> Result<ServerMsg, String> {
    let (mut stream, fingerprint) = common::tls::connect_blocking(addr, TIMEOUT).map_err(|e| e.to_string())?;
    // Check the certificate before sending anything.
    trust::check(&format!("server:{addr}"), &fingerprint)?;
    let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&reply).map_err(|e| e.to_string())
}

/// Starts a background search. Each server that answers is sent on `found`, and the window is
/// woken up for it.
pub fn start(via: String, found: Sender<FoundServer>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let Ok(ServerMsg::Peers { addrs }) = ask(&via, &ClientMsg::Peers { known: Vec::new() }) else {
            return;
        };
        let mut workers = Vec::new();
        for addr in addrs {
            let found = found.clone();
            let ctx = ctx.clone();
            workers.push(std::thread::spawn(move || {
                if let Ok(ServerMsg::Info { name, description, icon, online, public }) =
                    ask(&addr, &ClientMsg::Info)
                {
                    let _ = found.send(FoundServer { addr, name, description, icon, online, public });
                    ctx.request_repaint();
                }
            }));
        }
        for worker in workers {
            let _ = worker.join();
        }
    });
}
