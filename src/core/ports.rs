use crate::core::error::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;

/// Data transfer object for saving a session.
#[derive(Debug, Clone)]
pub struct SessionSaveRequest {
    pub session_id: String,
    pub domain: String,
    pub prompt: String,
    pub status: String,
    pub context_json: String,
    pub metadata_json: String,
}

/// Data transfer object for loading a session.
#[derive(Debug, Clone)]
pub struct SessionLoadResponse {
    pub session_id: String,
    pub domain: String,
    pub prompt: String,
    pub status: String,
    pub context_json: String,
    pub metadata_json: String,
    pub updated_at: i64,
}

/// Abstraction for storing and retrieving session state.
#[async_trait]
pub trait SessionRepository: Send + Sync {
    /// Save the current context of a session.
    async fn save_session(&self, request: &SessionSaveRequest) -> Result<()>;
    /// Load a session by its ID.
    async fn load_session(&self, session_id: &str) -> Result<Option<SessionLoadResponse>>;
    /// List all available sessions (returning summary info).
    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionLoadResponse>>;
}

/// Abstraction for interacting with an LLM provider.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a chat completion request.
    async fn chat_completion(
        &self,
        model: &str,
        prompt: &str,
        options: &LlmOptions,
    ) -> Result<String>;
}

/// Options for an LLM request.
#[derive(Debug, Clone, Default)]
pub struct LlmOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
    pub reasoning_effort: Option<String>,
}

/// Abstraction for rendering prompt templates.
pub trait PromptRenderer: Send + Sync {
    /// Render a template with the given data.
    fn render(&self, template_name: &str, data: &serde_json::Value) -> Result<String>;
}

/// Abstraction for checking content against safety or quality rules.
#[async_trait]
pub trait RedFlagger: Send + Sync {
    /// Check the given content. Returns `Ok(())` if safe, or `Err(Error::RedFlag)` if not.
    async fn check(&self, content: &str) -> Result<()>;
    /// The name of this red flagger.
    fn name(&self) -> &str;
}

/// Abstraction for file system operations.
pub trait FileSystem: Send + Sync {
    /// Read a file to a string.
    fn read_to_string(&self, path: &Path) -> Result<String>;
    /// Write a string to a file.
    fn write(&self, path: &Path, content: &str) -> Result<()>;
    /// Check if a file exists.
    fn exists(&self, path: &Path) -> bool;
}

/// Abstraction for getting the current time.
pub trait Clock: Send + Sync {
    /// Get the current UTC timestamp in milliseconds.
    fn now_ms(&self) -> u128;
}

/// Abstraction for sending telemetry events.
pub trait TelemetrySink: Send + Sync {
    /// Record a generic event.
    fn record_event(&self, event_name: &str, properties: HashMap<String, String>);
}
