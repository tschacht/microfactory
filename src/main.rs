use std::{
    fs,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use clap::Parser;
use serde::Serialize;
use uuid::Uuid;

use microfactory::{
    cli::{Cli, Commands, LlmProvider, ResumeArgs, RunArgs, ServeArgs, StatusArgs, SubprocessArgs},
    config::MicrofactoryConfig,
    context::{Context, StepMetrics, WorkItem},
    llm::{LlmClient, RigLlmClient},
    paths::home_env_path,
    persistence::{SessionEnvelope, SessionMetadata, SessionStatus, SessionStore},
    runner::{FlowRunner, RunnerOptions, RunnerOutcome},
    server::{self, ServeOptions},
    status_export::{SessionDetailExport, SessionListExport, count_completed_steps},
};

static HOME_ENV_ONCE: OnceLock<()> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run(args) => run_command(args).await?,
        Commands::Status(args) => status_command(args).await?,
        Commands::Resume(args) => resume_command(args).await?,
        Commands::Subprocess(args) => subprocess_command(args).await?,
        Commands::Serve(args) => serve_command(args).await?,
    }
    Ok(())
}

async fn run_command(args: RunArgs) -> Result<()> {
    let config = Arc::new(load_config(&args.config)?);
    ensure_domain_exists(&config, &args.domain)?;

    let llm_client: Arc<dyn LlmClient> = Arc::new(create_llm_client(
        args.llm_provider,
        args.llm_model.clone(),
        args.max_concurrent_llm,
        resolve_api_key(args.api_key.clone(), args.llm_provider)?,
    )?);
    let runner_options = RunnerOptions::from_cli(args.samples, args.k, args.adaptive_k);
    let mut context = Context::new(&args.prompt, &args.domain);
    context.session_id = new_session_id();
    context.dry_run = args.dry_run;

    if args.dry_run {
        run_dry_run_probe(&args, llm_client.clone()).await?;
        return Ok(());
    }

    println!(
        "Starting session {} (domain: {})",
        context.session_id, context.domain
    );

    let metadata = SessionMetadata {
        config_path: args.config.to_string_lossy().to_string(),
        llm_provider: args.llm_provider.as_str().to_string(),
        llm_model: args.llm_model.clone(),
        max_concurrent_llm: args.max_concurrent_llm,
        samples: args.samples,
        k: args.k,
        adaptive_k: args.adaptive_k,
    };
    let store = SessionStore::open(None)?;
    let mut envelope = SessionEnvelope {
        context: context.clone(),
        metadata: metadata.clone(),
    };
    store.save(&envelope, SessionStatus::Running)?;

    let runner = FlowRunner::new(config, Some(llm_client), runner_options);
    match runner.execute(&mut context).await {
        Ok(outcome) => {
            envelope.context = context.clone();
            let status = match &outcome {
                RunnerOutcome::Completed => SessionStatus::Completed,
                RunnerOutcome::Paused(wait) => {
                    println!(
                        "Session {} paused at step {} ({}) - {}",
                        context.session_id, wait.step_id, wait.trigger, wait.details
                    );
                    SessionStatus::Paused
                }
            };
            store.save(&envelope, status)?;
            match outcome {
                RunnerOutcome::Completed => {
                    println!("Session {} completed successfully.", context.session_id);
                }
                RunnerOutcome::Paused(_) => {
                    println!(
                        "Use `microfactory resume --session-id {}` after resolving the issue.",
                        context.session_id
                    );
                }
            }
        }
        Err(err) => {
            envelope.context = context.clone();
            store.save(&envelope, SessionStatus::Failed)?;
            return Err(err);
        }
    }

    Ok(())
}

