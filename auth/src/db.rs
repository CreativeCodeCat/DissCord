//! SQLite storage for accounts and login sessions.

use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

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
";

/// A stored account, as needed for logging in.
pub struct Account {
    pub name: String,
    pub password_hash: String,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn find_account(&self, email: &str) -> rusqlite::Result<Option<Account>> {
        self.conn
            .query_row(
                "SELECT name, password_hash FROM accounts WHERE email = ?1",
                params![email],
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
            params![email, name, password_hash, now as i64],
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
            params![token, email, now as i64],
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

    pub fn delete_session(&self, token: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM sessions WHERE token = ?1", params![token])?;
        Ok(())
    }
}
