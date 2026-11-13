//! Persistent local store of signed agent biographies.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use crate::identity::Biography;
use crate::{AgentId, Error, Result};

const MAX_BIOGRAPHIES: u64 = 5000;

/// SQLite-backed biography cache. Used by every node, including bootstrap nodes.
#[derive(Clone)]
pub struct BiographyStore {
    connection: std::sync::Arc<std::sync::Mutex<Connection>>,
}

impl BiographyStore {
    pub fn open(path: Option<&Path>) -> Result<Self> {
        let connection = match path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| Error::Unsupported(error.to_string()))?;
                }
                Connection::open(path)
            }
            None => Connection::open_in_memory(),
        }
        .map_err(|error| Error::Unsupported(error.to_string()))?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS biographies (
                    agent_id TEXT PRIMARY KEY,
                    biography TEXT NOT NULL,
                    observed_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );",
            )
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        Ok(Self {
            connection: std::sync::Arc::new(std::sync::Mutex::new(connection)),
        })
    }

    pub fn upsert(&self, biography: &Biography) -> Result<()> {
        biography.verify()?;
        let json = serde_json::to_string(biography)
            .map_err(|error| Error::Serialization(error.to_string()))?;
        let now = now_unix();
        let connection = self
            .connection
            .lock()
            .map_err(|_| Error::Unsupported("biography store lock poisoned".to_owned()))?;
        connection
            .execute(
                "INSERT INTO biographies (agent_id, biography, observed_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(agent_id) DO UPDATE SET
                   biography = excluded.biography,
                   observed_at = excluded.observed_at,
                   updated_at = excluded.updated_at
                 WHERE excluded.updated_at >= biographies.updated_at",
                params![
                    biography.agent_id.to_string(),
                    json,
                    now,
                    biography.updated_at
                ],
            )
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        connection
            .execute(
                "DELETE FROM biographies WHERE agent_id IN (
                    SELECT agent_id FROM biographies ORDER BY observed_at DESC LIMIT -1 OFFSET ?1
                 )",
                params![MAX_BIOGRAPHIES],
            )
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Biography>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| Error::Unsupported("biography store lock poisoned".to_owned()))?;
        let mut statement = connection
            .prepare("SELECT biography FROM biographies ORDER BY updated_at DESC")
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        let mut result = Vec::new();
        for row in rows {
            let json = row.map_err(|error| Error::Unsupported(error.to_string()))?;
            if let Ok(biography) = serde_json::from_str::<Biography>(&json) {
                if biography.verify().is_ok() {
                    result.push(biography);
                }
            }
        }
        Ok(result)
    }

    pub fn get(&self, agent_id: &AgentId) -> Result<Option<Biography>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| Error::Unsupported("biography store lock poisoned".to_owned()))?;
        let json: Option<String> = connection
            .query_row(
                "SELECT biography FROM biographies WHERE agent_id = ?1",
                params![agent_id.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| Error::Unsupported(error.to_string()))?;
        let Some(json) = json else {
            return Ok(None);
        };
        let biography: Biography =
            serde_json::from_str(&json).map_err(|error| Error::Serialization(error.to_string()))?;
        biography.verify()?;
        Ok(Some(biography))
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