async fn status_command(args: StatusArgs) -> Result<()> {
    let store = SessionStore::open(None)?;
    if let Some(id) = args.session_id {
        let record = store.load(&id)?;
        if args.json {
            let summary = SessionDetailExport::from_record(&record);
            println!("{}", serde_json::to_string_pretty(&summary)?);
        } else {
            println!("Session: {}", id);
            println!("Status: {}", record.status.as_str());
            println!("Prompt: {}", record.envelope.context.prompt);
            println!("Domain: {}", record.envelope.context.domain);
            println!("Updated: {}", record.updated_at);
            if let Some(wait) = &record.envelope.context.wait_state {
                println!(
                    "Waiting on step {} ({}) - {}",
                    wait.step_id, wait.trigger, wait.details
                );
            }
            println!(
                "Steps completed: {}",
                count_completed_steps(&record.envelope.context)
            );
        }
    } else {
        let limit = args.limit.max(1);
        let summaries = store.list(limit)?;
        if args.json {
            let payload = SessionListExport::from_summaries(summaries);
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else if summaries.is_empty() {
            println!("No sessions recorded yet.");
        } else {
            println!("Recent sessions:");
            for summary in summaries {
                println!(
                    "- {} [{}] domain={} updated={} prompt={}",
                    summary.session_id,
                    summary.status.as_str(),
                    summary.domain,
                    summary.updated_at,
                    summary.prompt
                );
            }
        }
    }
    Ok(())
}

async fn resume_command(args: ResumeArgs) -> Result<()> {
    let store = SessionStore::open(None)?;
    let record = store.load(&args.session_id)?;
    let mut context = record.envelope.context;
    let prev_metadata = record.envelope.metadata;

    let provider = args
        .llm_provider
        .or_else(|| LlmProvider::from_name(prev_metadata.llm_provider.as_str()))
        .ok_or_else(|| {
            anyhow!(
                "Session stored unsupported provider '{}'",
                prev_metadata.llm_provider
            )
        })?;
    let model = args
        .llm_model
        .unwrap_or_else(|| prev_metadata.llm_model.clone());
    let max_concurrent = args
        .max_concurrent_llm
        .unwrap_or(prev_metadata.max_concurrent_llm);
    let samples = args.samples.unwrap_or(prev_metadata.samples);
    let k = args.k.unwrap_or(prev_metadata.k);
    let adaptive = prev_metadata.adaptive_k;

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| PathBuf::from(prev_metadata.config_path.clone()));
    let config = Arc::new(load_config(&config_path)?);
    ensure_domain_exists(&config, &context.domain)?;

    if let Some(wait) = &context.wait_state {
        println!(
            "Resuming session {} previously paused at step {} ({}) - {}",
            context.session_id, wait.step_id, wait.trigger, wait.details
        );
    }
    context.clear_wait_state();

    let api_key = resolve_api_key(args.api_key.clone(), provider)?;
    let llm_client: Arc<dyn LlmClient> = Arc::new(create_llm_client(
        provider,
        model.clone(),
        max_concurrent,
        api_key,
    )?);
    let runner_options = RunnerOptions::from_cli(samples, k, adaptive);

    let metadata = SessionMetadata {
        config_path: config_path.to_string_lossy().to_string(),
        llm_provider: provider.as_str().to_string(),
        llm_model: model.clone(),
        max_concurrent_llm: max_concurrent,
        samples,
        k,
        adaptive_k: adaptive,
    };
    let mut envelope = SessionEnvelope {
        context: context.clone(),
        metadata: metadata.clone(),
    };
    store.save(&envelope, SessionStatus::Running)?;

    let runner = FlowRunner::new(config, Some(llm_client), runner_options);
    match runner.execute(&mut context).await {
        Ok(outcome) => {
            envelope.context = context.clone();
            let status = match &outcome {
                RunnerOutcome::Completed => SessionStatus::Completed,
                RunnerOutcome::Paused(wait) => {
                    println!(
                        "Session {} paused again at step {} ({}) - {}",
                        context.session_id, wait.step_id, wait.trigger, wait.details
                    );
                    SessionStatus::Paused
                }
            };
            store.save(&envelope, status)?;
            match outcome {
                RunnerOutcome::Completed => {
                    println!("Session {} completed.", context.session_id);
                }
                RunnerOutcome::Paused(_) => {
                    println!(
                        "Use `microfactory resume --session-id {}` once resolved.",
                        context.session_id
                    );
                }
            }
        }
        Err(err) => {
            envelope.context = context.clone();
            store.save(&envelope, SessionStatus::Failed)?;
            return Err(err);
        }
    }

    Ok(())
}

