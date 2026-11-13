//! SQLite storage for accounts and login sessions.

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    email         TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created       INTEGER NOT NULL,
    email_enc     TEXT
);
CREATE TABLE IF NOT EXISTS sessions (
    token   TEXT PRIMARY KEY,
    email   TEXT NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tls_identity (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    cert TEXT NOT NULL,
    key  TEXT NOT NULL
);
";

/// The account behind a session token.
pub struct SessionAccount {
    /// The account's hidden id. Chat servers use this to identify people.
    pub id: String,
    /// The stored (hashed) email, which also identifies the account.
    pub email_hash: String,
    /// The name shown in chat.
    pub name: String,
    /// The lowercase handle.
    pub username: String,
    pub password_hash: String,
    pub email_enc: Option<String>,
}

/// A username made from a name: lowercase letters, digits and underscores only.
pub fn username_from(name: &str) -> String {
    let cleaned: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(20)
        .collect();
    if cleaned.len() < 2 { format!("user_{cleaned}") } else { cleaned }
}

/// `Ok(true)` when the statement ran, `Ok(false)` when it broke a uniqueness rule.
fn constraint_ok(result: rusqlite::Result<usize>) -> rusqlite::Result<bool> {
    match result {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => Ok(false),
        Err(e) => Err(e),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// A stored account, as needed for logging in.
pub struct Account {
    pub name: String,
    pub password_hash: String,
}

pub struct Db {
    conn: Connection,
    /// Secret that emails are hashed with. It lives in a file beside the database.
    key: [u8; 32],
}

/// Hashes an email (already trimmed and lowercased) with the secret key. The result is what is
/// stored in place of the address, so the database alone does not reveal anyone's email.
fn hash_email(key: &[u8; 32], email: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(email.as_bytes());
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl Db {
    pub fn open(path: &str, key: [u8; 32]) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        // Databases made before emails could be shown lack this column.
        let _ = conn.execute("ALTER TABLE accounts ADD COLUMN email_enc TEXT", []);
        // Accounts made before ids and usernames existed. An account's id starts as its name,
        // so the messages it wrote still belong to it. Its username is its name in lowercase.
        let _ = conn.execute("ALTER TABLE accounts ADD COLUMN id TEXT", []);
        let _ = conn.execute("ALTER TABLE accounts ADD COLUMN username TEXT", []);
        conn.execute("UPDATE accounts SET id = name WHERE id IS NULL", [])?;
        let mut db = Self { conn, key };
        db.fill_usernames()?;
        db.conn.execute_batch(
            "CREATE UNIQUE INDEX IF NOT EXISTS accounts_id ON accounts (id);
             CREATE UNIQUE INDEX IF NOT EXISTS accounts_username ON accounts (username);",
        )?;
        Ok(db)
    }

    /// Gives every account without a username one, from its name. If the name is taken as a
    /// username already, a number is added.
    fn fill_usernames(&mut self) -> rusqlite::Result<()> {
        let missing: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare("SELECT email, name FROM accounts WHERE username IS NULL")?;
            let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (email, name) in missing {
            let base = username_from(&name);
            let mut candidate = base.clone();
            let mut n = 2;
            while self.username_taken(&candidate)? {
                candidate = format!("{base}{n}");
                n += 1;
            }
            self.conn.execute(
                "UPDATE accounts SET username = ?1 WHERE email = ?2",
                params![candidate, email],
            )?;
        }
        Ok(())
    }

    pub fn username_taken(&self, username: &str) -> rusqlite::Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Sets the display name for the account with this (hashed) email. Returns `false` if the
    /// name is already used by another account.
    pub fn set_display_name(&self, email_hash: &str, name: &str) -> rusqlite::Result<bool> {
        constraint_ok(self.conn.execute(
            "UPDATE accounts SET name = ?1 WHERE email = ?2",
            params![name, email_hash],
        ))
    }

    /// Sets the username for the account with this (hashed) email. Returns `false` if it is taken.
    pub fn set_username(&self, email_hash: &str, username: &str) -> rusqlite::Result<bool> {
        constraint_ok(self.conn.execute(
            "UPDATE accounts SET username = ?1 WHERE email = ?2",
            params![username, email_hash],
        ))
    }

    /// Accounts and sessions still holding a plain email (from before emails were hashed) are
    /// rewritten as hashes. Returns how many addresses were converted.
    pub fn hash_plain_emails(&mut self) -> rusqlite::Result<usize> {
        let plain: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT email FROM accounts WHERE instr(email, '@') > 0
                 UNION SELECT email FROM sessions WHERE instr(email, '@') > 0",
            )?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let tx = self.conn.transaction()?;
        for email in &plain {
            let hashed = hash_email(&self.key, email);
            tx.execute("UPDATE accounts SET email = ?1 WHERE email = ?2", params![hashed, email])?;
            tx.execute("UPDATE sessions SET email = ?1 WHERE email = ?2", params![hashed, email])?;
        }
        tx.commit()?;
        Ok(plain.len())
    }

    /// Accounts whose email is already hashed. These need the key file to log in.
    pub fn hashed_account_count(&self) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM accounts WHERE instr(email, '@') = 0",
            [],
            |row| row.get(0),
        )
    }

    pub fn find_account(&self, email: &str) -> rusqlite::Result<Option<Account>> {
        self.conn
            .query_row(
                "SELECT name, password_hash FROM accounts WHERE email = ?1",
                params![hash_email(&self.key, email)],
                |row| {
                    Ok(Account {
                        name: row.get(0)?,
                        password_hash: row.get(1)?,
                    })
                },
            )
            .optional()
    }

    /// Creates an account. Returns `false` if the email or the name is already taken.
    pub fn create_account(
        &self,
        name: &str,
        username: &str,
        email: &str,
        password_hash: &str,
        now: u64,
    ) -> rusqlite::Result<bool> {
        let id: [u8; 16] = rand::random();
        let result = self.conn.execute(
            "INSERT INTO accounts (id, email, name, username, password_hash, created, email_enc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                hex(&id),
                hash_email(&self.key, email),
                name,
                username,
                password_hash,
                now as i64,
                self.encrypt_email(email)
            ],
        );
        constraint_ok(result)
    }

    /// The account a session token belongs to, if the token is valid.
    pub fn session_account(&self, token: &str) -> rusqlite::Result<Option<SessionAccount>> {
        self.conn
            .query_row(
                "SELECT accounts.id, accounts.email, accounts.name, accounts.username,
                        accounts.password_hash, accounts.email_enc
                 FROM sessions JOIN accounts ON accounts.email = sessions.email
                 WHERE sessions.token = ?1",
                params![token],
                |row| {
                    Ok(SessionAccount {
                        id: row.get(0)?,
                        email_hash: row.get(1)?,
                        name: row.get(2)?,
                        username: row.get(3)?,
                        password_hash: row.get(4)?,
                        email_enc: row.get(5)?,
                    })
                },
            )
            .optional()
    }

    /// The email an account was stored with, decrypted. `None` for accounts made before emails
    /// were kept this way, which have no encrypted copy.
    pub fn account_email(&self, account: &SessionAccount) -> Option<String> {
        let enc = account.email_enc.as_deref()?;
        let (nonce_hex, cipher_hex) = enc.split_once(':')?;
        let nonce = unhex(nonce_hex)?;
        let cipher = unhex(cipher_hex)?;
        let key = chacha20poly1305::Key::from_slice(&self.key);
        let plain = ChaCha20Poly1305::new(key).decrypt(Nonce::from_slice(&nonce), cipher.as_slice()).ok()?;
        String::from_utf8(plain).ok()
    }

    /// Replaces the account's email and every session's email. Returns `false` if the new email
    /// is already used by another account.
    pub fn change_email(&mut self, old_hash: &str, new_email: &str) -> rusqlite::Result<bool> {
        let new_hash = hash_email(&self.key, new_email);
        let enc = self.encrypt_email(new_email);
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "UPDATE accounts SET email = ?1, email_enc = ?2 WHERE email = ?3",
            params![new_hash, enc, old_hash],
        );
        match changed {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => return Ok(false),
            Err(e) => return Err(e),
        }
        tx.execute(
            "UPDATE sessions SET email = ?1 WHERE email = ?2",
            params![new_hash, old_hash],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// The email, encrypted with the account key as `nonce:ciphertext` in hex.
    fn encrypt_email(&self, email: &str) -> String {
        let nonce: [u8; 12] = rand::random();
        let key = chacha20poly1305::Key::from_slice(&self.key);
        let cipher = ChaCha20Poly1305::new(key)
            .encrypt(Nonce::from_slice(&nonce), email.as_bytes())
            .unwrap_or_default();
        format!("{}:{}", hex(&nonce), hex(&cipher))
    }

    pub fn create_session(&self, token: &str, email: &str, now: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (token, email, created) VALUES (?1, ?2, ?3)",
            params![token, hash_email(&self.key, email), now as i64],
        )?;
        Ok(())
    }

    /// The display name a session token belongs to, if the token is valid.
    /// The stored TLS certificate and key (PEM), if one has been made.
    pub fn tls_identity(&self) -> rusqlite::Result<Option<(String, String)>> {
        self.conn
            .query_row("SELECT cert, key FROM tls_identity WHERE id = 1", [], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()
    }

    pub fn store_tls_identity(&self, cert: &str, key: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tls_identity (id, cert, key) VALUES (1, ?1, ?2)",
            params![cert, key],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, token: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
        Ok(())
    }
}
