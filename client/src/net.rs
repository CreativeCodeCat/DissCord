//! Blocking networking for the GUI. Two background threads per connection:
//! one reads server lines and forwards them to the UI, the other writes
//! client messages from the UI to the socket.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use common::{ClientMsg, ServerMsg};

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
        // Closing the socket makes the server see a clean disconnect and unblocks the reader thread.
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

/// Connects, sends the login, and starts the reader/writer threads.
/// `wake` is called whenever new data arrives so the UI repaints promptly.
pub fn connect(
    addr: &str,
    first: ClientMsg,
    wake: impl Fn() + Send + Clone + 'static,
) -> Result<Connection, String> {
    let socket_addr = addr
        .to_socket_addrs()
        .map_err(|e| format!("invalid address: {e}"))?
        .next()
        .ok_or_else(|| "address did not resolve".to_string())?;

    let stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(4))
        .map_err(|e| format!("could not connect: {e}"))?;
    let read_half = stream.try_clone().map_err(|e| e.to_string())?;
    let socket = stream.try_clone().map_err(|e| e.to_string())?;

    let (out_tx, out_rx) = mpsc::channel::<ClientMsg>();
    let (in_tx, in_rx) = mpsc::channel::<NetEvent>();

    // Writer thread
    let mut write_half = stream;
    std::thread::spawn(move || {
        for msg in out_rx {
            let Ok(mut line) = serde_json::to_string(&msg) else { continue };
            line.push('\n');
            if write_half.write_all(line.as_bytes()).is_err() {
                break;
            }
        }
    });

    // Reader thread
    {
        let in_tx = in_tx.clone();
        let wake = wake.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(read_half);
            let mut reason = String::from("connection closed by server");
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if let Ok(msg) = serde_json::from_str::<ServerMsg>(&line) {
                            if in_tx.send(NetEvent::Server(msg)).is_err() {
                                return;
                            }
                            wake();
                        }
                    }
                    Err(e) => {
                        reason = e.to_string();
                        break;
                    }
                }
            }
            let _ = in_tx.send(NetEvent::Disconnected(reason));
            wake();
        });
    }

    // The first message logs in or registers; the server answers with Welcome or Error.
    out_tx.send(first).map_err(|e| e.to_string())?;

    Ok(Connection { outgoing: out_tx, incoming: in_rx, socket })
}
