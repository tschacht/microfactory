use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use async_trait::async_trait;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{
    core::{
        domain::Context,
        error::Error as CoreError,
        ports::{SessionLoadResponse, SessionRepository, SessionSaveRequest},
    },
    paths::data_dir,
};

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
    #[serde(default = "default_low_margin_threshold")]
    pub human_low_margin_threshold: usize,
}

impl SessionMetadata {
    pub fn describe_provider(&self) -> &str {
        &self.llm_provider
    }
}

fn default_low_margin_threshold() -> usize {
    1
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
            .with_context(|| format!("Session {session_id} not found"))?;

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
                .ok_or_else(|| anyhow!("Invalid status '{status_str}' in store"))?;
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

#[async_trait]
impl SessionRepository for SessionStore {
    async fn save_session(&self, request: &SessionSaveRequest) -> crate::core::Result<()> {
        let store = self.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let conn = store
                .connect()
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
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
                    request.session_id,
                    request.domain,
                    request.prompt,
                    request.status,
                    request.context_json,
                    request.metadata_json,
                    now
                ],
            )
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::System(format!("Join error: {e}")))?
    }

    async fn load_session(
        &self,
        session_id: &str,
    ) -> crate::core::Result<Option<SessionLoadResponse>> {
        let store = self.clone();
        let session_id = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = store
                .connect()
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            let mut stmt = conn
                .prepare(
                    r#"
                SELECT domain, prompt, status, context_json, metadata_json, updated_at
                FROM sessions
                WHERE session_id = ?1
                "#,
                )
                .map_err(|e| CoreError::Persistence(e.to_string()))?;

            let mut rows = stmt
                .query(params![session_id])
                .map_err(|e| CoreError::Persistence(e.to_string()))?;

            if let Some(row) = rows
                .next()
                .map_err(|e| CoreError::Persistence(e.to_string()))?
            {
                Ok(Some(SessionLoadResponse {
                    session_id,
                    domain: row
                        .get(0)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    prompt: row
                        .get(1)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    status: row
                        .get(2)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    context_json: row
                        .get(3)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    metadata_json: row
                        .get(4)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                    updated_at: row
                        .get(5)
                        .map_err(|e| CoreError::Persistence(e.to_string()))?,
                }))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| CoreError::System(format!("Join error: {e}")))?
    }

    async fn list_sessions(&self, limit: usize) -> crate::core::Result<Vec<SessionLoadResponse>> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let conn = store
                .connect()
                .map_err(|e| CoreError::Persistence(e.to_string()))?;
            let mut stmt = conn
                .prepare(
                    r#"
                SELECT session_id, domain, prompt, status, context_json, metadata_json, updated_at
                FROM sessions
                ORDER BY updated_at DESC
                LIMIT ?1
                "#,
                )
                .map_err(|e| CoreError::Persistence(e.to_string()))?;

            let rows = stmt
                .query_map(params![limit as i64], |row| {
                    Ok(SessionLoadResponse {
                        session_id: row.get(0)?,
                        domain: row.get(1)?,
                        prompt: row.get(2)?,
                        status: row.get(3)?,
                        context_json: row.get(4)?,
                        metadata_json: row.get(5)?,
                        updated_at: row.get(6)?,
                    })
                })
                .map_err(|e| CoreError::Persistence(e.to_string()))?;

            let mut result = Vec::new();
            for row in rows {
                result.push(row.map_err(|e| CoreError::Persistence(e.to_string()))?);
            }
            Ok(result)
        })
        .await
        .map_err(|e| CoreError::System(format!("Join error: {e}")))?
    }
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
                human_low_margin_threshold: 1,
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
