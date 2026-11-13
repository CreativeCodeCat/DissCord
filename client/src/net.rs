//! Blocking networking for the GUI. One background thread per connection owns the TLS
//! socket: it sends what the UI queues and reads server lines, then forwards them to the UI.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Duration;

use common::{ClientMsg, ServerMsg};

use crate::trust;

pub enum NetEvent {
    Server(ServerMsg),
    Disconnected(String),
}

pub struct Connection {
    pub outgoing: Sender<ClientMsg>,
    pub incoming: Receiver<NetEvent>,
    /// Second handle to the socket, used to close it when the connection is dropped.
    socket: TcpStream,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing the socket makes the server see a clean disconnect and ends the thread's reads.
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

/// How long the thread waits for data before it checks for messages to send.
const POLL: Duration = Duration::from_millis(100);

/// Connects over TLS, checks the server's certificate, sends the login, and starts the thread.
/// `wake` is called whenever new data arrives so the UI repaints promptly.
pub fn connect(
    addr: &str,
    first: ClientMsg,
    wake: impl Fn() + Send + 'static,
) -> Result<Connection, String> {
    let (mut stream, fingerprint) = common::tls::connect_blocking(addr, Duration::from_secs(4))
        .map_err(|e| format!("could not connect: {e}"))?;
    // Check the certificate before the login is sent.
    trust::check(&format!("server:{addr}"), &fingerprint)?;

    let socket = stream.sock.try_clone().map_err(|e| e.to_string())?;
    stream.sock.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;

    let (out_tx, out_rx) = mpsc::channel::<ClientMsg>();
    let (in_tx, in_rx) = mpsc::channel::<NetEvent>();

    // The first message logs in or registers; the server answers with Welcome or Error.
    out_tx.send(first).map_err(|e| e.to_string())?;

    std::thread::spawn(move || {
        let reason = pump(&mut stream, &out_rx, &in_tx, &wake);
        if let Some(reason) = reason {
            let _ = in_tx.send(NetEvent::Disconnected(reason));
            wake();
        }
    });

    Ok(Connection { outgoing: out_tx, incoming: in_rx, socket })
}

/// Runs until the connection ends. Returns why, or None when the UI has already let go of it.
fn pump(
    stream: &mut common::tls::Stream,
    out_rx: &Receiver<ClientMsg>,
    in_tx: &Sender<NetEvent>,
    wake: &impl Fn(),
) -> Option<String> {
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Send everything the UI has queued.
        loop {
            match out_rx.try_recv() {
                Ok(msg) => {
                    let Ok(mut line) = serde_json::to_string(&msg) else { continue };
                    line.push('\n');
                    if let Err(e) = stream.write_all(line.as_bytes()) {
                        return Some(e.to_string());
                    }
                }
                Err(TryRecvError::Empty) => break,
                // The UI dropped the connection.
                Err(TryRecvError::Disconnected) => return None,
            }
        }

        match stream.read(&mut chunk) {
            Ok(0) => return Some("connection closed by server".into()),
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    if let Ok(msg) = serde_json::from_slice::<ServerMsg>(&line[..line.len() - 1]) {
                        if in_tx.send(NetEvent::Server(msg)).is_err() {
                            return None;
                        }
                        wake();
                    }
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Some(e.to_string()),
        }
    }
}
