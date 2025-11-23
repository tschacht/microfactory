use anyhow::Result;
use futures_util::StreamExt;
use microfactory::{
    adapters::persistence::{SessionEnvelope, SessionMetadata, SessionStatus, SessionStore},
    context::Context,
    server::{ServeOptions, run_with_listener},
    status_export::{SessionDetailExport, SessionListExport},
};
use reqwest::Client;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_routes_return_session_json() -> Result<()> {
    let temp = tempdir()?;
    let data_dir = temp.path().join(".microfactory");
    let store = SessionStore::open(Some(data_dir))?;
    seed_session(&store, "serve-session", "Summarize findings", "analysis");

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let options = ServeOptions {
        default_limit: 5,
        poll_interval: Duration::from_millis(200),
    };

    let server_store = store.clone();
    let handle = tokio::spawn(async move {
        if let Err(err) = run_with_listener(listener, server_store, options).await {
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

    let detail: SessionDetailExport = client
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

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let options = ServeOptions {
        default_limit: 5,
        poll_interval: Duration::from_millis(100),
    };

    let server_store = store.clone();
    let handle = tokio::spawn(async move {
        if let Err(err) = run_with_listener(listener, server_store, options).await {
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
