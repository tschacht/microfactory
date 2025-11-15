use std::{
    fs,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use anyhow::{Result, anyhow};
use clap::Parser;

use microfactory::{
    cli::{Cli, Commands, LlmProvider, ResumeArgs, RunArgs, StatusArgs},
    config::MicrofactoryConfig,
    context::Context,
    llm::{LlmClient, RigLlmClient},
    runner::FlowRunner,
};

static HOME_ENV_ONCE: OnceLock<()> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Run(args) => run_command(args).await?,
        Commands::Status(args) => status_command(args).await?,
        Commands::Resume(args) => resume_command(args).await?,
    }
    Ok(())
}

async fn run_command(args: RunArgs) -> Result<()> {
    let config = Arc::new(load_config(&args.config)?);
    ensure_domain_exists(&config, &args.domain)?;

    let llm_client: Arc<dyn LlmClient> = Arc::new(create_llm_client(&args)?);
    let mut context = Context::new(&args.prompt, &args.domain);
    context.dry_run = args.dry_run;

    if args.dry_run {
        run_dry_run_probe(&args, llm_client.clone()).await?;
        return Ok(());
    }

    let runner = FlowRunner::new(config, Some(llm_client));
    runner.execute(&mut context).await?;
    Ok(())
}

async fn status_command(args: StatusArgs) -> Result<()> {
    let config = Arc::new(load_config(&default_config_path())?);
    let runner = FlowRunner::new(config, None);
    runner.status(args.session_id.as_deref())?;
    Ok(())
}

async fn resume_command(args: ResumeArgs) -> Result<()> {
    let config = Arc::new(load_config(&default_config_path())?);
    let mut context = Context::default();
    context.session_id = args.session_id;

    let runner = FlowRunner::new(config, None);
    runner.execute(&mut context).await?;
    Ok(())
}

fn load_config(path: &PathBuf) -> Result<MicrofactoryConfig> {
    MicrofactoryConfig::from_path(path)
}

fn default_config_path() -> PathBuf {
    PathBuf::from("config.yaml")
}

fn create_llm_client(args: &RunArgs) -> Result<RigLlmClient> {
    let api_key = resolve_api_key(args)?;
    RigLlmClient::new(
        args.llm_provider,
        api_key,
        args.llm_model.clone(),
        args.max_concurrent_llm,
    )
}

fn resolve_api_key(args: &RunArgs) -> Result<String> {
    ensure_home_env_loaded();
    let env_var = args.llm_provider.env_var();
    let env_value = std::env::var(env_var).ok();
    pick_api_key(args.api_key.clone(), env_value)
        .map_err(|_| anyhow!("Missing API key: pass --api-key or set {}", env_var))
}

fn pick_api_key(cli_value: Option<String>, env_value: Option<String>) -> Result<String> {
    if let Some(key) = normalize_key(cli_value) {
        return Ok(key);
    }
    if let Some(key) = normalize_key(env_value) {
        return Ok(key);
    }

    Err(anyhow!(
        "Missing API key: pass --api-key or set OPENAI_API_KEY"
    ))
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
        if let Some(path) = home_env_path() {
            if let Ok(contents) = fs::read_to_string(&path) {
                apply_env_contents(&contents);
            }
        }
    });
}

fn home_env_path() -> Option<PathBuf> {
    home_dir().map(|mut dir| {
        dir.push(".env");
        dir
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn apply_env_contents(contents: &str) {
    for line in contents.lines() {
        if let Some((key, value)) = parse_env_assignment(line) {
            if std::env::var_os(&key).is_none() {
                unsafe {
                    std::env::set_var(&key, &value);
                }
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
    if trimmed.len() >= 2 {
        if (trimmed.starts_with('\"') && trimmed.ends_with('\"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

async fn run_dry_run_probe(args: &RunArgs, llm: Arc<dyn LlmClient>) -> Result<()> {
    println!(
        "[dry-run] probing model '{}' with prompt...",
        args.llm_model
    );
    let response = llm.sample(&args.prompt).await?;
    println!("--- LLM Response Start ---\n{response}\n--- LLM Response End ---");
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
            async fn sample(&self, _: &str) -> Result<String> {
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

fn ensure_domain_exists(config: &Arc<MicrofactoryConfig>, domain: &str) -> Result<()> {
    if config.domain(domain).is_none() {
        return Err(anyhow!(
            "Domain '{}' not defined in provided configuration",
            domain
        ));
    }
    Ok(())
}