async fn subprocess_command(args: SubprocessArgs) -> Result<()> {
    let config = Arc::new(load_config(&args.config)?);
    ensure_domain_exists(&config, &args.domain)?;
    let api_key = resolve_api_key(args.api_key.clone(), args.llm_provider)?;
    let llm_client: Arc<dyn LlmClient> = Arc::new(create_llm_client(
        args.llm_provider,
        args.llm_model.clone(),
        args.max_concurrent_llm,
        api_key,
    )?);

    let mut context = Context::new(&args.step, &args.domain);
    context.session_id = format!("subprocess-{}", new_session_id());
    if let Some(extra) = &args.context_json {
        context
            .domain_data
            .insert("context_json".into(), extra.clone());
    }
    let root_id = context.ensure_root();
    context.work_queue.clear();
    context.enqueue_work(WorkItem::Solve { step_id: root_id });
    context.enqueue_work(WorkItem::SolutionVote { step_id: root_id });

    let runner_options = RunnerOptions::from_cli(args.samples, args.k, false);
    let runner = FlowRunner::new(config, Some(llm_client), runner_options);
    match runner.execute(&mut context).await? {
        RunnerOutcome::Completed => {
            let step = context
                .step(root_id)
                .with_context(|| "Root step missing after subprocess run")?;
            let metrics = context.metrics().step_metrics(root_id).cloned();
            let output = SubprocessOutput {
                session_id: context.session_id.clone(),
                step_id: root_id,
                candidate_solutions: step.candidate_solutions.clone(),
                winning_solution: step.winning_solution.clone(),
                metrics,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        RunnerOutcome::Paused(wait) => {
            return Err(anyhow!(
                "Subprocess paused at step {} ({}) - {}",
                wait.step_id,
                wait.trigger,
                wait.details
            ));
        }
    }

    Ok(())
}

async fn serve_command(args: ServeArgs) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .context("Invalid bind/port combination for serve command")?;
    let store = SessionStore::open(None)?;
    let options = ServeOptions {
        default_limit: args.limit.max(1),
        poll_interval: Duration::from_millis(args.poll_interval_ms.max(250)),
    };
    println!("Serving session API on http://{addr}");
    server::run(addr, store, options).await
}

fn load_config(path: &PathBuf) -> Result<MicrofactoryConfig> {
    MicrofactoryConfig::from_path(path)
}

fn create_llm_client(
    provider: LlmProvider,
    model: String,
    max_concurrent: usize,
    api_key: String,
) -> Result<RigLlmClient> {
    RigLlmClient::new(provider, api_key, model, max_concurrent)
}

fn resolve_api_key(cli_value: Option<String>, provider: LlmProvider) -> Result<String> {
    ensure_home_env_loaded();
    let env_var = provider.env_var();
    let env_value = std::env::var(env_var).ok();
    pick_api_key(cli_value, env_value)
        .map_err(|_| anyhow!("Missing API key: pass --api-key or set {}", env_var))
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
        if let Some(path) = home_env_path()
            && let Ok(contents) = fs::read_to_string(&path)
        {
            apply_env_contents(&contents);
        }
    });
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

async fn run_dry_run_probe(args: &RunArgs, llm: Arc<dyn LlmClient>) -> Result<()> {
    println!(
        "[dry-run] probing model '{}' with prompt...",
        args.llm_model
    );
    let response = llm.sample(&args.prompt, Some(&args.llm_model)).await?;
    println!("--- LLM Response Start ---\n{response}\n--- LLM Response End ---");
    Ok(())
}

fn new_session_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Serialize)]
struct SubprocessOutput {
    session_id: String,
    step_id: usize,
    candidate_solutions: Vec<String>,
    winning_solution: Option<String>,
    metrics: Option<StepMetrics>,
}

fn ensure_domain_exists(config: &Arc<MicrofactoryConfig>, domain: &str) -> Result<()> {
    if config.domain(domain).is_none() {
        let available = if config.domains.is_empty() {
            "<none>".to_string()
        } else {
            config
                .domains
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(anyhow!(
            "Domain '{}' not defined in provided configuration. Available domains: {}",
            domain,
            available
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use async_trait::async_trait;
    use std::{path::PathBuf, sync::Arc};

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

    #[tokio::test]
    async fn dry_run_probe_bubbles_llm_errors() {
        struct FailingClient;

        #[async_trait]
        impl LlmClient for FailingClient {
            async fn sample(&self, _: &str, _: Option<&str>) -> Result<String> {
                Err(anyhow!("boom"))
            }
        }

        let args = RunArgs {
            prompt: "demo".into(),
            config: PathBuf::from("config.yaml"),
            domain: "code".into(),
            api_key: Some("key".into()),
            llm_model: "gpt".into(),
            llm_provider: LlmProvider::Openai,
            samples: 1,
            k: 1,
            adaptive_k: false,
            max_concurrent_llm: 1,
            repo_path: None,
            dry_run: true,
        };

        let client: Arc<dyn LlmClient> = Arc::new(FailingClient);
        let err = run_dry_run_probe(&args, client).await.unwrap_err();
        assert!(err.to_string().contains("boom"));
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
