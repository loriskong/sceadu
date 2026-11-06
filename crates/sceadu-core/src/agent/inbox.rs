//! Persistent receiver-side inbox.
//!
//! The inbox stores complete message envelopes as JSON and deduplicates by
//! `message_id`. Files are stored separately by the file transfer layer.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{AgentMessage, Error, Result};

const MAX_INBOX_MESSAGES: i64 = 10_000;

#[derive(Clone)]
pub struct Inbox {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredMessage {
    pub message_id: String,
    pub context_id: String,
    pub from: String,
    pub to: String,
    pub role: String,
    pub received_at: u64,
    pub read: bool,
    pub message: AgentMessage,
}

impl Inbox {
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let conn = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| Error::Message(format!("inbox dir: {error}")))?;
                }
                Connection::open(path)
            }
            None => Connection::open_in_memory(),
        }
        .map_err(|error| Error::Message(format!("inbox open: {error}")))?;
        let inbox = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        inbox.init()?;
        Ok(inbox)
    }

    fn init(&self) -> Result<()> {
        let conn = self.lock()?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             CREATE TABLE IF NOT EXISTS messages (
                 message_id TEXT PRIMARY KEY,
                 context_id TEXT NOT NULL,
                 from_agent TEXT NOT NULL,
                 to_agent TEXT NOT NULL,
                 role TEXT NOT NULL,
                 received_at INTEGER NOT NULL,
                 read INTEGER NOT NULL DEFAULT 0,
                 payload TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_received
                 ON messages(received_at DESC);",
        )
        .map_err(|error| Error::Message(format!("inbox schema: {error}")))?;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| Error::Message("inbox lock poisoned".to_owned()))
    }

    /// Store a message. Returns false when the message ID already exists.
    pub fn ingest(&self, message: &AgentMessage) -> Result<bool> {
        let conn = self.lock()?;
        let payload = serde_json::to_string(message)
            .map_err(|error| Error::Message(format!("inbox encode: {error}")))?;
        let changed = conn
            .execute(
                "INSERT OR IGNORE INTO messages
                 (message_id, context_id, from_agent, to_agent, role, received_at, read, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
                params![
                    message.message_id,
                    message.context_id,
                    message.from,
                    message.to,
                    format!("{:?}", message.role).to_lowercase(),
                    now_unix() as i64,
                    payload
                ],
            )
            .map_err(|error| Error::Message(format!("inbox write: {error}")))?;
        // Keep the inbox bounded. Deleting oldest rows is intentionally
        // simple; file references remain content-addressed on disk.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .map_err(|error| Error::Message(format!("inbox count: {error}")))?;
        if count > MAX_INBOX_MESSAGES {
            conn.execute(
                "DELETE FROM messages WHERE message_id IN (
                     SELECT message_id FROM messages
                     ORDER BY received_at ASC, rowid ASC LIMIT ?1
                 )",
                params![count - MAX_INBOX_MESSAGES],
            )
            .map_err(|error| Error::Message(format!("inbox evict: {error}")))?;
        }
        Ok(changed > 0)
    }

    pub fn list(&self, limit: u32) -> Result<Vec<StoredMessage>> {
        let conn = self.lock()?;
        let limit = limit.clamp(1, 1000) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT message_id, context_id, from_agent, to_agent, role,
                        received_at, read, payload
                 FROM messages ORDER BY received_at DESC, rowid DESC LIMIT ?1",
            )
            .map_err(|error| Error::Message(format!("inbox list: {error}")))?;
        let rows = stmt
            .query_map(params![limit], row_to_message)
            .map_err(|error| Error::Message(format!("inbox list: {error}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| Error::Message(format!("inbox list: {error}")))
    }

    pub fn read(&self, message_id: &str) -> Result<Option<StoredMessage>> {
        let conn = self.lock()?;
        let row = conn
            .query_row(
                "SELECT message_id, context_id, from_agent, to_agent, role,
                        received_at, read, payload
                 FROM messages WHERE message_id = ?1",
                params![message_id],
                row_to_message,
            )
            .optional()
            .map_err(|error| Error::Message(format!("inbox read: {error}")))?;
        Ok(row)
    }

    pub fn mark_read(&self, message_id: &str) -> Result<bool> {
        let conn = self.lock()?;
        let changed = conn
            .execute(
                "UPDATE messages SET read = 1 WHERE message_id = ?1",
                params![message_id],
            )
            .map_err(|error| Error::Message(format!("inbox mark read: {error}")))?;
        Ok(changed > 0)
    }

    pub fn unread_count(&self) -> Result<u64> {
        let conn = self.lock()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM messages WHERE read = 0", [], |row| {
                row.get(0)
            })
            .map_err(|error| Error::Message(format!("inbox unread: {error}")))?;
        Ok(count.max(0) as u64)
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMessage> {
    let payload: String = row.get(7)?;
    let message = serde_json::from_str::<AgentMessage>(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            payload.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })?;
    Ok(StoredMessage {
        message_id: row.get(0)?,
        context_id: row.get(1)?,
        from: row.get(2)?,
        to: row.get(3)?,
        role: row.get(4)?,
        received_at: row.get::<_, i64>(5)? as u64,
        read: row.get::<_, i64>(6)? != 0,
        message,
    })
}
