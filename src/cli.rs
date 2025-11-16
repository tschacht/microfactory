use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    /// Execute a single-step subprocess workflow and emit JSON.
    Subprocess(SubprocessArgs),
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

    #[arg(
        long,
        default_value_t = LlmProvider::Openai,
        value_enum,
        help = "LLM provider backend (openai, anthropic, gemini, grok)"
    )]
    pub llm_provider: LlmProvider,

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

    #[arg(long, help = "Override config file path (defaults to stored path)")]
    pub config: Option<PathBuf>,

    #[arg(long, help = "Override LLM provider API key")]
    pub api_key: Option<String>,

    #[arg(
        long,
        value_enum,
        help = "Override LLM provider backend (defaults to stored provider)"
    )]
    pub llm_provider: Option<LlmProvider>,

    #[arg(long, help = "Override model identifier (defaults to stored model)")]
    pub llm_model: Option<String>,

    #[arg(long, help = "Override max concurrent LLM calls")]
    pub max_concurrent_llm: Option<usize>,

    #[arg(long, help = "Override sample count")]
    pub samples: Option<usize>,

    #[arg(long, help = "Override voting k")]
    pub k: Option<usize>,
}

#[derive(Debug, Args, Clone)]
pub struct SubprocessArgs {
    #[arg(long, help = "Domain identifier (e.g., code)")]
    pub domain: String,

    #[arg(
        long,
        default_value = "config.yaml",
        help = "Path to the domain configuration file"
    )]
    pub config: PathBuf,

    #[arg(long, help = "Step description to execute in isolation")]
    pub step: String,

    #[arg(
        long,
        help = "Optional inline JSON blob to pass through domain context"
    )]
    pub context_json: Option<String>,

    #[arg(long, help = "LLM provider API key (can also come from env vars)")]
    pub api_key: Option<String>,

    #[arg(
        long,
        default_value = "gpt-5.1-codex-mini",
        help = "Model identifier for the subprocess run"
    )]
    pub llm_model: String,

    #[arg(
        long,
        default_value_t = LlmProvider::Openai,
        value_enum,
        help = "LLM provider backend"
    )]
    pub llm_provider: LlmProvider,

    #[arg(long, default_value_t = 2, help = "Samples per solver call")]
    pub samples: usize,

    #[arg(
        long,
        default_value_t = 2,
        help = "First-to-ahead-by-k margin for subprocess voting"
    )]
    pub k: usize,

    #[arg(long, default_value_t = 2, help = "Max concurrent LLM calls")]
    pub max_concurrent_llm: usize,
}

/// Supported LLM providers surfaced via the CLI.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
#[clap(rename_all = "lower")]
pub enum LlmProvider {
    Openai,
    Anthropic,
    Gemini,
    Grok,
}

impl LlmProvider {
    pub fn provider_id(self) -> &'static str {
        match self {
            LlmProvider::Openai => rig::client::builder::DefaultProviders::OPENAI,
            LlmProvider::Anthropic => rig::client::builder::DefaultProviders::ANTHROPIC,
            LlmProvider::Gemini => rig::client::builder::DefaultProviders::GEMINI,
            LlmProvider::Grok => rig::client::builder::DefaultProviders::XAI,
        }
    }

    pub fn env_var(self) -> &'static str {
        match self {
            LlmProvider::Openai => "OPENAI_API_KEY",
            LlmProvider::Anthropic => "ANTHROPIC_API_KEY",
            LlmProvider::Gemini => "GEMINI_API_KEY",
            LlmProvider::Grok => "XAI_API_KEY",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LlmProvider::Openai => "openai",
            LlmProvider::Anthropic => "anthropic",
            LlmProvider::Gemini => "gemini",
            LlmProvider::Grok => "grok",
        }
    }

    pub fn from_name(value: &str) -> Option<Self> {
        match value {
            "openai" => Some(LlmProvider::Openai),
            "anthropic" => Some(LlmProvider::Anthropic),
            "gemini" => Some(LlmProvider::Gemini),
            "grok" => Some(LlmProvider::Grok),
            _ => None,
        }
    }
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
            "--llm-provider",
            "openai",
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
