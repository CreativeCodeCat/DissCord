//! SQLite storage for accounts and login sessions.

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};
use sha2::{Digest, Sha256};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    email         TEXT PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created       INTEGER NOT NULL
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
        Ok(Self { conn, key })
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
        email: &str,
        password_hash: &str,
        now: u64,
    ) -> rusqlite::Result<bool> {
        let result = self.conn.execute(
            "INSERT INTO accounts (email, name, password_hash, created) VALUES (?1, ?2, ?3, ?4)",
            params![hash_email(&self.key, email), name, password_hash, now as i64],
        );
        match result {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn create_session(&self, token: &str, email: &str, now: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (token, email, created) VALUES (?1, ?2, ?3)",
            params![token, hash_email(&self.key, email), now as i64],
        )?;
        Ok(())
    }

    /// The display name a session token belongs to, if the token is valid.
    pub fn session_name(&self, token: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT accounts.name FROM sessions
                 JOIN accounts ON accounts.email = sessions.email
                 WHERE sessions.token = ?1",
                params![token],
                |row| row.get(0),
            )
            .optional()
    }

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
