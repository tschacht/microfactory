//! Inbound ports (use-case ports) define the application service interface that
//! driving adapters (CLI, HTTP server) consume. These traits represent the
//! domain-facing API for session management and workflow execution.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::core::error::Result;

/// Request to start a new workflow session.
#[derive(Debug, Clone)]
pub struct RunSessionRequest {
    pub prompt: String,
    pub domain: String,
    pub config_path: PathBuf,
    pub llm_provider: String,
    pub llm_model: String,
    pub api_key: Option<String>,
    pub samples: usize,
    pub k: usize,
    pub adaptive_k: bool,
    pub max_concurrent_llm: usize,
    pub dry_run: bool,
    pub step_by_step: bool,
    pub human_low_margin_threshold: usize,
    pub output_dir: Option<PathBuf>,
}

/// Request to resume an existing session.
#[derive(Debug, Clone)]
pub struct ResumeSessionRequest {
    pub session_id: String,
    pub config_path: Option<PathBuf>,
    pub llm_provider: Option<String>,
    pub llm_model: Option<String>,
    pub api_key: Option<String>,
    pub samples: Option<usize>,
    pub k: Option<usize>,
    pub max_concurrent_llm: Option<usize>,
    pub human_low_margin_threshold: Option<usize>,
}

/// Request to run a subprocess (single-step execution).
#[derive(Debug, Clone)]
pub struct SubprocessRequest {
    pub domain: String,
    pub config_path: PathBuf,
    pub step: String,
    pub context_json: Option<String>,
    pub llm_provider: String,
    pub llm_model: String,
    pub api_key: Option<String>,
    pub samples: usize,
    pub k: usize,
    pub max_concurrent_llm: usize,
}

/// Response from session execution (run or resume).
#[derive(Debug, Clone, Serialize)]
pub struct SessionOutcome {
    pub session_id: String,
    pub completed: bool,
    pub paused: bool,
    pub pause_reason: Option<PauseInfo>,
}

/// Information about why a session paused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PauseInfo {
    pub step_id: usize,
    pub trigger: String,
    pub details: String,
}

/// Response from subprocess execution.
#[derive(Debug, Clone, Serialize)]
pub struct SubprocessOutcome {
    pub session_id: String,
    pub step_id: usize,
    pub candidate_solutions: Vec<String>,
    pub winning_solution: Option<String>,
    pub metrics: Option<SubprocessMetrics>,
}

/// Metrics from subprocess execution.
#[derive(Debug, Clone, Serialize)]
pub struct SubprocessMetrics {
    pub samples_requested: usize,
    pub samples_accepted: usize,
    pub vote_margin: Option<usize>,
}

/// Summary of a session for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub domain: String,
    pub prompt: String,
    pub status: String,
    pub updated_at: String,
}

/// Detailed session information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub session_id: String,
    pub domain: String,
    pub prompt: String,
    pub status: String,
    pub updated_at: String,
    pub steps_completed: usize,
    pub wait_state: Option<PauseInfo>,
    pub metadata: SessionMetadataInfo,
}

/// Session metadata for detail view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadataInfo {
    pub config_path: String,
    pub llm_provider: String,
    pub llm_model: String,
    pub samples: usize,
    pub k: usize,
}

/// Dry-run probe response.
#[derive(Debug, Clone)]
pub struct DryRunResult {
    pub model: String,
    pub response: String,
}

/// The primary application service trait that driving adapters consume.
///
/// This trait defines the use-case port for workflow operations. CLI and HTTP
/// server adapters receive an implementation of this trait and call its methods
/// to execute business logic.
#[async_trait]
pub trait WorkflowService: Send + Sync {
    /// Start a new workflow session.
    async fn run_session(&self, request: RunSessionRequest) -> Result<SessionOutcome>;

    /// Resume a paused or failed session.
    async fn resume_session(&self, request: ResumeSessionRequest) -> Result<SessionOutcome>;

    /// Run a single-step subprocess and return results.
    async fn run_subprocess(&self, request: SubprocessRequest) -> Result<SubprocessOutcome>;

    /// Get detailed information about a specific session.
    async fn get_session(&self, session_id: &str) -> Result<Option<SessionDetail>>;

    /// List recent sessions.
    async fn list_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>>;

    /// Run a dry-run probe to test LLM connectivity.
    async fn dry_run_probe(&self, request: &RunSessionRequest) -> Result<DryRunResult>;
}
