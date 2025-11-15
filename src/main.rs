use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, anyhow};
use clap::Parser;

use microfactory::{
    cli::{Cli, Commands, ResumeArgs, RunArgs, StatusArgs},
    config::MicrofactoryConfig,
    context::Context,
    runner::FlowRunner,
};

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

    let mut context = Context::new(&args.prompt, &args.domain);
    context.dry_run = args.dry_run;

    let runner = FlowRunner::new(config);
    runner.execute(&mut context).await?;
    Ok(())
}

async fn status_command(args: StatusArgs) -> Result<()> {
    let config = Arc::new(load_config(&default_config_path())?);
    let runner = FlowRunner::new(config);
    runner.status(args.session_id.as_deref())?;
    Ok(())
}

async fn resume_command(args: ResumeArgs) -> Result<()> {
    let config = Arc::new(load_config(&default_config_path())?);
    let mut context = Context::default();
    context.session_id = args.session_id;

    let runner = FlowRunner::new(config);
    runner.execute(&mut context).await?;
    Ok(())
}

fn load_config(path: &PathBuf) -> Result<MicrofactoryConfig> {
    MicrofactoryConfig::from_path(path)
}

fn default_config_path() -> PathBuf {
    PathBuf::from("config.yaml")
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
