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
    adapters::persistence::{SessionRecord, SessionStatus, SessionStore},
    status_export::{SessionDetailExport, SessionListExport},
};

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

pub async fn run(addr: SocketAddr, store: SessionStore, options: ServeOptions) -> Result<()> {
    let listener = TcpListener::bind(addr)
        .await
        .context("failed to bind session service listener")?;
    run_with_listener(listener, store, options).await
}

pub async fn run_with_listener(
    listener: TcpListener,
    store: SessionStore,
    options: ServeOptions,
) -> Result<()> {
    let state = Arc::new(ServeState::new(store, options));
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

#[derive(Clone)]
struct ServeState {
    store: SessionStore,
    default_limit: usize,
    poll_interval: Duration,
}

impl ServeState {
    fn new(store: SessionStore, options: ServeOptions) -> Self {
        Self {
            store,
            default_limit: options.default_limit.max(1),
            poll_interval: options.poll_interval.max(Duration::from_millis(200)),
        }
    }

    fn limit_or_default(&self, value: Option<usize>) -> usize {
        value.filter(|v| *v > 0).unwrap_or(self.default_limit)
    }

    fn list_sessions(&self, limit: usize) -> Result<SessionListExport> {
        let summaries = self
            .store
            .list(limit)
            .context("failed to list sessions from store")?;
        Ok(SessionListExport::from_summaries(summaries))
    }

    fn load_session(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        match self.store.load(session_id) {
            Ok(record) => Ok(Some(record)),
            Err(err) if err.to_string().contains("not found") => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn resume_session(&self, session_id: &str) -> Result<bool> {
        let record = match self.store.load(session_id) {
            Ok(r) => r,
            Err(err) if err.to_string().contains("not found") => return Ok(false),
            Err(err) => return Err(err),
        };

        if !matches!(record.status, SessionStatus::Paused | SessionStatus::Failed) {
            return Err(anyhow::anyhow!(
                "Session {session_id} is not paused or failed (status: {:?})",
                record.status
            ));
        }

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
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn session_detail_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<ServeState>>,
) -> Result<Json<SessionDetailExport>, StatusCode> {
    match state
        .load_session(&session_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        Some(record) => Ok(Json(SessionDetailExport::from_record(&record))),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn resume_session_handler(
    Path(session_id): Path<String>,
    State(state): State<Arc<ServeState>>,
) -> Result<StatusCode, (StatusCode, String)> {
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
    use crate::{
        adapters::persistence::{SessionEnvelope, SessionMetadata, SessionStatus},
        core::domain::Context,
    };
    use axum::body::Body;
    use tempfile::tempdir;
    use tower::ServiceExt;

    #[tokio::test]
    async fn list_endpoint_returns_sessions() {
        let temp = tempdir().unwrap();
        let store = SessionStore::open(Some(temp.path().to_path_buf())).unwrap();
        seed_session(&store, "session-a", SessionStatus::Running);
        let state = Arc::new(ServeState::new(store, ServeOptions::default()));
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
        let temp = tempdir().unwrap();
        let store = SessionStore::open(Some(temp.path().to_path_buf())).unwrap();
        let state = Arc::new(ServeState::new(store, ServeOptions::default()));
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
    async fn resume_endpoint_accepts_paused_session() {
        let temp = tempdir().unwrap();
        let store = SessionStore::open(Some(temp.path().to_path_buf())).unwrap();
        seed_session(&store, "paused-session", SessionStatus::Paused);
        let state = Arc::new(ServeState::new(store, ServeOptions::default()));
        let app = build_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/sessions/paused-session/resume")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn resume_endpoint_rejects_running_session() {
        let temp = tempdir().unwrap();
        let store = SessionStore::open(Some(temp.path().to_path_buf())).unwrap();
        seed_session(&store, "running-session", SessionStatus::Running);
        let state = Arc::new(ServeState::new(store, ServeOptions::default()));
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

    fn seed_session(store: &SessionStore, session_id: &str, status: SessionStatus) {
        let mut ctx = Context::new("demo", "code");
        ctx.session_id = session_id.into();
        let envelope = SessionEnvelope {
            context: ctx,
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
        store.save(&envelope, status).unwrap();
    }
}
