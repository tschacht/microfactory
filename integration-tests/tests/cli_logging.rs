#![allow(deprecated)]

use assert_cmd::Command;
use microfactory::{
    context::Context,
    persistence::{SessionEnvelope, SessionMetadata, SessionStatus, SessionStore},
};
use predicates::prelude::*;
use std::{io::Write, path::Path};

#[test]
fn test_default_logging_is_human_readable() {
    let temp = tempfile::TempDir::new().unwrap();
    seed_logging_session(temp.path());
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));

    let assert = cmd
        .env("MICROFACTORY_HOME", temp.path())
        .arg("status")
        .assert();

    assert
        .success()
        .stdout(predicate::str::contains("Recent sessions:"))
        .stdout(predicate::str::contains("\"level\":").not());
}

#[test]
fn test_json_logging_flag_emits_json() {
    let mut run_cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));
    let run_assert = run_cmd
        .arg("--log-json")
        .arg("run")
        .arg("--prompt")
        .arg("test")
        .arg("--domain")
        .arg("code")
        .arg("--config")
        .arg("/nonexistent/config.yaml")
        .assert();

    // We expect failure, but now main() catches the error and logs it via tracing (JSON) to stdout
    run_assert
        .failure()
        .stdout(predicate::str::contains("\"level\":"));
}

#[test]
fn test_verbose_logging_adds_timestamps() {
    let temp = tempfile::TempDir::new().unwrap();
    let config_path = temp.path().join("config.yaml");
    {
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(file, "domains:\n  code:\n    verifier: 'echo ok'\n    applier: 'overwrite_file'\n    agents:\n      decomposition:\n        model: gpt\n        prompt_template: t\n        samples: 1\n      decomposition_discriminator:\n        model: gpt\n        prompt_template: t\n        k: 1\n      solver:\n        model: gpt\n        prompt_template: t\n        samples: 1\n      solution_discriminator:\n        model: gpt\n        prompt_template: t\n        k: 1").unwrap();
    }

    let mut real_run = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));
    let output = real_run
        .env("MICROFACTORY_HOME", temp.path())
        .env("NO_COLOR", "1") // Disable ANSI colors for predictable string matching
        .arg("-v")
        .arg("run")
        .arg("--prompt")
        .arg("test logging")
        .arg("--domain")
        .arg("code")
        .arg("--config")
        .arg(config_path.to_str().unwrap())
        .arg("--llm-model")
        .arg("gpt-invalid-model")
        .arg("--api-key")
        .arg("sk-dummy")
        .assert();

    output
        .failure()
        .stdout(predicate::str::contains("Starting session"))
        .stdout(predicate::str::contains(" INFO "));
}

#[test]
fn test_pretty_logging_is_formatted() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));
    let assert = cmd
        .arg("--log-json")
        .arg("--pretty")
        .arg("run")
        .arg("--prompt")
        .arg("test")
        .arg("--domain")
        .arg("invalid-domain")
        .arg("--config")
        .arg("config.yaml") // Needs to try loading config
        .assert();

    // Expect failure and pretty-printed JSON log of the error
    assert
        .failure()
        .stdout(predicate::str::contains("{\n"))
        .stdout(predicate::str::contains("  \"message\": \"Command failed:"));
}

#[test]
fn test_file_logging_captures_events() {
    let temp = tempfile::TempDir::new().unwrap();
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));

    // We need a session ID to generate a log file.
    // 'run' generates one automatically.
    // We use a dry-run to avoid needing real config/API keys,
    // though we still need a valid config file to start.
    let config_path = temp.path().join("config.yaml");
    std::fs::write(&config_path, "domains:\n  test:\n    agents: {}").unwrap();

    let _assert = cmd
        .env("MICROFACTORY_HOME", temp.path())
        .arg("run")
        .arg("--prompt")
        .arg("test logging")
        .arg("--domain")
        .arg("test")
        .arg("--config")
        .arg(config_path)
        .arg("--dry-run")
        .arg("--llm-model")
        .arg("gpt-4o")
        .arg("--llm-provider")
        .arg("openai")
        .arg("--api-key")
        .arg("sk-dummy")
        .assert();

    // Command fails due to invalid config, but that's fine.
    // We verify the log file captured the error.

    let logs_dir = temp.path().join(".microfactory").join("logs");
    assert!(
        logs_dir.exists(),
        "Logs directory should be created at {logs_dir:?}"
    );

    let mut found_log = false;
    for entry in std::fs::read_dir(logs_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("log") {
            found_log = true;
            let content = std::fs::read_to_string(&path).unwrap();

            // Existence of "level" field proves it's the JSON logger.
            assert!(
                content.contains("\"level\":"),
                "Log file should be in JSON format"
            );

            // Verify we captured the actual application error
            assert!(
                content.contains("Command failed"),
                "Log file should capture the error event"
            );
        }
    }
    assert!(found_log, "Should have found at least one session log file");
}

fn seed_logging_session(home: &Path) {
    let data_dir = home.join(".microfactory");
    let store = SessionStore::open(Some(data_dir)).expect("open session store for logging test");
    let mut ctx = Context::new("Log smoke test", "code");
    ctx.session_id = "logging-test-session".into();
    let envelope = SessionEnvelope {
        context: ctx,
        metadata: SessionMetadata {
            config_path: "config.yaml".into(),
            llm_provider: "openai".into(),
            llm_model: "gpt-4o".into(),
            max_concurrent_llm: 1,
            samples: 1,
            k: 1,
            adaptive_k: false,
            human_low_margin_threshold: 1,
        },
    };
    store
        .save(&envelope, SessionStatus::Running)
        .expect("seed logging session");
}
