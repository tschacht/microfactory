use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, Context, Result};
use walkdir::WalkDir;

pub fn run_check() -> Result<()> {
    let repo_root = std::env::current_dir().context("Failed to resolve current directory")?;
    let target_dir = repo_root.join("target");
    fs::create_dir_all(&target_dir).context("Failed to create target directory")?;

    let workspace_db_root = target_dir.join("cargo-deny-advisory-dbs");
    ensure_workspace_advisory_db(&workspace_db_root)?;

    let deny_config_path = target_dir.join("deny.xtask.toml");
    let deny_toml =
        fs::read_to_string(repo_root.join("deny.toml")).context("Failed to read deny.toml")?;
    let deny_toml = upsert_advisory_db_path(&deny_toml, &workspace_db_root);
    fs::write(&deny_config_path, deny_toml).context("Failed to write deny config override")?;

    let status = Command::new("cargo")
        .args([
            "deny",
            "check",
            "--disable-fetch",
            "--config",
            deny_config_path
                .to_str()
                .ok_or_else(|| anyhow!("deny config path is not valid UTF-8"))?,
        ])
        .status()
        .context("Failed to run cargo deny check")?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("cargo deny check exited with status {status}"))
    }
}

fn ensure_workspace_advisory_db(workspace_db_root: &Path) -> Result<()> {
    if workspace_db_root.exists() {
        return Ok(());
    }
    fs::create_dir_all(workspace_db_root).context("Failed to create workspace advisory db dir")?;

    let source_db_root = default_cargo_home()?.join("advisory-dbs");
    if !source_db_root.exists() {
        return Err(anyhow!(
            "No RustSec advisory DB found at {}. Run `cargo deny check` once outside the sandbox (or allow network) to fetch it.",
            source_db_root.display()
        ));
    }

    copy_dir_all(&source_db_root, workspace_db_root).with_context(|| {
        format!(
            "Failed to copy advisory DB from {} to {}",
            source_db_root.display(),
            workspace_db_root.display()
        )
    })?;

    Ok(())
}

fn default_cargo_home() -> Result<PathBuf> {
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        return Ok(PathBuf::from(cargo_home));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".cargo"))
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    for entry in WalkDir::new(src) {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .context("Failed to compute relative path")?;
        let out_path = dst.join(rel);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }

        if entry.file_type().is_file() {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &out_path)?;
        }
    }

    Ok(())
}

fn upsert_advisory_db_path(deny_toml: &str, db_path: &Path) -> String {
    let db_path = db_path.to_string_lossy().replace('\\', "\\\\");
    let replacement = format!("db-path = \"{db_path}\"");

    let mut lines: Vec<String> = deny_toml.lines().map(|l| l.to_string()).collect();
    let mut in_advisories = false;
    let mut advisories_header_index: Option<usize> = None;
    let mut replaced = false;

    for (idx, line) in lines.iter_mut().enumerate() {
        let trimmed = line.trim();
        if trimmed == "[advisories]" {
            in_advisories = true;
            advisories_header_index = Some(idx);
            continue;
        }

        if in_advisories && trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_advisories = false;
        }

        if in_advisories && (trimmed.starts_with("db-path") || trimmed.starts_with("#db-path")) {
            *line = replacement.clone();
            replaced = true;
        }
    }

    if !replaced {
        if let Some(idx) = advisories_header_index {
            lines.insert(idx + 1, replacement);
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    out
}
