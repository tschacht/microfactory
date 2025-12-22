use std::{fs, io::ErrorKind, process::Command};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use walkdir::WalkDir;

mod cargo_deny;

#[derive(Parser)]
#[command(author, version, about = "Workspace maintenance tasks")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run dependency and layering guardrails.
    CheckArchitecture,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::CheckArchitecture => check_architecture(),
    }
}

fn check_architecture() -> Result<()> {
    if has_cargo_deny()? {
        cargo_deny::run_check()?;
    } else {
        eprintln!("cargo deny not found; skipping advisory/ban checks");
    }
    ensure_no_pattern("src/core", "crate::adapters")?;
    Ok(())
}

fn has_cargo_deny() -> Result<bool> {
    match Command::new("cargo").args(["deny", "--version"]).output() {
        Ok(output) if output.status.success() => Ok(true),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no such command: `deny`") {
                Ok(false)
            } else {
                Err(anyhow!("cargo deny --version failed: {}", stderr.trim()))
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow!(e)),
    }
}

fn ensure_no_pattern(dir: &str, needle: &str) -> Result<()> {
    let mut offenders = Vec::new();
    for entry in WalkDir::new(dir) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                offenders.push(format!("{dir} (walk error: {e})"));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let content = fs::read_to_string(entry.path())
            .with_context(|| format!("Failed to read {}", entry.path().display()))?;
        if content.contains(needle) {
            offenders.push(entry.path().display().to_string());
        }
    }

    if offenders.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Forbidden reference to '{needle}' found in: {}",
            offenders.join(", ")
        ))
    }
}

// Intentionally no generic `run()` helper: callers tend to want to capture output for
// diagnostics and/or retry logic.
