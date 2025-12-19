//! Application service implementation that provides the `WorkflowService` trait.
//! This is the primary use-case port implementation that driving adapters consume.

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use uuid::Uuid;

use crate::{
    adapters::outbound::persistence::{
        SessionEnvelope, SessionMetadata, SessionStatus, SessionStore,
    },
    config::MicrofactoryConfig,
    core::{
        domain::{Context, WorkItem},
        error::{Error as CoreError, Result as CoreResult},
        ports::{
            Clock, DryRunResult, FileSystem, LlmClient, LlmOptions, PauseInfo, PromptRenderer,
            ResumeSessionRequest, RunSessionRequest, SessionDetail, SessionMetadataInfo,
            SessionOutcome, SessionSummary, SubprocessMetrics, SubprocessOutcome,
            SubprocessRequest, TelemetrySink, WorkflowService,
        },
    },
    runner::{FlowRunner, RunnerOptions, RunnerOutcome},
    status_export::count_completed_steps,
};

/// Factory function type for creating LLM clients.
pub type LlmClientFactory =
    Arc<dyn Fn(&str, &str, usize, String) -> anyhow::Result<Arc<dyn LlmClient>> + Send + Sync>;

/// Factory function type for resolving API keys.
pub type ApiKeyResolver = Arc<dyn Fn(Option<String>, &str) -> anyhow::Result<String> + Send + Sync>;

/// Application service that implements `WorkflowService`.
///
/// This struct holds all the dependencies needed to execute workflow operations
/// and is injected into driving adapters (CLI, HTTP server).
pub struct AppService {
    store: SessionStore,
    renderer: Arc<dyn PromptRenderer>,
    file_system: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    telemetry: Arc<dyn TelemetrySink>,
    llm_factory: LlmClientFactory,
    api_key_resolver: ApiKeyResolver,
}

impl AppService {
    pub fn new(
        store: SessionStore,
        renderer: Arc<dyn PromptRenderer>,
        file_system: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        telemetry: Arc<dyn TelemetrySink>,
        llm_factory: LlmClientFactory,
        api_key_resolver: ApiKeyResolver,
    ) -> Self {
        Self {
            store,
            renderer,
            file_system,
            clock,
            telemetry,
            llm_factory,
            api_key_resolver,
        }
    }

    fn load_config(&self, path: &std::path::Path) -> anyhow::Result<Arc<MicrofactoryConfig>> {
        Ok(Arc::new(MicrofactoryConfig::from_path(path)?))
    }

    fn ensure_domain_exists(
        &self,
        config: &MicrofactoryConfig,
        domain: &str,
    ) -> anyhow::Result<()> {
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
                "Domain '{domain}' not defined in provided configuration. Available domains: {available}"
            ));
        }
        Ok(())
    }

    fn create_llm_client(
        &self,
        provider: &str,
        model: &str,
        max_concurrent: usize,
        api_key: Option<String>,
    ) -> anyhow::Result<Arc<dyn LlmClient>> {
        let resolved_key = (self.api_key_resolver)(api_key, provider)?;
        (self.llm_factory)(provider, model, max_concurrent, resolved_key)
    }

    fn runner_options_from_request(&self, req: &RunSessionRequest) -> RunnerOptions {
        RunnerOptions::from_cli(
            req.samples,
            req.k,
            req.adaptive_k,
            req.step_by_step,
            req.human_low_margin_threshold,
        )
    }

    fn outcome_from_runner_result(
        &self,
        session_id: &str,
        result: RunnerOutcome,
    ) -> SessionOutcome {
        match result {
            RunnerOutcome::Completed => SessionOutcome {
                session_id: session_id.to_string(),
                completed: true,
                paused: false,
                pause_reason: None,
            },
            RunnerOutcome::Paused(wait) => SessionOutcome {
                session_id: session_id.to_string(),
                completed: false,
                paused: true,
                pause_reason: Some(PauseInfo {
                    step_id: wait.step_id,
                    trigger: wait.trigger,
                    details: wait.details,
                }),
            },
        }
    }
}

