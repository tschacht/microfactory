use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use microfactory::{
    adapters::{
        inbound::server::{ServeOptions, ServerAdapter},
        outbound::persistence::{SessionEnvelope, SessionMetadata, SessionStatus, SessionStore},
    },
    core::{
        domain::Context,
        ports::{
            DryRunResult, PauseInfo, ResumeSessionRequest, RunSessionRequest, SessionDetail,
            SessionMetadataInfo, SessionOutcome, SessionSummary, SubprocessOutcome,
            SubprocessRequest, WorkflowService,
        },
    },
    status_export::SessionListExport,
};
use reqwest::Client;
use std::{io::ErrorKind, sync::Arc};
use tempfile::tempdir;
use tokio::{
    net::TcpListener,
    time::{Duration, sleep, timeout},
};

fn seed_session(store: &SessionStore, session_id: &str, prompt: &str, domain: &str) {
    let mut ctx = Context::new(prompt, domain);
    ctx.session_id = session_id.to_string();
    let envelope = SessionEnvelope {
        context: ctx,
        metadata: SessionMetadata {
            config_path: "config.yaml".into(),
            llm_provider: "openai".into(),
            llm_model: "gpt-5".into(),
            max_concurrent_llm: 2,
            samples: 2,
            k: 2,
            adaptive_k: false,
            human_low_margin_threshold: 1,
        },
    };
    store
        .save(&envelope, SessionStatus::Completed)
        .expect("seed session");
}

/// A mock workflow service that delegates to the store for session queries.
struct MockWorkflowService {
    store: SessionStore,
}

impl MockWorkflowService {
    fn new(store: SessionStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl WorkflowService for MockWorkflowService {
    async fn run_session(
        &self,
        _request: RunSessionRequest,
    ) -> microfactory::core::Result<SessionOutcome> {
        unimplemented!("not needed for serve tests")
    }

    async fn resume_session(
        &self,
        _request: ResumeSessionRequest,
    ) -> microfactory::core::Result<SessionOutcome> {
        unimplemented!("not needed for serve tests")
    }

    async fn run_subprocess(
        &self,
        _request: SubprocessRequest,
    ) -> microfactory::core::Result<SubprocessOutcome> {
        unimplemented!("not needed for serve tests")
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> microfactory::core::Result<Option<SessionDetail>> {
        match self.store.load(session_id) {
            Ok(record) => {
                let context = &record.envelope.context;
                let wait_state = context.wait_state.as_ref().map(|w| PauseInfo {
                    step_id: w.step_id,
                    trigger: w.trigger.clone(),
                    details: w.details.clone(),
                });

                Ok(Some(SessionDetail {
                    session_id: context.session_id.clone(),
                    domain: context.domain.clone(),
                    prompt: context.prompt.clone(),
                    status: record.status.as_str().to_string(),
                    updated_at: record.updated_at.to_string(),
                    steps_completed: 0,
                    wait_state,
                    metadata: SessionMetadataInfo {
                        config_path: record.envelope.metadata.config_path.clone(),
                        llm_provider: record.envelope.metadata.llm_provider.clone(),
                        llm_model: record.envelope.metadata.llm_model.clone(),
                        samples: record.envelope.metadata.samples,
                        k: record.envelope.metadata.k,
                    },
                }))
            }
            Err(e) if e.to_string().contains("not found") => Ok(None),
            Err(e) => Err(microfactory::core::error::Error::Persistence(e.to_string())),
        }
    }

    async fn list_sessions(&self, limit: usize) -> microfactory::core::Result<Vec<SessionSummary>> {
        let summaries = self
            .store
            .list(limit)
            .map_err(|e| microfactory::core::error::Error::Persistence(e.to_string()))?;

        Ok(summaries
            .into_iter()
            .map(|s| SessionSummary {
                session_id: s.session_id,
                domain: s.domain,
                prompt: s.prompt,
                status: s.status.as_str().to_string(),
                updated_at: s.updated_at.to_string(),
            })
            .collect())
    }

    async fn dry_run_probe(
        &self,
        _request: &RunSessionRequest,
    ) -> microfactory::core::Result<DryRunResult> {
        unimplemented!("not needed for serve tests")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_routes_return_session_json() -> Result<()> {
    let temp = tempdir()?;
    let data_dir = temp.path().join(".microfactory");
    let store = SessionStore::open(Some(data_dir))?;
    seed_session(&store, "serve-session", "Summarize findings", "analysis");

    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipping serve_routes_return_session_json: {e}");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let addr = listener.local_addr()?;
    let options = ServeOptions {
        default_limit: 5,
        poll_interval: Duration::from_millis(200),
    };

    let service: Arc<dyn WorkflowService> = Arc::new(MockWorkflowService::new(store));
    let adapter = ServerAdapter::new(service, options);

    let handle = tokio::spawn(async move {
        if let Err(err) = adapter.run_with_listener(listener).await {
            eprintln!("serve task exited: {err:?}");
        }
    });

    sleep(Duration::from_millis(250)).await;

    let client = Client::builder().build()?;
    let base = format!("http://{}:{}", addr.ip(), addr.port());

    let list: SessionListExport = client
        .get(format!("{base}/sessions?limit=1"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(list.sessions.len(), 1, "one seeded session available");
    let session_id = list.sessions[0].session_id.clone();

    let detail: SessionDetail = client
        .get(format!("{base}/sessions/{session_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(detail.session_id, session_id);
    assert_eq!(detail.domain, "analysis");

    handle.abort();
    let _ = handle.await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_sse_stream_emits_snapshots() -> Result<()> {
    let temp = tempdir()?;
    let data_dir = temp.path().join(".microfactory");
    let store = SessionStore::open(Some(data_dir))?;
    seed_session(&store, "serve-sse", "Outline approach", "code");

    let listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            eprintln!("skipping serve_sse_stream_emits_snapshots: {e}");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
    let addr = listener.local_addr()?;
    let options = ServeOptions {
        default_limit: 5,
        poll_interval: Duration::from_millis(100),
    };

    let service: Arc<dyn WorkflowService> = Arc::new(MockWorkflowService::new(store));
    let adapter = ServerAdapter::new(service, options);

    let handle = tokio::spawn(async move {
        if let Err(err) = adapter.run_with_listener(listener).await {
            eprintln!("serve task exited: {err:?}");
        }
    });

    sleep(Duration::from_millis(150)).await;
    let client = Client::builder().build()?;
    let base = format!("http://{}:{}", addr.ip(), addr.port());
    let response = client
        .get(format!("{base}/sessions/stream"))
        .send()
        .await?
        .error_for_status()?;

    let mut stream = response.bytes_stream();
    let first_chunk = timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("sse stream timed out")
        .expect("sse chunk")?;
    let payload = String::from_utf8(first_chunk.to_vec())?;
    assert!(payload.contains("data:"), "chunk contains SSE data field");
    assert!(
        payload.contains("sessions"),
        "export JSON present in SSE chunk"
    );

    handle.abort();
    let _ = handle.await;
    Ok(())
}
