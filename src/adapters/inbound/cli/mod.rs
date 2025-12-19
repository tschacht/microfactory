//! CLI inbound adapter that translates command-line arguments into application service calls.

mod definitions;
mod help;

pub use definitions::*;

use std::sync::Arc;

use anyhow::Result;

use crate::{
    core::ports::{ResumeSessionRequest, RunSessionRequest, SubprocessRequest, WorkflowService},
    status_export::{SessionListExport, SessionSummaryExport},
};

/// CLI adapter that consumes the `WorkflowService` to execute commands.
pub struct CliAdapter {
    service: Arc<dyn WorkflowService>,
}

impl CliAdapter {
    pub fn new(service: Arc<dyn WorkflowService>) -> Self {
        Self { service }
    }

    /// Execute a CLI command by dispatching to the appropriate service method.
    pub async fn execute(&self, command: Commands) -> Result<()> {
        match command {
            Commands::Run(args) => self.run_command(args).await,
            Commands::Status(args) => self.status_command(args).await,
            Commands::Resume(args) => self.resume_command(args).await,
            Commands::Subprocess(args) => self.subprocess_command(args).await,
            Commands::Serve(_) => {
                // Serve is handled separately in main.rs since it needs special setup
                Err(anyhow::anyhow!(
                    "Serve command should be handled by the composition root"
                ))
            }
            Commands::Help(args) => self.help_command(args).await,
        }
    }

    async fn run_command(&self, args: RunArgs) -> Result<()> {
        if args.dry_run {
            let request = self.run_args_to_request(&args);
            let result = self.service.dry_run_probe(&request).await?;
            println!("[dry-run] probing model '{}' with prompt...", result.model);
            println!(
                "--- LLM Response Start ---\n{}\n--- LLM Response End ---",
                result.response
            );
            return Ok(());
        }

        let request = self.run_args_to_request(&args);
        let outcome = self.service.run_session(request).await?;

        if outcome.paused
            && let Some(reason) = &outcome.pause_reason
        {
            tracing::info!(
                "Session {} paused at step {} ({}) - {}",
                outcome.session_id,
                reason.step_id,
                reason.trigger,
                reason.details
            );
        }

        Ok(())
    }

    async fn status_command(&self, args: StatusArgs) -> Result<()> {
        if let Some(id) = args.session_id {
            let detail = self.service.get_session(&id).await?;
            if let Some(session) = detail {
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&session)?);
                } else {
                    println!("Session: {}", session.session_id);
                    println!("Status: {}", session.status);
                    println!("Prompt: {}", session.prompt);
                    println!("Domain: {}", session.domain);
                    println!("Updated: {}", session.updated_at);
                    if let Some(wait) = &session.wait_state {
                        println!(
                            "Waiting on step {} ({}) - {}",
                            wait.step_id, wait.trigger, wait.details
                        );
                    }
                    println!("Steps completed: {}", session.steps_completed);
                }
            } else {
                return Err(anyhow::anyhow!("Session {id} not found"));
            }
        } else {
            let limit = args.limit.max(1);
            let summaries = self.service.list_sessions(limit).await?;
            if args.json {
                // Convert to export format for backward compatibility
                let export_summaries: Vec<SessionSummaryExport> = summaries
                    .into_iter()
                    .map(|s| SessionSummaryExport {
                        session_id: s.session_id,
                        status: s.status,
                        prompt: s.prompt,
                        domain: s.domain,
                        updated_at: s.updated_at.parse().unwrap_or(0),
                    })
                    .collect();
                let payload = SessionListExport {
                    sessions: export_summaries,
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if summaries.is_empty() {
                println!("No sessions recorded yet.");
            } else {
                println!("Recent sessions:");
                for summary in summaries {
                    println!(
                        "- {} [{}] domain={} updated={} prompt={}",
                        summary.session_id,
                        summary.status,
                        summary.domain,
                        summary.updated_at,
                        summary.prompt
                    );
                }
            }
        }
        Ok(())
    }

    async fn resume_command(&self, args: ResumeArgs) -> Result<()> {
        let request = ResumeSessionRequest {
            session_id: args.session_id.clone(),
            config_path: args.config.clone(),
            llm_provider: args.llm_provider.map(|p| p.as_str().to_string()),
            llm_model: args.llm_model.clone(),
            api_key: args.api_key.clone(),
            samples: args.samples,
            k: args.k,
            max_concurrent_llm: args.max_concurrent_llm,
            human_low_margin_threshold: args.human_low_margin_threshold,
        };

        let outcome = self.service.resume_session(request).await?;

        if outcome.paused
            && let Some(reason) = &outcome.pause_reason
        {
            tracing::info!(
                "Session {} paused at step {} ({}) - {}",
                outcome.session_id,
                reason.step_id,
                reason.trigger,
                reason.details
            );
        }

        Ok(())
    }

    async fn subprocess_command(&self, args: SubprocessArgs) -> Result<()> {
        let request = SubprocessRequest {
            domain: args.domain.clone(),
            config_path: args.config.clone(),
            step: args.step.clone(),
            context_json: args.context_json.clone(),
            llm_provider: args.llm_provider.as_str().to_string(),
            llm_model: args.llm_model.clone(),
            api_key: args.api_key.clone(),
            samples: args.samples,
            k: args.k,
            max_concurrent_llm: args.max_concurrent_llm,
        };

        let outcome = self.service.run_subprocess(request).await?;
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        Ok(())
    }

    async fn help_command(&self, args: HelpArgs) -> Result<()> {
        let topic = args.topic.unwrap_or(HelpTopic::Overview);
        let section = help::build_help_section(topic);
        match args.format {
            HelpFormat::Text => help::render_help_text(&section),
            HelpFormat::Json => println!("{}", serde_json::to_string_pretty(&section)?),
        }
        Ok(())
    }

    fn run_args_to_request(&self, args: &RunArgs) -> RunSessionRequest {
        RunSessionRequest {
            prompt: args.prompt.clone(),
            domain: args.domain.clone(),
            config_path: args.config.clone(),
            llm_provider: args.llm_provider.as_str().to_string(),
            llm_model: args.llm_model.clone(),
            api_key: args.api_key.clone(),
            samples: args.samples,
            k: args.k,
            adaptive_k: args.adaptive_k,
            max_concurrent_llm: args.max_concurrent_llm,
            dry_run: args.dry_run,
            step_by_step: args.step_by_step,
            human_low_margin_threshold: args.human_low_margin_threshold,
            output_dir: args.output_dir.clone(),
        }
    }
}
