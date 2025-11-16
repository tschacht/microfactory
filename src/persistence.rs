use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{context::Context, paths::data_dir};

/// Metadata captured alongside a persisted workflow context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub config_path: String,
    pub llm_provider: String,
    pub llm_model: String,
    pub max_concurrent_llm: usize,
    pub samples: usize,
    pub k: usize,
    pub adaptive_k: bool,
}

impl SessionMetadata {
    pub fn describe_provider(&self) -> &str {
        &self.llm_provider
    }
}

/// Stored payload including context plus metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEnvelope {
    pub context: Context,
    pub metadata: SessionMetadata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Paused,
    Completed,
    Failed,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionStatus::Running => "running",
            SessionStatus::Paused => "paused",
            SessionStatus::Completed => "completed",
            SessionStatus::Failed => "failed",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "running" => Some(SessionStatus::Running),
            "paused" => Some(SessionStatus::Paused),
            "completed" => Some(SessionStatus::Completed),
            "failed" => Some(SessionStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub status: SessionStatus,
    pub prompt: String,
    pub domain: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub envelope: SessionEnvelope,
    pub status: SessionStatus,
    pub updated_at: i64,
}

/// Simple SQLite-backed store for session data.
#[derive(Clone)]
pub struct SessionStore {
    db_path: PathBuf,
}

impl SessionStore {
    pub fn open(custom_root: Option<PathBuf>) -> Result<Self> {
        let base = custom_root.unwrap_or_else(data_dir);
        if !base.exists() {
            fs::create_dir_all(&base).with_context(|| {
                format!("Failed to create session directory {}", base.display())
            })?;
        }
        let db_path = base.join("sessions.sqlite3");
        let store = Self { db_path };
        store.init_schema()?;
        Ok(store)
    }

    pub fn save(&self, envelope: &SessionEnvelope, status: SessionStatus) -> Result<()> {
        let conn = self.connect()?;
        let context_json = serde_json::to_string(&envelope.context)?;
        let metadata_json = serde_json::to_string(&envelope.metadata)?;
        let now = timestamp();
        conn.execute(
            r#"
            INSERT INTO sessions (session_id, domain, prompt, status, context_json, metadata_json, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(session_id)
            DO UPDATE SET
                domain=excluded.domain,
                prompt=excluded.prompt,
                status=excluded.status,
                context_json=excluded.context_json,
                metadata_json=excluded.metadata_json,
                updated_at=excluded.updated_at
            "#,
            params![
                envelope.context.session_id,
                envelope.context.domain,
                envelope.context.prompt,
                status.as_str(),
                context_json,
                metadata_json,
                now
            ],
        )?;
        Ok(())
    }

    pub fn load(&self, session_id: &str) -> Result<SessionRecord> {
        let conn = self.connect()?;
        let row = conn
            .query_row(
                r#"
                SELECT domain, prompt, status, context_json, metadata_json, updated_at
                FROM sessions
                WHERE session_id = ?1
                "#,
                params![session_id],
                |row| {
                    let status_str: String = row.get(2)?;
                    let context_json: String = row.get(3)?;
                    let metadata_json: String = row.get(4)?;
                    let updated_at: i64 = row.get(5)?;
                    Ok((status_str, context_json, metadata_json, updated_at))
                },
            )
            .with_context(|| format!("Session {} not found", session_id))?;

        let status = SessionStatus::from_str(&row.0)
            .ok_or_else(|| anyhow!("Invalid status '{}' in store", row.0))?;
        let context: Context = serde_json::from_str(&row.1)?;
        let metadata: SessionMetadata = serde_json::from_str(&row.2)?;

        Ok(SessionRecord {
            envelope: SessionEnvelope { context, metadata },
            status,
            updated_at: row.3,
        })
    }

    pub fn list(&self, limit: usize) -> Result<Vec<SessionSummary>> {
        let conn = self.connect()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT session_id, prompt, domain, status, updated_at
            FROM sessions
            ORDER BY updated_at DESC
            LIMIT ?1
            "#,
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let (session_id, prompt, domain, status_str, updated_at) = row?;
            let status = SessionStatus::from_str(&status_str)
                .ok_or_else(|| anyhow!("Invalid status '{}' in store", status_str))?;
            summaries.push(SessionSummary {
                session_id,
                prompt,
                domain,
                status,
                updated_at,
            });
        }
        Ok(summaries)
    }

    fn connect(&self) -> Result<Connection> {
        Connection::open(&self.db_path)
            .with_context(|| format!("Failed to open session database {}", self.db_path.display()))
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.connect()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                session_id TEXT PRIMARY KEY,
                domain TEXT NOT NULL,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL,
                context_json TEXT NOT NULL,
                metadata_json TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            "#,
        )?;
        Ok(())
    }
}

fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn saves_and_loads_session() {
        let temp = tempdir().unwrap();
        let store = SessionStore::open(Some(temp.path().to_path_buf())).unwrap();
        let mut ctx = Context::new("demo task", "code");
        ctx.session_id = "test-session".into();
        let envelope = SessionEnvelope {
            context: ctx.clone(),
            metadata: SessionMetadata {
                config_path: "config.yaml".into(),
                llm_provider: "openai".into(),
                llm_model: "gpt".into(),
                max_concurrent_llm: 2,
                samples: 2,
                k: 2,
                adaptive_k: false,
            },
        };

        store
            .save(&envelope, SessionStatus::Running)
            .expect("Saved");

        let record = store.load("test-session").expect("Loaded");
        assert_eq!(record.envelope.context.prompt, "demo task");
        assert_eq!(record.status, SessionStatus::Running);
        assert_eq!(record.envelope.metadata.llm_provider, "openai");

        let list = store.list(10).expect("Listed");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].session_id, "test-session");
    }
}