#[async_trait]
impl WorkflowService for AppService {
    async fn run_session(&self, request: RunSessionRequest) -> CoreResult<SessionOutcome> {
        let config = self
            .load_config(&request.config_path)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        self.ensure_domain_exists(&config, &request.domain)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        let llm_client = self
            .create_llm_client(
                &request.llm_provider,
                &request.llm_model,
                request.max_concurrent_llm,
                request.api_key.clone(),
            )
            .map_err(|e| CoreError::System(e.to_string()))?;

        let session_id = Uuid::new_v4().to_string();
        let mut context = Context::new(&request.prompt, &request.domain);
        context.session_id = session_id.clone();
        context.dry_run = request.dry_run;
        context.output_dir = request.output_dir.clone();

        tracing::info!(
            "Starting session {} (domain: {})",
            context.session_id,
            context.domain
        );

        let metadata = SessionMetadata {
            config_path: request.config_path.to_string_lossy().to_string(),
            llm_provider: request.llm_provider.clone(),
            llm_model: request.llm_model.clone(),
            max_concurrent_llm: request.max_concurrent_llm,
            samples: request.samples,
            k: request.k,
            adaptive_k: request.adaptive_k,
            human_low_margin_threshold: request.human_low_margin_threshold,
        };

        let mut envelope = SessionEnvelope {
            context: context.clone(),
            metadata,
        };

        self.store
            .save(&envelope, SessionStatus::Running)
            .map_err(|e| CoreError::Persistence(e.to_string()))?;

        let runner_options = self.runner_options_from_request(&request);
        let runner = FlowRunner::new(
            config,
            Some(llm_client),
            self.renderer.clone(),
            runner_options,
            self.file_system.clone(),
            self.clock.clone(),
            self.telemetry.clone(),
        );

        match runner.execute(&mut context).await {
            Ok(outcome) => {
                envelope.context = context.clone();
                let status = match &outcome {
                    RunnerOutcome::Completed => SessionStatus::Completed,
                    RunnerOutcome::Paused(wait) => {
                        tracing::info!(
                            "Session {} paused at step {} ({}) - {}",
                            context.session_id,
                            wait.step_id,
                            wait.trigger,
                            wait.details
                        );
                        SessionStatus::Paused
                    }
                };
                self.store
                    .save(&envelope, status)
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;

                if matches!(outcome, RunnerOutcome::Completed) {
                    tracing::info!("Session {} completed successfully.", context.session_id);
                } else {
                    tracing::info!(
                        "Use `microfactory resume --session-id {}` after resolving the issue.",
                        context.session_id
                    );
                }

                Ok(self.outcome_from_runner_result(&session_id, outcome))
            }
            Err(err) => {
                envelope.context = context;
                self.store
                    .save(&envelope, SessionStatus::Failed)
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                Err(CoreError::System(err.to_string()))
            }
        }
    }

    async fn resume_session(&self, request: ResumeSessionRequest) -> CoreResult<SessionOutcome> {
        let record = self
            .store
            .load(&request.session_id)
            .map_err(|e| CoreError::Persistence(e.to_string()))?;

        let mut context = record.envelope.context;
        let prev_metadata = record.envelope.metadata;

        let provider = request
            .llm_provider
            .clone()
            .unwrap_or_else(|| prev_metadata.llm_provider.clone());
        let model = request
            .llm_model
            .clone()
            .unwrap_or_else(|| prev_metadata.llm_model.clone());
        let max_concurrent = request
            .max_concurrent_llm
            .unwrap_or(prev_metadata.max_concurrent_llm);
        let samples = request.samples.unwrap_or(prev_metadata.samples);
        let k = request.k.unwrap_or(prev_metadata.k);
        let adaptive = prev_metadata.adaptive_k;
        let human_low_margin_threshold = request
            .human_low_margin_threshold
            .unwrap_or(prev_metadata.human_low_margin_threshold);

        let config_path = request
            .config_path
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from(&prev_metadata.config_path));

        let config = self
            .load_config(&config_path)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        self.ensure_domain_exists(&config, &context.domain)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        if let Some(wait) = &context.wait_state {
            tracing::info!(
                "Resuming session {} previously paused at step {} ({}) - {}",
                context.session_id,
                wait.step_id,
                wait.trigger,
                wait.details
            );
        }
        context.clear_wait_state();

        let llm_client = self
            .create_llm_client(&provider, &model, max_concurrent, request.api_key.clone())
            .map_err(|e| CoreError::System(e.to_string()))?;

        let runner_options =
            RunnerOptions::from_cli(samples, k, adaptive, false, human_low_margin_threshold);

        let metadata = SessionMetadata {
            config_path: config_path.to_string_lossy().to_string(),
            llm_provider: provider,
            llm_model: model,
            max_concurrent_llm: max_concurrent,
            samples,
            k,
            adaptive_k: adaptive,
            human_low_margin_threshold,
        };

        let mut envelope = SessionEnvelope {
            context: context.clone(),
            metadata,
        };

        self.store
            .save(&envelope, SessionStatus::Running)
            .map_err(|e| CoreError::Persistence(e.to_string()))?;

        let runner = FlowRunner::new(
            config,
            Some(llm_client),
            self.renderer.clone(),
            runner_options,
            self.file_system.clone(),
            self.clock.clone(),
            self.telemetry.clone(),
        );

