#![allow(deprecated)]

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_inspect_conflicts_with_log_json() {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));
    let assert = cmd
        .arg("run")
        .arg("--prompt")
        .arg("foo")
        .arg("--domain")
        .arg("code")
        .arg("--inspect")
        .arg("ops")
        .arg("--log-json")
        .assert();

    assert.failure().stderr(predicate::str::contains(
        "--inspect cannot be used with --log-json",
    ));
}

#[test]
fn test_inspect_suppresses_standard_logs() {
    let temp = tempfile::TempDir::new().unwrap();
    let config_path = temp.path().join("config.yaml");
    // valid yaml but empty agent defs
    std::fs::write(&config_path, "domains:\n  test:\n    agents: {}").unwrap();

    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin("microfactory"));
    let assert = cmd
        .env("MICROFACTORY_HOME", temp.path())
        .arg("run")
        .arg("--prompt")
        .arg("test inspect")
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
        .arg("--inspect")
        .arg("ops")
        .assert();

    // The command will likely fail due to network/api key,
    // BUT we are asserting that the "Starting session" INFO log is suppressed.
    assert.stdout(predicate::str::contains("Starting session").not());
}
