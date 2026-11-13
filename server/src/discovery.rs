//! Finding other servers without a central list.
//!
//! - Beacons: each server announces its address on the local network (UDP broadcast), so
//!   servers on the same network find each other by themselves.
//! - Peer exchange: every so often each server asks the servers it knows for their lists and
//!   keeps any new ones. A server that stops answering is forgotten after a few failures.
//!   Connections are TLS, and a server's certificate is remembered the first time it is seen.

use std::time::Duration;

use anyhow::anyhow;
use common::{ClientMsg, ServerMsg};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UdpSocket;
use tokio::time::{interval, timeout};

use crate::{pin_ok, SharedState};

const BEACON_PREFIX: &str = "DISSCORD ";
const BEACON_EVERY: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_FAILURES: u32 = 3;

/// True for something that looks like `host:port` and is short enough to store.
pub fn valid_addr(addr: &str) -> bool {
    addr.len() <= 100 && addr.contains(':') && !addr.chars().any(char::is_whitespace)
}

/// Sends a beacon every few seconds and records the servers whose beacons arrive.
pub async fn beacons(state: SharedState, port: u16) {
    let socket = match UdpSocket::bind(("0.0.0.0", port)).await {
        Ok(socket) => socket,
        Err(e) => {
            eprintln!("LAN discovery is off: could not use UDP port {port}: {e}");
            return;
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        eprintln!("LAN discovery is off: {e}");
        return;
    }
    let advertise = state.lock().unwrap().advertise.clone();
    let beacon = format!("{BEACON_PREFIX}{advertise}");
    let mut tick = interval(BEACON_EVERY);
    let mut buf = [0u8; 512];
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let _ = socket.send_to(beacon.as_bytes(), ("255.255.255.255", port)).await;
            }
            received = socket.recv_from(&mut buf) => {
                if let Ok((n, _from)) = received {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if let Some(addr) = text.strip_prefix(BEACON_PREFIX) {
                        if addr != advertise && valid_addr(addr) {
                            let _ = state.lock().unwrap().db.add_peer(addr);
                        }
                    }
                }
            }
        }
    }
}

/// Every so often, asks each known server for its list and keeps anything new.
pub async fn exchange(state: SharedState, every: Duration) {
    let mut tick = interval(every);
    loop {
        tick.tick().await;
        let (advertise, peers) = {
            let s = state.lock().unwrap();
            (s.advertise.clone(), s.db.peers().unwrap_or_default())
        };
        for peer in peers {
            tokio::spawn(exchange_with(state.clone(), advertise.clone(), peer));
        }
    }
}

async fn exchange_with(state: SharedState, advertise: String, peer: String) {
    let known = {
        let s = state.lock().unwrap();
        let mut known = s.db.peers().unwrap_or_default();
        known.push(advertise.clone());
        known
    };
    let result = match timeout(REQUEST_TIMEOUT, common::tls::connect_async(&peer)).await {
        Ok(Ok((stream, fingerprint))) => {
            // Check the certificate before sending anything.
            if pin_ok(&state, "server", &peer, &fingerprint) {
                ask_over(stream, known).await
            } else {
                Err(anyhow!("certificate changed"))
            }
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(e.into()),
    };

    let mut s = state.lock().unwrap();
    match result {
        Ok(addrs) => {
            s.peer_failures.remove(&peer);
            // The peer now has our address, so other servers know this one.
            let _ = s.db.mark_public();
            for addr in addrs {
                if addr != advertise && valid_addr(&addr) {
                    let _ = s.db.add_peer(&addr);
                }
            }
        }
        Err(_) => {
            let failures = s.peer_failures.entry(peer.clone()).or_insert(0);
            *failures += 1;
            if *failures >= MAX_FAILURES {
                s.peer_failures.remove(&peer);
                let _ = s.db.remove_peer(&peer);
            }
        }
    }
}

/// Sends our known servers over an open connection and returns the servers the other end knows.
async fn ask_over<S>(mut stream: S, known: Vec<String>) -> anyhow::Result<Vec<String>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut line = serde_json::to_string(&ClientMsg::Peers { known })?;
    line.push('\n');
    stream.write_all(line.as_bytes()).await?;
    let mut lines = BufReader::new(stream).lines();
    let reply = timeout(REQUEST_TIMEOUT, lines.next_line())
        .await??
        .ok_or_else(|| anyhow!("no reply"))?;
    match serde_json::from_str::<ServerMsg>(&reply)? {
        ServerMsg::Peers { addrs } => Ok(addrs),
        _ => Err(anyhow!("unexpected reply")),
    }
}
