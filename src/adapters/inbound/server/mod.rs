//! HTTP server inbound adapter that exposes session data via REST and SSE.

use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as AnyhowContext, Result};
use axum::response::sse::{Event, KeepAlive};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json;
use tokio::net::TcpListener;
use tokio_stream::{StreamExt, wrappers::IntervalStream};
use tracing::info;

use crate::{
    core::ports::{SessionDetail, WorkflowService},
    status_export::{SessionListExport, SessionSummaryExport},
};

/// Configuration options for the server adapter.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub default_limit: usize,
    pub poll_interval: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            default_limit: 25,
            poll_interval: Duration::from_secs(1),
        }
    }
}

/// Server adapter that exposes the `WorkflowService` via HTTP.
pub struct ServerAdapter {
    service: Arc<dyn WorkflowService>,
    options: ServeOptions,
}

impl ServerAdapter {
    pub fn new(service: Arc<dyn WorkflowService>, options: ServeOptions) -> Self {
        Self { service, options }
    }

    /// Run the HTTP server on the given address.
    pub async fn run(self, addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(addr)
            .await
            .context("failed to bind session service listener")?;
        self.run_with_listener(listener).await
    }

    /// Run the HTTP server with an existing listener (useful for tests).
    pub async fn run_with_listener(self, listener: TcpListener) -> Result<()> {
        let state = Arc::new(ServeState::new(self.service, self.options));
        let router = build_router(state);
        if let Ok(addr) = listener.local_addr() {
            info!(%addr, "microfactory serve listening");
        } else {
            info!("microfactory serve listening");
        }
        axum::serve(listener, router.into_make_service())
            .await
            .context("serve endpoint failed")
    }
}

#[derive(Clone)]
struct ServeState {
    service: Arc<dyn WorkflowService>,
    default_limit: usize,
    poll_interval: Duration,
}

impl ServeState {
    fn new(service: Arc<dyn WorkflowService>, options: ServeOptions) -> Self {
        Self {
            service,
            default_limit: options.default_limit.max(1),
            poll_interval: options.poll_interval.max(Duration::from_millis(200)),
        }
    }

    fn limit_or_default(&self, value: Option<usize>) -> usize {
        value.filter(|v| *v > 0).unwrap_or(self.default_limit)
    }

    async fn list_sessions(&self, limit: usize) -> Result<SessionListExport> {
        let summaries = self
            .service
            .list_sessions(limit)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        // Convert to export format for backward compatibility
        let export_summaries: Vec<SessionSummaryExport> = summaries
            .into_iter()
            .map(|s| SessionSummaryExport {
                session_id: s.session_id,
                status: s.status,
                prompt: s.prompt,
                domain: s.domain,
                updated_at: s.updated_at.parse().unwrap_or(0),
            })
            .collect();

        Ok(SessionListExport {
            sessions: export_summaries,
        })
    }