        let session_id = context.session_id.clone();
        match runner.execute(&mut context).await {
            Ok(outcome) => {
                envelope.context = context.clone();
                let status = match &outcome {
                    RunnerOutcome::Completed => SessionStatus::Completed,
                    RunnerOutcome::Paused(wait) => {
                        tracing::info!(
                            "Session {} paused again at step {} ({}) - {}",
                            context.session_id,
                            wait.step_id,
                            wait.trigger,
                            wait.details
                        );
                        SessionStatus::Paused
                    }
                };
                self.store
                    .save(&envelope, status)
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;

                if matches!(outcome, RunnerOutcome::Completed) {
                    tracing::info!("Session {} completed.", context.session_id);
                } else {
                    tracing::info!(
                        "Use `microfactory resume --session-id {}` once resolved.",
                        context.session_id
                    );
                }

                Ok(self.outcome_from_runner_result(&session_id, outcome))
            }
            Err(err) => {
                envelope.context = context;
                self.store
                    .save(&envelope, SessionStatus::Failed)
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                Err(CoreError::System(err.to_string()))
            }
        }
    }

    async fn run_subprocess(&self, request: SubprocessRequest) -> CoreResult<SubprocessOutcome> {
        let config = self
            .load_config(&request.config_path)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        self.ensure_domain_exists(&config, &request.domain)
            .map_err(|e| CoreError::Config(e.to_string()))?;

        let llm_client = self
            .create_llm_client(
                &request.llm_provider,
                &request.llm_model,
                request.max_concurrent_llm,
                request.api_key.clone(),
            )
            .map_err(|e| CoreError::System(e.to_string()))?;

        let session_id = format!("subprocess-{}", Uuid::new_v4());
        let mut context = Context::new(&request.step, &request.domain);
        context.session_id = session_id.clone();

        if let Some(extra) = &request.context_json {
            context
                .domain_data
                .insert("context_json".into(), extra.clone());
        }

        let root_id = context.ensure_root();
        context.work_queue.clear();
        context.enqueue_work(WorkItem::Solve { step_id: root_id });
        context.enqueue_work(WorkItem::SolutionVote { step_id: root_id });

        let runner_options = RunnerOptions::from_cli(request.samples, request.k, false, false, 1);
        let runner = FlowRunner::new(
            config,
            Some(llm_client),
            self.renderer.clone(),
            runner_options,
            self.file_system.clone(),
            self.clock.clone(),
            self.telemetry.clone(),
        );

        match runner
            .execute(&mut context)
            .await
            .map_err(|e| CoreError::System(e.to_string()))?
        {
            RunnerOutcome::Completed => {
                let step = context.step(root_id).ok_or_else(|| {
                    CoreError::System("Root step missing after subprocess run".into())
                })?;
                let metrics = context.metrics().step_metrics(root_id).cloned();

                Ok(SubprocessOutcome {
                    session_id,
                    step_id: root_id,
                    candidate_solutions: step.candidate_solutions.clone(),
                    winning_solution: step.winning_solution.clone(),
                    metrics: metrics.map(|m| SubprocessMetrics {
                        samples_requested: m.samples_requested,
                        samples_accepted: m.samples_retained,
                        vote_margin: m.vote_margin,
                    }),
                })
            }
            RunnerOutcome::Paused(wait) => Err(CoreError::System(format!(
                "Subprocess paused at step {} ({}) - {}",
                wait.step_id, wait.trigger, wait.details
            ))),
        }
    }

    async fn get_session(&self, session_id: &str) -> CoreResult<Option<SessionDetail>> {
        match self.store.load(session_id) {
            Ok(record) => {
                let context = &record.envelope.context;
                let wait_state = context.wait_state.as_ref().map(|w| PauseInfo {
                    step_id: w.step_id,
                    trigger: w.trigger.clone(),
                    details: w.details.clone(),
                });

                Ok(Some(SessionDetail {
                    session_id: context.session_id.clone(),
                    domain: context.domain.clone(),
                    prompt: context.prompt.clone(),
                    status: record.status.as_str().to_string(),
                    updated_at: record.updated_at.to_string(),
                    steps_completed: count_completed_steps(context),
                    wait_state,
                    metadata: SessionMetadataInfo {
                        config_path: record.envelope.metadata.config_path.clone(),
                        llm_provider: record.envelope.metadata.llm_provider.clone(),
                        llm_model: record.envelope.metadata.llm_model.clone(),
                        samples: record.envelope.metadata.samples,
                        k: record.envelope.metadata.k,
                    },
                }))
            }
            Err(e) if e.to_string().contains("not found") => Ok(None),
            Err(e) => Err(CoreError::Persistence(e.to_string())),
        }
    }

    async fn list_sessions(&self, limit: usize) -> CoreResult<Vec<SessionSummary>> {
        let summaries = self
            .store
            .list(limit)
            .map_err(|e| CoreError::Persistence(e.to_string()))?;

        Ok(summaries
            .into_iter()
            .map(|s| SessionSummary {
                session_id: s.session_id,
                domain: s.domain,
                prompt: s.prompt,
                status: s.status.as_str().to_string(),
                updated_at: s.updated_at.to_string(),
            })
            .collect())
    }

    async fn dry_run_probe(&self, request: &RunSessionRequest) -> CoreResult<DryRunResult> {
        tracing::info!("[dry-run] probing model '{}'...", request.llm_model);

        let llm_client = self
            .create_llm_client(
                &request.llm_provider,
                &request.llm_model,
                request.max_concurrent_llm,
                request.api_key.clone(),
            )
            .map_err(|e| CoreError::System(e.to_string()))?;

        let response = llm_client
            .chat_completion(&request.llm_model, &request.prompt, &LlmOptions::default())
            .await?;

        Ok(DryRunResult {
            model: request.llm_model.clone(),
            response,
        })
    }
}
