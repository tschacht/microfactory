use anyhow::Result;
use assert_cmd::Command;
use microfactory::{
    context::Context,
    persistence::{SessionEnvelope, SessionMetadata, SessionStatus, SessionStore},
    status_export::SessionListExport,
};
use tempfile::tempdir;

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
        },
    };

    store
        .save(&envelope, SessionStatus::Running)
        .expect("seed session");
}

#[test]
fn status_json_lists_seeded_sessions() -> Result<()> {
    let temp = tempdir()?;
    let home = temp.path();
    let data_dir = home.join(".microfactory");
    let store = SessionStore::open(Some(data_dir))?;

    seed_session(&store, "session-cli", "Refactor batching", "code");
    seed_session(&store, "session-cli-2", "Draft brief", "analysis");

    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("microfactory")
        .arg("--bin")
        .arg("microfactory")
        .arg("--");
    let assert = cmd
        .env("MICROFACTORY_HOME", home)
        .arg("status")
        .arg("--json")
        .arg("--limit")
        .arg("5")
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone())?;
    let export: SessionListExport = serde_json::from_str(&stdout)?;
    assert_eq!(export.sessions.len(), 2, "two seeded sessions returned");
    let ids: Vec<&str> = export
        .sessions
        .iter()
        .map(|s| s.session_id.as_str())
        .collect();
    assert!(ids.contains(&"session-cli"));
    assert!(ids.contains(&"session-cli-2"));

    Ok(())
}