    async fn load_session(&self, session_id: &str) -> Result<Option<SessionDetail>> {
        self.service
            .get_session(session_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn resume_session(&self, session_id: &str) -> Result<bool> {
        // Note: Resume spawns a background process, so we use the CLI approach
        // This is a special case where we spawn a new process rather than using the service directly
        let exe = std::env::current_exe().context("Failed to determine current executable path")?;

        info!(session_id, "Spawning background resume process");
        std::process::Command::new(exe)
            .arg("resume")
            .arg("--session-id")
            .arg(session_id)
            .spawn()
            .context("Failed to spawn resume process")?;

        Ok(true)
    }
}

fn build_router(state: Arc<ServeState>) -> Router {
    Router::new()
        .route("/sessions", get(list_sessions_handler))
        .route("/sessions/{id}", get(session_detail_handler))
        .route("/sessions/{id}/resume", post(resume_session_handler))
        .route("/sessions/stream", get(stream_sessions_handler))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<usize>,
}

async fn list_sessions_handler(
    State(state): State<Arc<ServeState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<SessionListExport>, StatusCode> {
    let limit = state.limit_or_default(query.limit);
    state
        .list_sessions(limit)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn session_detail_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<ServeState>>,
) -> Result<Json<SessionDetail>, StatusCode> {
    match state
        .load_session(&session_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(detail) => Ok(Json(detail)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn resume_session_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<ServeState>>,
) -> Result<StatusCode, (StatusCode, String)> {
    // First check if session exists and is in a resumable state
    match state.load_session(&session_id).await {
        Ok(Some(detail)) => {
            if detail.status != "paused" && detail.status != "failed" {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Session {session_id} is not paused or failed (status: {})",
                        detail.status
                    ),
                ));
            }
        }
        Ok(None) => return Err((StatusCode::NOT_FOUND, "Session not found".into())),
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }

    match state.resume_session(&session_id) {
        Ok(true) => Ok(StatusCode::ACCEPTED),
        Ok(false) => Err((StatusCode::NOT_FOUND, "Session not found".into())),
        Err(err) => Err((StatusCode::BAD_REQUEST, err.to_string())),
    }
}

async fn stream_sessions_handler(State(state): State<Arc<ServeState>>) -> impl IntoResponse {
    let poll = state.poll_interval;
    let mut interval = tokio::time::interval(poll);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stream_state = state.clone();
    let stream = IntervalStream::new(interval).then(move |_| {
        let state = stream_state.clone();
        async move {
            let start = Instant::now();
            let payload = state
                .list_sessions(state.default_limit)
                .await
                .map_err(|err| {
                    tracing::error!(error = %err, "serve stream failed to list sessions");
                })
                .ok();
            let event = if let Some(export) = payload {
                match serde_json::to_string(&export) {
                    Ok(json) => Event::default().data(json),
                    Err(err) => {
                        tracing::error!(error = %err, "failed to serialize session export");
                        Event::default().comment("serialization_error")
                    }
                }
            } else {
                Event::default().comment("snapshot_error")
            };
            tracing::trace!(
                elapsed_ms = start.elapsed().as_millis(),
                "serve stream event ready"
            );
            Result::<Event, Infallible>::Ok(event)
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(poll).text("keep-alive"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ports::{
        DryRunResult, ResumeSessionRequest, RunSessionRequest, SessionMetadataInfo, SessionOutcome,
        SessionSummary, SubprocessOutcome, SubprocessRequest,
    };
    use async_trait::async_trait;
    use axum::body::Body;
    use tower::ServiceExt;

    struct MockWorkflowService {
        sessions: Vec<SessionSummary>,
        details: std::collections::HashMap<String, SessionDetail>,
    }

    impl MockWorkflowService {
        fn new() -> Self {
            Self {
                sessions: vec![],
                details: std::collections::HashMap::new(),
            }
        }

        fn with_session(mut self, id: &str, status: &str) -> Self {
            let summary = SessionSummary {
                session_id: id.to_string(),
                domain: "code".to_string(),
                prompt: "test prompt".to_string(),
                status: status.to_string(),
                updated_at: "12345".to_string(),
            };
            self.sessions.push(summary);

            let detail = SessionDetail {
                session_id: id.to_string(),
                domain: "code".to_string(),
                prompt: "test prompt".to_string(),
                status: status.to_string(),
                updated_at: "12345".to_string(),
                steps_completed: 0,
                wait_state: None,
                metadata: SessionMetadataInfo {
                    config_path: "config.yaml".to_string(),
                    llm_provider: "openai".to_string(),
                    llm_model: "gpt".to_string(),
                    samples: 2,
                    k: 2,
                },
            };
            self.details.insert(id.to_string(), detail);
            self
        }
    }

    #[async_trait]
    impl WorkflowService for MockWorkflowService {
        async fn run_session(
            &self,
            _request: RunSessionRequest,
        ) -> crate::core::Result<SessionOutcome> {
            unimplemented!()
        }

        async fn resume_session(
            &self,
            _request: ResumeSessionRequest,
        ) -> crate::core::Result<SessionOutcome> {
            unimplemented!()
        }

        async fn run_subprocess(
            &self,
            _request: SubprocessRequest,
        ) -> crate::core::Result<SubprocessOutcome> {
            unimplemented!()
        }

        async fn get_session(
            &self,
            session_id: &str,
        ) -> crate::core::Result<Option<SessionDetail>> {
            Ok(self.details.get(session_id).cloned())
        }

        async fn list_sessions(&self, limit: usize) -> crate::core::Result<Vec<SessionSummary>> {
            Ok(self.sessions.iter().take(limit).cloned().collect())
        }

        async fn dry_run_probe(
            &self,
            _request: &RunSessionRequest,
        ) -> crate::core::Result<DryRunResult> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn list_endpoint_returns_sessions() {
        let service = Arc::new(MockWorkflowService::new().with_session("session-a", "running"));
        let state = Arc::new(ServeState::new(service, ServeOptions::default()));
        let app = build_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn detail_endpoint_returns_not_found_for_unknown() {
        let service = Arc::new(MockWorkflowService::new());
        let state = Arc::new(ServeState::new(service, ServeOptions::default()));
        let app = build_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/sessions/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn resume_endpoint_rejects_running_session() {
        let service =
            Arc::new(MockWorkflowService::new().with_session("running-session", "running"));
        let state = Arc::new(ServeState::new(service, ServeOptions::default()));
        let app = build_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions/running-session/resume")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
