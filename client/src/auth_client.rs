//! Blocking requests to the auth service (register, log in, log out).
//! Set the `DISSCORD_AUTH` environment variable to use a different auth service.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use common::{AuthReply, AuthRequest, DEFAULT_AUTH_ADDR};

pub fn auth_addr() -> String {
    std::env::var("DISSCORD_AUTH").unwrap_or_else(|_| DEFAULT_AUTH_ADDR.to_string())
}

/// Sends one request and returns the reply.
pub fn request(req: &AuthRequest) -> Result<AuthReply, String> {
    let addr = auth_addr();
    let socket_addr = addr
        .to_socket_addrs()
        .map_err(|e| format!("invalid auth service address: {e}"))?
        .next()
        .ok_or_else(|| "auth service address did not resolve".to_string())?;

    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(4))
        .map_err(|e| connect_error_message(&addr, &e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(8)))
        .map_err(|e| e.to_string())?;

    let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    line.push('\n');
    stream.write_all(line.as_bytes()).map_err(|e| e.to_string())?;

    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&reply).map_err(|e| format!("bad reply from auth service: {e}"))
}

/// Turns a connection error into a message that says what to do about it.
fn connect_error_message(addr: &str, e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::ConnectionRefused => format!(
            "Nothing is listening at {addr}. Start the auth service with `cargo run -p auth`, \
             or set DISSCORD_AUTH to the address it is running on."
        ),
        std::io::ErrorKind::TimedOut => format!(
            "The auth service at {addr} did not answer in time. Check that it is running and reachable."
        ),
        _ => format!("Could not reach the auth service at {addr}: {e}"),
    }
}
