use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Microfactory CLI definition following the architecture spec.
#[derive(Debug, Parser)]
#[command(name = "microfactory")]
#[command(about = "MAKER-inspired workflow runner", version)]
#[command(disable_help_subcommand = true)]
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
    /// Serve session data over HTTP (REST + SSE).
    Serve(ServeArgs),
    /// Provide structured help so operators or agents can self-orient.
    Help(HelpArgs),
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

    #[arg(long, help = "Skips persistence and runs a single probe for testing.")]
    pub dry_run: bool,

    #[arg(
        long,
        help = "Pauses execution after decomposition and after each step completion."
    )]
    pub step_by_step: bool,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    #[arg(long, help = "Optional session identifier to inspect")]
    pub session_id: Option<String>,

    #[arg(
        long,
        default_value_t = 10,
        help = "Maximum number of sessions to list when no ID is provided"
    )]
    pub limit: usize,

    #[arg(long, help = "Emit JSON instead of human-readable output")]
    pub json: bool,
}

impl Default for StatusArgs {
    fn default() -> Self {
        Self {
            session_id: None,
            limit: 10,
            json: false,
        }
    }
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

#[derive(Debug, Args, Clone)]
pub struct ServeArgs {
    #[arg(
        long,
        default_value = "127.0.0.1",
        help = "Bind interface for the HTTP server"
    )]
    pub bind: String,

    #[arg(long, default_value_t = 8080, help = "Port for the HTTP server")]
    pub port: u16,

    #[arg(
        long,
        default_value_t = 25,
        help = "Default session list limit when not specified by clients"
    )]
    pub limit: usize,

    #[arg(
        long,
        default_value_t = 1000,
        help = "Polling interval for SSE stream in milliseconds"
    )]
    pub poll_interval_ms: u64,
}

#[derive(Debug, Args, Clone)]
pub struct HelpArgs {
    #[arg(
        value_enum,
        long,
        short = 't',
        help = "Specific topic to explain (defaults to overview)"
    )]
    pub topic: Option<HelpTopic>,

    #[arg(
        long,
        value_enum,
        default_value_t = HelpFormat::Text,
        help = "Output format: text or json"
    )]
    pub format: HelpFormat,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum HelpTopic {
    Overview,
    Run,
    Status,
    Resume,
    Subprocess,
    Serve,
}

#[derive(Debug, Copy, Clone, ValueEnum)]
pub enum HelpFormat {
    Text,
    Json,
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

    #[test]
    fn parses_status_with_json_limit() {
        let cli = Cli::parse_from(["microfactory", "status", "--json", "--limit", "5"]);

        match cli.command {
            Commands::Status(status) => {
                assert!(status.json);
                assert_eq!(status.limit, 5);
            }
            _ => panic!("Expected status command"),
        }
    }

    #[test]
    fn parses_serve_command() {
        let cli = Cli::parse_from([
            "microfactory",
            "serve",
            "--bind",
            "0.0.0.0",
            "--port",
            "9090",
            "--limit",
            "5",
            "--poll-interval-ms",
            "500",
        ]);

        match cli.command {
            Commands::Serve(args) => {
                assert_eq!(args.bind, "0.0.0.0");
                assert_eq!(args.port, 9090);
                assert_eq!(args.limit, 5);
                assert_eq!(args.poll_interval_ms, 500);
            }
            _ => panic!("expected serve command"),
        }
    }

    #[test]
    fn parses_help_with_topic_and_format() {
        let cli = Cli::parse_from(["microfactory", "help", "--topic", "run", "--format", "json"]);

        match cli.command {
            Commands::Help(args) => {
                assert!(matches!(args.topic, Some(HelpTopic::Run)));
                assert!(matches!(args.format, HelpFormat::Json));
            }
            _ => panic!("expected help command"),
        }
    }
}
