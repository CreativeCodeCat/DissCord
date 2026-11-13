//! SQLite storage for channels and messages. Accounts live in the auth service.

use common::ChatMessage;
use rusqlite::{params, Connection};

/// Messages sent to a user when they join a channel.
pub const HISTORY_LEN: i64 = 200;
const DEFAULT_CHANNELS: [&str; 2] = ["general", "random"];
const DEFAULT_SERVER_NAME: &str = "My server";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS members (
    name       TEXT PRIMARY KEY,
    first_seen INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS server_info (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS channels (
    name TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS reactions (
    message_id INTEGER NOT NULL,
    emoji      TEXT NOT NULL,
    user       TEXT NOT NULL,
    PRIMARY KEY (message_id, emoji, user)
);
CREATE TABLE IF NOT EXISTS session_cache (
    token    TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    verified INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS peers (
    addr TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS messages (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    channel   TEXT NOT NULL,
    author    TEXT NOT NULL,
    text      TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    edited    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_by_channel ON messages (channel, id);
";

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        // Databases made before messages could be edited lack this column.
        let _ = conn.execute("ALTER TABLE messages ADD COLUMN edited INTEGER NOT NULL DEFAULT 0", []);
        // Servers that already have members but no owner: the earliest member becomes the owner.
        conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value)
             SELECT 'owner', name FROM members ORDER BY first_seen, rowid LIMIT 1",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value) VALUES ('server_name', ?1)",
            params![DEFAULT_SERVER_NAME],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value) VALUES ('server_description', '')",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value) VALUES ('server_icon', '')",
            [],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value) VALUES ('public', '0')",
            [],
        )?;
        for channel in DEFAULT_CHANNELS {
            conn.execute("INSERT OR IGNORE INTO channels (name) VALUES (?1)", params![channel])?;
        }
        Ok(Self { conn })
    }

    /// Remembers that this user has been on the server. Later logins don't change it.
    /// The first member ever added becomes the owner. The owner never changes after that.
    pub fn add_member(&self, name: &str, now: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO members (name, first_seen) VALUES (?1, ?2)",
            params![name, now as i64],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO server_info (key, value) VALUES ('owner', ?1)",
            params![name],
        )?;
        Ok(())
    }

    /// The owner of this server, if anyone has joined it yet.
    pub fn owner(&self) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row("SELECT value FROM server_info WHERE key = 'owner'", [], |row| row.get(0))
            .optional()
    }

    /// The name everyone sees for this server.
    pub fn server_name(&self) -> rusqlite::Result<String> {
        self.conn.query_row(
            "SELECT value FROM server_info WHERE key = 'server_name'",
            [],
            |row| row.get(0),
        )
    }

    pub fn server_description(&self) -> rusqlite::Result<String> {
        self.conn.query_row(
            "SELECT value FROM server_info WHERE key = 'server_description'",
            [],
            |row| row.get(0),
        )
    }

    /// The server icon as base64 PNG, or `None` when the server has no icon.
    pub fn server_icon(&self) -> rusqlite::Result<Option<String>> {
        let icon: String = self.conn.query_row(
            "SELECT value FROM server_info WHERE key = 'server_icon'",
            [],
            |row| row.get(0),
        )?;
        Ok(if icon.is_empty() { None } else { Some(icon) })
    }

    pub fn set_server_icon(&self, png_base64: Option<&str>) -> rusqlite::Result<()> {
        self.set_server_info("server_icon", png_base64.unwrap_or(""))
    }

    pub fn set_server_name(&self, name: &str) -> rusqlite::Result<()> {
        self.set_server_info("server_name", name)
    }

    pub fn set_server_description(&self, description: &str) -> rusqlite::Result<()> {
        self.set_server_info("server_description", description)
    }

    /// A value from the server_info table, if it is set.
    pub fn info_value(&self, key: &str) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row("SELECT value FROM server_info WHERE key = ?1", params![key], |row| row.get(0))
            .optional()
    }

    pub fn set_server_info(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO server_info (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// True once another server has got in touch with this one, so other servers know it.
    pub fn is_public(&self) -> rusqlite::Result<bool> {
        let value: String = self.conn.query_row(
            "SELECT value FROM server_info WHERE key = 'public'",
            [],
            |row| row.get(0),
        )?;
        Ok(value == "1")
    }

    pub fn mark_public(&self) -> rusqlite::Result<()> {
        self.set_server_info("public", "1")
    }

    /// Addresses of other servers this one knows about.
    pub fn peers(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT addr FROM peers ORDER BY addr")?;
        let addrs = stmt.query_map([], |row| row.get(0))?;
        addrs.collect()
    }

    pub fn add_peer(&self, addr: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO peers (addr) VALUES (?1)", params![addr])?;
        Ok(())
    }

    pub fn remove_peer(&self, addr: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM peers WHERE addr = ?1", params![addr])?;
        Ok(())
    }

    /// Remembers that the auth service confirmed this token at time `verified`.
    pub fn cache_session(&self, token: &str, name: &str, verified: u64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_cache (token, name, verified) VALUES (?1, ?2, ?3)
             ON CONFLICT(token) DO UPDATE SET name = excluded.name, verified = excluded.verified",
            params![token, name, verified as i64],
        )?;
        Ok(())
    }

    /// The name and last confirmed time for a token that was verified before, if any.
    pub fn cached_session(&self, token: &str) -> rusqlite::Result<Option<(String, u64)>> {
        use rusqlite::OptionalExtension;
        let row = self
            .conn
            .query_row(
                "SELECT name, verified FROM session_cache WHERE token = ?1",
                params![token],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .optional()?;
        Ok(row)
    }

    pub fn forget_cached_session(&self, token: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM session_cache WHERE token = ?1", params![token])?;
        Ok(())
    }

    /// Everyone who has been a member, including anyone who has posted a message.
    pub fn member_names(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM members UNION SELECT author FROM messages ORDER BY 1",
        )?;
        let names = stmt.query_map([], |row| row.get(0))?;
        names.collect()
    }

    pub fn ensure_channel(&self, name: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO channels (name) VALUES (?1)", params![name])?;
        Ok(())
    }

    pub fn channel_names(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT name FROM channels ORDER BY name")?;
        let names = stmt.query_map([], |row| row.get(0))?;
        names.collect()
    }

    /// Saves a message and returns its new id.
    pub fn save_message(&self, m: &ChatMessage) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO messages (channel, author, text, timestamp) VALUES (?1, ?2, ?3, ?4)",
            params![m.channel, m.author, m.text, m.timestamp as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Deletes a message, but only when `author` wrote it. Returns the message's channel, or
    /// `None` when there is no such message by that author.
    pub fn delete_message(&self, id: i64, author: &str) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let channel: Option<String> = self
            .conn
            .query_row(
                "SELECT channel FROM messages WHERE id = ?1 AND author = ?2",
                params![id, author],
                |row| row.get(0),
            )
            .optional()?;
        if channel.is_some() {
            self.conn.execute("DELETE FROM reactions WHERE message_id = ?1", params![id])?;
            self.conn.execute("DELETE FROM messages WHERE id = ?1", params![id])?;
        }
        Ok(channel)
    }

    /// Adds `user`'s reaction to a message, or removes it if they already reacted with that emoji.
    /// Returns the message's channel and its reactions afterwards, or `None` if there is no such
    /// message.
    pub fn toggle_reaction(
        &self,
        id: i64,
        emoji: &str,
        user: &str,
    ) -> rusqlite::Result<Option<(String, Vec<common::Reaction>)>> {
        use rusqlite::OptionalExtension;
        let channel: Option<String> = self
            .conn
            .query_row("SELECT channel FROM messages WHERE id = ?1", params![id], |row| row.get(0))
            .optional()?;
        let Some(channel) = channel else {
            return Ok(None);
        };
        let exists = self.conn.execute(
            "DELETE FROM reactions WHERE message_id = ?1 AND emoji = ?2 AND user = ?3",
            params![id, emoji, user],
        )?;
        if exists == 0 {
            self.conn.execute(
                "INSERT INTO reactions (message_id, emoji, user) VALUES (?1, ?2, ?3)",
                params![id, emoji, user],
            )?;
        }
        Ok(Some((channel, self.reactions_for(&[id])?.remove(&id).unwrap_or_default())))
    }

    /// Reactions for the given messages, keyed by message id. Each emoji appears once, in the
    /// order it was first added.
    fn reactions_for(&self, ids: &[i64]) -> rusqlite::Result<std::collections::HashMap<i64, Vec<common::Reaction>>> {
        let mut out: std::collections::HashMap<i64, Vec<common::Reaction>> = Default::default();
        if ids.is_empty() {
            return Ok(out);
        }
        let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT message_id, emoji, user FROM reactions WHERE message_id IN ({list}) ORDER BY rowid"
        ))?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))?;
        for row in rows {
            let (message, emoji, user) = row?;
            let reactions = out.entry(message).or_default();
            match reactions.iter_mut().find(|r| r.emoji == emoji) {
                Some(r) => r.users.push(user),
                None => reactions.push(common::Reaction { emoji, users: vec![user] }),
            }
        }
        Ok(out)
    }

    /// Changes the text of a message, but only when `author` wrote it. Returns the message's
    /// channel, or `None` when there is no such message by that author.
    pub fn edit_message(&self, id: i64, author: &str, text: &str) -> rusqlite::Result<Option<String>> {
        use rusqlite::OptionalExtension;
        let changed = self.conn.execute(
            "UPDATE messages SET text = ?1, edited = 1 WHERE id = ?2 AND author = ?3",
            params![text, id, author],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.conn
            .query_row("SELECT channel FROM messages WHERE id = ?1", params![id], |row| row.get(0))
            .optional()
    }

    /// The most recent `HISTORY_LEN` messages in a channel, oldest first.
    pub fn history(&self, channel: &str) -> rusqlite::Result<Vec<ChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel, author, text, timestamp, edited FROM (
                 SELECT id, channel, author, text, timestamp, edited
                 FROM messages WHERE channel = ?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![channel, HISTORY_LEN], |row| {
            Ok(ChatMessage {
                id: row.get(0)?,
                channel: row.get(1)?,
                author: row.get(2)?,
                text: row.get(3)?,
                timestamp: row.get::<_, i64>(4)? as u64,
                edited: row.get::<_, i64>(5)? != 0,
                reactions: Vec::new(),
            })
        })?;
        let mut messages: Vec<ChatMessage> = rows.collect::<rusqlite::Result<_>>()?;
        let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
        let mut reactions = self.reactions_for(&ids)?;
        for m in &mut messages {
            m.reactions = reactions.remove(&m.id).unwrap_or_default();
        }
        Ok(messages)
    }
}
