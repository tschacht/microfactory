//! Microfactory CLI entry point and composition root.
//!
//! This file wires together the application by:
//! 1. Parsing CLI arguments
//! 2. Initializing tracing/logging
//! 3. Constructing outbound adapters (LLM client, persistence, etc.)
//! 4. Constructing the application service
//! 5. Passing the service to inbound adapters (CLI, HTTP server)

use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Result, anyhow};
use clap::Parser;

use microfactory::{
    adapters::{
        inbound::{Cli, CliAdapter, Commands, LlmProvider, ServeArgs, ServeOptions, ServerAdapter},
        llm::RigLlmClient,
        outbound::{
            clock::SystemClock, filesystem::StdFileSystem, persistence::SessionStore,
            telemetry::TracingTelemetrySink,
        },
        templating::HandlebarsRenderer,
    },
    application::service::{ApiKeyResolver, AppService, LlmClientFactory},
    core::ports::{Clock, FileSystem, LlmClient, TelemetrySink, WorkflowService},
    paths, tracing_setup,
};

static HOME_ENV_ONCE: OnceLock<()> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Validate mutually exclusive flags
    if cli.inspect.is_some() && (cli.log_json || cli.pretty || cli.compact) {
        use clap::CommandFactory;
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                "--inspect cannot be used with --log-json, --pretty, or --compact",
            )
            .exit();
    }

    // Determine JSON log format
    let mut json_format = if cli.compact {
        tracing_setup::JsonLogFormat::Compact
    } else {
        tracing_setup::JsonLogFormat::Pretty
    };
    if cli.pretty {
        json_format = tracing_setup::JsonLogFormat::Pretty;
    }

    // Pre-calculate session ID for logging context
    let log_session_id = compute_log_session_id(&cli.command);

    // Initialize tracing (holds file handle)
    let _guard = tracing_setup::init(
        cli.verbose,
        cli.log_json,
        json_format,
        cli.inspect,
        log_session_id.as_deref(),
    );

    // Build the application service with all dependencies
    let service = build_app_service()?;

    // Dispatch command to appropriate adapter
    let result = match cli.command {
        Commands::Serve(args) => serve_command(args, service).await,
        command => {
            let adapter = CliAdapter::new(service);
            adapter.execute(command).await
        }
    };

    if let Err(ref e) = result {
        tracing::error!("Command failed: {:#}", e);
    }

    result
}

/// Build the application service with all injected dependencies.
fn build_app_service() -> Result<Arc<dyn WorkflowService>> {
    let store = SessionStore::open(None)?;
    let renderer = Arc::new(HandlebarsRenderer::new());
    let (file_system, clock, telemetry) = default_runner_deps();

    let llm_factory: LlmClientFactory = Arc::new(
        |provider: &str, model: &str, max_concurrent: usize, api_key: String| {
            let llm_provider = LlmProvider::from_name(provider)
                .ok_or_else(|| anyhow!("Unknown LLM provider: {}", provider))?;
            let client =
                RigLlmClient::new(llm_provider, api_key, model.to_string(), max_concurrent)?;
            Ok(Arc::new(client) as Arc<dyn LlmClient>)
        },
    );

    let api_key_resolver: ApiKeyResolver = Arc::new(|cli_value: Option<String>, provider: &str| {
        let llm_provider = LlmProvider::from_name(provider)
            .ok_or_else(|| anyhow!("Unknown LLM provider: {}", provider))?;
        resolve_api_key(cli_value, llm_provider)
    });

    let service = AppService::new(
        store,
        renderer,
        file_system,
        clock,
        telemetry,
        llm_factory,
        api_key_resolver,
    );

    Ok(Arc::new(service))
}

/// Handle the serve command separately since it needs special setup.
async fn serve_command(args: ServeArgs, service: Arc<dyn WorkflowService>) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .map_err(|_| anyhow!("Invalid bind/port combination for serve command"))?;

    let options = ServeOptions {
        default_limit: args.limit.max(1),
        poll_interval: Duration::from_millis(args.poll_interval_ms.max(250)),
    };

    tracing::info!("Serving session API on http://{addr}");
    let adapter = ServerAdapter::new(service, options);
    adapter.run(addr).await
}

