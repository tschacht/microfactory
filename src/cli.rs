use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Microfactory CLI definition following the architecture spec.
#[derive(Debug, Parser)]
#[command(name = "microfactory")]
#[command(about = "MAKER-inspired workflow runner", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run a microfactory workflow for a given prompt.
    Run(RunArgs),
    /// Inspect the progress of a workflow session.
    Status(StatusArgs),
    /// Resume a paused or failed workflow session.
    Resume(ResumeArgs),
}

#[derive(Debug, Args, Clone)]
pub struct RunArgs {
    #[arg(long, help = "High-level task description")]
    pub prompt: String,

    #[arg(
        long,
        default_value = "config.yaml",
        help = "Path to the domain configuration file"
    )]
    pub config: PathBuf,

    #[arg(long, help = "Domain identifier (e.g., code)")]
    pub domain: String,

    #[arg(long, help = "LLM provider API key (can also come from env vars)")]
    pub api_key: Option<String>,

    #[arg(
        long,
        default_value = "gpt-5.1-codex-mini",
        help = "Default model identifier"
    )]
    pub llm_model: String,

    #[arg(long, default_value_t = 10, help = "Samples per microagent step")]
    pub samples: usize,

    #[arg(long, default_value_t = 3, help = "First-to-ahead-by-k voting margin")]
    pub k: usize,

    #[arg(long, help = "Enable adaptive k adjustment")]
    pub adaptive_k: bool,

    #[arg(long, default_value_t = 4, help = "Maximum concurrent LLM calls")]
    pub max_concurrent_llm: usize,

    #[arg(long, help = "Repository path or domain-specific working directory")]
    pub repo_path: Option<PathBuf>,

    #[arg(long, help = "Dry-run mode that avoids applying changes")]
    pub dry_run: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct StatusArgs {
    #[arg(long, help = "Optional session identifier to inspect")]
    pub session_id: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub struct ResumeArgs {
    #[arg(long, help = "Session identifier to resume")]
    pub session_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_run_command() {
        let cli = Cli::parse_from([
            "microfactory",
            "run",
            "--prompt",
            "fix tests",
            "--domain",
            "code",
            "--repo-path",
            "./repo",
            "--dry-run",
        ]);

        match cli.command {
            Commands::Run(run) => {
                assert_eq!(run.prompt, "fix tests");
                assert_eq!(run.domain, "code");
                assert_eq!(run.repo_path.unwrap(), PathBuf::from("./repo"));
                assert!(run.dry_run);
            }
            _ => panic!("expected run command"),
        }
    }
}
