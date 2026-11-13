//! The auth service owns every account. Chat servers ask it who a session token
//! belongs to, so one account works on every server.
//!
//! Usage: `auth [ADDR] [DB_PATH]`

mod db;

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use common::{is_valid_email, AuthReply, AuthRequest, DEFAULT_AUTH_ADDR};
use db::Db;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const DEFAULT_DB_PATH: &str = "auth.db";
const MAX_NAME_LEN: usize = 24;
const MIN_PASSWORD_LEN: usize = 8;
const MAX_PASSWORD_LEN: usize = 128;

type SharedDb = Arc<Mutex<Db>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let addr = args.next().unwrap_or_else(|| DEFAULT_AUTH_ADDR.to_string());
    let db_path = args.next().unwrap_or_else(|| DEFAULT_DB_PATH.to_string());

    let key = load_email_key(&db_path)?;
    let mut db = Db::open(&db_path, key)?;
    let converted = db.hash_plain_emails()?;
    if converted > 0 {
        println!("hashed {converted} stored email address(es)");
    }
    // The service's own certificate. It is made the first time the service runs.
    let (cert, key) = match db.tls_identity()? {
        Some(identity) => identity,
        None => {
            let identity = common::tls::new_identity()?;
            db.store_tls_identity(&identity.0, &identity.1)?;
            identity
        }
    };
    let acceptor = common::tls::acceptor(&cert, &key)?;
    let db: SharedDb = Arc::new(Mutex::new(db));
    let listener = TcpListener::bind(&addr).await?;
    println!("DissCord auth service listening on {addr} (TLS), storing accounts in {db_path}");

    loop {
        let (socket, peer) = listener.accept().await?;
        let db = db.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let socket = match acceptor.accept(socket).await {
                Ok(socket) => socket,
                Err(e) => {
                    eprintln!("{peer}: TLS handshake failed: {e}");
                    return;
                }
            };
            if let Err(e) = handle_connection(socket, db).await {
                if !common::tls::is_quiet_close(&e) {
                    eprintln!("{peer}: {e}");
                }
            }
        });
    }
}

/// The secret used to hash emails. It is kept in `<DB_PATH>.key`, made the first time the
/// service runs. Keep it apart from the database: someone with only the database cannot read
/// the emails.
fn load_email_key(db_path: &str) -> anyhow::Result<[u8; 32]> {
    let path = format!("{db_path}.key");
    if let Ok(text) = std::fs::read_to_string(&path) {
        let bytes = hex_decode(text.trim()).filter(|b| b.len() == 32);
        return bytes
            .map(|b| b.try_into().expect("checked length"))
            .ok_or_else(|| anyhow::anyhow!("{path} does not hold a valid key"));
    }
    // No key file yet. That is only safe when no account has a hashed email, because a new key
    // could not match those.
    if std::path::Path::new(db_path).exists() {
        let existing = Db::open(db_path, [0; 32])?;
        if existing.hashed_account_count()? > 0 {
            anyhow::bail!("{path} is missing, but accounts in {db_path} need it to log in");
        }
    }
    let key: [u8; 32] = rand::random();
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&path, hex)?;
    println!("made a new email key in {path}");
    Ok(key)
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Reads requests line by line until the client disconnects.
async fn handle_connection<S>(socket: S, db: SharedDb) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, mut writer) = tokio::io::split(socket);
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let reply = match serde_json::from_str::<AuthRequest>(&line) {
            Ok(request) => handle_request(&db, request),
            Err(e) => AuthReply::Error { text: format!("bad request: {e}") },
        };
        let mut out = serde_json::to_string(&reply)?;
        out.push('\n');
        writer.write_all(out.as_bytes()).await?;
    }
    Ok(())
}

fn handle_request(db: &SharedDb, request: AuthRequest) -> AuthReply {
    let result = match request {
        AuthRequest::Register { name, email, password } => register(db, &name, &email, &password),
        AuthRequest::Login { email, password } => login(db, &email, &password),
        AuthRequest::Verify { token } => {
            return match db.lock().unwrap().session_name(&token) {
                Ok(Some(name)) => AuthReply::Verified { name },
                Ok(None) => AuthReply::Error { text: "your session has expired, log in again".into() },
                Err(e) => AuthReply::Error { text: e.to_string() },
            };
        }
        AuthRequest::Logout { token } => {
            return match db.lock().unwrap().delete_session(&token) {
                Ok(()) => AuthReply::LoggedOut,
                Err(e) => AuthReply::Error { text: e.to_string() },
            };
        }
    };
    match result {
        Ok((token, name)) => AuthReply::Session { token, name },
        Err(text) => AuthReply::Error { text },
    }
}

/// Creates an account and a session. Returns `(token, name)`.
fn register(db: &SharedDb, name: &str, email: &str, password: &str) -> Result<(String, String), String> {
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(format!("name must be 1-{MAX_NAME_LEN} characters"));
    }
    let email = normalize_email(email);
    if !is_valid_email(&email) {
        return Err("enter a valid email address".into());
    }
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(format!("password must be at least {MIN_PASSWORD_LEN} characters"));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(format!("password must be at most {MAX_PASSWORD_LEN} characters"));
    }

    let hash = hash_password(password)?;
    let token = new_token();
    let guard = db.lock().unwrap();
    let created = guard
        .create_account(&name, &email, &hash, now_secs())
        .map_err(|e| e.to_string())?;
    if !created {
        return Err("that name or email is already registered".into());
    }
    guard
        .create_session(&token, &email, now_secs())
        .map_err(|e| e.to_string())?;
    Ok((token, name))
}

/// Checks an email and password. Returns `(token, name)`.
fn login(db: &SharedDb, email: &str, password: &str) -> Result<(String, String), String> {
    let email = normalize_email(email);
    // Look the account up, then release the lock before the slow password check.
    let account = db
        .lock()
        .unwrap()
        .find_account(&email)
        .map_err(|e| e.to_string())?;
    let account = match account {
        Some(acc) if verify_password(password, &acc.password_hash) => acc,
        _ => return Err("wrong email or password".into()),
    };

    let token = new_token();
    db.lock()
        .unwrap()
        .create_session(&token, &email, now_secs())
        .map_err(|e| e.to_string())?;
    Ok((token, account.name))
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Hashes with Argon2 and a random salt. The result is a self-describing string.
fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

/// A random 256-bit session token, hex-encoded.
fn new_token() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