/// Compute the session ID for log file naming.
fn compute_log_session_id(command: &Commands) -> Option<String> {
    match command {
        Commands::Run(_) => Some(uuid::Uuid::new_v4().to_string()),
        Commands::Resume(args) => Some(args.session_id.clone()),
        Commands::Subprocess(_) => Some(format!("subprocess-{}", uuid::Uuid::new_v4())),
        Commands::Status(args) => args.session_id.clone(),
        Commands::Serve(_) | Commands::Help(_) => None,
    }
}

/// Create default runtime dependencies for the runner.
fn default_runner_deps() -> (Arc<dyn FileSystem>, Arc<dyn Clock>, Arc<dyn TelemetrySink>) {
    let file_system: Arc<dyn FileSystem> = Arc::new(StdFileSystem::new());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let telemetry: Arc<dyn TelemetrySink> = Arc::new(TracingTelemetrySink::new());
    (file_system, clock, telemetry)
}

/// Resolve API key from CLI value or environment.
fn resolve_api_key(cli_value: Option<String>, provider: LlmProvider) -> Result<String> {
    ensure_home_env_loaded();
    let env_var = provider.env_var();
    let env_value = std::env::var(env_var).ok();
    pick_api_key(cli_value, env_value)
        .map_err(|_| anyhow!("Missing API key: pass --api-key or set {env_var}"))
}

fn pick_api_key(cli_value: Option<String>, env_value: Option<String>) -> Result<String> {
    if let Some(key) = normalize_key(cli_value) {
        return Ok(key);
    }
    if let Some(key) = normalize_key(env_value) {
        return Ok(key);
    }
    Err(anyhow!("Missing API key"))
}

fn normalize_key(value: Option<String>) -> Option<String> {
    value.and_then(|candidate| {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn ensure_home_env_loaded() {
    HOME_ENV_ONCE.get_or_init(|| {
        let candidates = paths::env_file_candidates();
        if let Some(contents) = load_env_from_candidates(&candidates) {
            apply_env_contents(&contents);
        }
    });
}

fn load_env_from_candidates(paths: &[PathBuf]) -> Option<String> {
    for path in paths {
        if let Ok(contents) = fs::read_to_string(path) {
            return Some(contents);
        }
    }
    None
}

fn apply_env_contents(contents: &str) {
    for line in contents.lines() {
        if let Some((key, value)) = parse_env_assignment(line)
            && std::env::var_os(&key).is_none()
        {
            unsafe {
                std::env::set_var(&key, &value);
            }
        }
    }
}

fn parse_env_assignment(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }

    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();

    let (key, value) = trimmed.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }

    let value = normalize_env_value(value.trim());
    Some((key.to_string(), value))
}

fn normalize_env_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('\"') && trimmed.ends_with('\"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_api_key_prefers_cli_value() {
        let key = pick_api_key(Some(" cli ".into()), Some("env".into())).expect("CLI key used");
        assert_eq!(key, "cli");
    }

    #[test]
    fn pick_api_key_falls_back_to_env() {
        let key = pick_api_key(None, Some("env-key".into())).expect("env key used");
        assert_eq!(key, "env-key");
    }

    #[test]
    fn pick_api_key_errors_when_missing() {
        let err = pick_api_key(None, None).unwrap_err();
        assert!(err.to_string().contains("Missing API key"));
    }

    #[test]
    fn parse_env_assignment_handles_export_and_quotes() {
        let parsed =
            parse_env_assignment(" export OPENAI_API_KEY=\"abc123\" ").expect("assignment parsed");
        assert_eq!(parsed.0, "OPENAI_API_KEY");
        assert_eq!(parsed.1, "abc123");
    }

    #[test]
    fn parse_env_assignment_skips_comments() {
        assert!(parse_env_assignment(" # comment").is_none());
        assert!(parse_env_assignment("   ").is_none());
        assert!(parse_env_assignment("invalidline").is_none());
    }

    #[test]
    fn apply_env_contents_respects_existing_vars() {
        const NEW_VAR: &str = "MF_TEST_NEW";
        const EXISTING_VAR: &str = "MF_TEST_EXISTING";

        unsafe {
            std::env::remove_var(NEW_VAR);
            std::env::set_var(EXISTING_VAR, "original");
        }

        apply_env_contents(&format!("{NEW_VAR}=fromfile\n{EXISTING_VAR}=override"));

        assert_eq!(std::env::var(NEW_VAR).unwrap(), "fromfile");
        assert_eq!(std::env::var(EXISTING_VAR).unwrap(), "original");

        unsafe {
            std::env::remove_var(NEW_VAR);
            std::env::remove_var(EXISTING_VAR);
        }
    }
}
