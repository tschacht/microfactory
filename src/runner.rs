use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use tracing::{debug, info};

use crate::{
    config::{AgentDefinition, DomainConfig, MicrofactoryConfig},
    context::{AgentConfig, AgentKind, Context, StepStatus, WaitState, WorkItem},
    llm::LlmClient,
    red_flaggers::RedFlagPipeline,
    tasks::{
        DecompositionTask, DecompositionVoteTask, MicroTask, NextAction, SolutionVoteTask,
        SolveTask, TaskEffect,
    },
};

/// Orchestrates MAKER-style workflows across decomposition, solving, and voting tasks.
pub struct FlowRunner {
    config: Arc<MicrofactoryConfig>,
    llm: Option<Arc<dyn LlmClient>>,
    options: RunnerOptions,
}

impl FlowRunner {
    pub fn new(
        config: Arc<MicrofactoryConfig>,
        llm: Option<Arc<dyn LlmClient>>,
        options: RunnerOptions,
    ) -> Self {
        Self {
            config,
            llm,
            options,
        }
    }

    /// Executes pending work items stored in the context until completion or a human-in-loop pause.
    pub async fn execute(&self, context: &mut Context) -> Result<RunnerOutcome> {
        let llm = self
            .llm
            .clone()
            .ok_or_else(|| anyhow!("LLM client required for execution"))?;

        let domain_cfg = self
            .config
            .domain(&context.domain)
            .with_context(|| format!("Unknown domain: {}", context.domain))?;
        let agent_configs = self.agent_configs(domain_cfg);
        let red_flag_pipeline = Arc::new(
            RedFlagPipeline::from_configs(&domain_cfg.red_flaggers)
                .context("Failed to build red-flagger pipeline")?,
        );

        if context.root_step_id().is_none() {
            let root = context.ensure_root();
            context.enqueue_work(WorkItem::Decomposition { step_id: root });
        }

        if !context.has_pending_work() {
            if let Some(root) = context.root_step_id() {
                context.enqueue_work(WorkItem::Decomposition { step_id: root });
            }
        }

        while let Some(item) = context.dequeue_work() {
            let current_item = item.clone();
            match item {
                WorkItem::Decomposition { step_id } => {
                    let step_prompt = context
                        .step(step_id)
                        .map(|s| s.description.clone())
                        .unwrap_or_else(|| context.prompt.clone());
                    let agent = agent_configs
                        .get(&AgentKind::Decomposition)
                        .expect("missing decomposition agent")
                        .clone();
                    let task = DecompositionTask::new(
                        step_id,
                        step_prompt,
                        agent,
                        llm.clone(),
                        red_flag_pipeline.clone(),
                    );
                    let result = task.run(context).await?;
                    if let Some(outcome) =
                        self.handle_next_action(result.action, &current_item, context)
                    {
                        return Ok(outcome);
                    }
                    if let Some(wait) =
                        self.check_sampling_triggers(context, step_id, "decomposition sampling")
                    {
                        return Ok(self.pause_with(context, wait, current_item));
                    }
                    context.enqueue_work_front(WorkItem::DecompositionVote { step_id });
                }
                WorkItem::DecompositionVote { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::DecompositionDiscriminator)
                        .expect("missing decomposition discriminator")
                        .clone();
                    let vote_k =
                        self.resolve_k(AgentKind::DecompositionDiscriminator, &agent, context);
                    let task = DecompositionVoteTask::new(step_id, agent, llm.clone(), vote_k);
                    let result = task.run(context).await?;
                    if let Some(outcome) =
                        self.handle_next_action(result.action, &current_item, context)
                    {
                        return Ok(outcome);
                    }
                    if let Some(wait) =
                        self.check_vote_triggers(context, step_id, "decomposition vote")
                    {
                        return Ok(self.pause_with(
                            context,
                            wait,
                            WorkItem::Decomposition { step_id },
                        ));
                    }

                    if let TaskEffect::SpawnedSteps(children) = result.effect {
                        if children.is_empty() {
                            context.enqueue_work(WorkItem::Solve { step_id });
                        } else {
                            for child in children {
                                let next = if self.should_recurse(context, child) {
                                    WorkItem::Decomposition { step_id: child }
                                } else {
                                    WorkItem::Solve { step_id: child }
                                };
                                context.enqueue_work(next);
                            }
                        }
                    }
                }
                WorkItem::Solve { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::Solver)
                        .expect("missing solver agent")
                        .clone();
                    let task =
                        SolveTask::new(step_id, agent, llm.clone(), red_flag_pipeline.clone());
                    let result = task.run(context).await?;
                    if let Some(outcome) =
                        self.handle_next_action(result.action, &current_item, context)
                    {
                        return Ok(outcome);
                    }
                    if let Some(wait) =
                        self.check_sampling_triggers(context, step_id, "solver sampling")
                    {
                        return Ok(self.pause_with(context, wait, current_item));
                    }
                    if matches!(result.effect, TaskEffect::SolutionsReady { .. }) {
                        context.enqueue_work_front(WorkItem::SolutionVote { step_id });
                    }
                }
                WorkItem::SolutionVote { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::SolutionDiscriminator)
                        .expect("missing solution discriminator")
                        .clone();
                    let vote_k = self.resolve_k(AgentKind::SolutionDiscriminator, &agent, context);
                    let task = SolutionVoteTask::new(step_id, agent, llm.clone(), vote_k);
                    let result = task.run(context).await?;
                    if let Some(outcome) =
                        self.handle_next_action(result.action, &current_item, context)
                    {
                        return Ok(outcome);
                    }
                    if let Some(wait) = self.check_vote_triggers(context, step_id, "solution vote")
                    {
                        return Ok(self.pause_with(context, wait, WorkItem::Solve { step_id }));
                    }
                    if let TaskEffect::StepCompleted { step_id } = result.effect {
                        if let Some(step) = context.step(step_id) {
                            info!(step_id, %step.description, "Step completed");
                        }
                    }
                }
            }
        }

        let completed = context
            .steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Completed))
            .count();
        info!(
            completed,
            total = context.steps.len(),
            "FlowRunner execution complete"
        );
        Ok(RunnerOutcome::Completed)
    }

    fn pause_with(
        &self,
        context: &mut Context,
        wait: WaitState,
        retry_item: WorkItem,
    ) -> RunnerOutcome {
        context.enqueue_work_front(retry_item);
        context.set_wait_state(wait.step_id, wait.trigger.clone(), wait.details.clone());
        RunnerOutcome::Paused(
            context
                .wait_state
                .clone()
                .expect("wait state recorded during pause"),
        )
    }

    fn handle_next_action(
        &self,
        action: NextAction,
        current_item: &WorkItem,
        context: &mut Context,
    ) -> Option<RunnerOutcome> {
        match action {
            NextAction::Continue | NextAction::End => None,
            NextAction::WaitForInput => {
                let wait = WaitState {
                    step_id: current_item.step_id(),
                    trigger: "task_requested_input".into(),
                    details: "Task requested human approval before continuing".into(),
                };
                Some(self.pause_with(context, wait, current_item.clone()))
            }
            NextAction::GoTo(_) => {
                panic!("GoTo transitions are not supported in this phase");
            }
            NextAction::Error(msg) => {
                panic!("Task reported error: {msg}");
            }
        }
    }

    fn should_recurse(&self, context: &Context, step_id: usize) -> bool {
        if let Some(step) = context.step(step_id) {
            if step.depth >= self.options.max_decomposition_depth {
                return false;
            }
            let word_count = step.description.split_whitespace().count();
            return word_count >= self.options.min_words_for_decomposition;
        }
        false
    }

    fn resolve_k(&self, agent_kind: AgentKind, agent: &AgentConfig, context: &Context) -> usize {
        let base = agent.k.unwrap_or(self.options.default_k).max(1);
        if !self.options.adaptive_k {
            return base;
        }

        if let Some(stats) = context.metrics().vote_stats(agent_kind) {
            if !stats.recent_margins.is_empty() {
                let sum: usize = stats.recent_margins.iter().copied().sum();
                let avg = sum as f32 / stats.recent_margins.len() as f32;
                let mut adjusted = base;
                if avg < base as f32 * 0.75 {
                    adjusted = base + 1;
                } else if avg > base as f32 * 1.5 && base > 1 {
                    adjusted = base - 1;
                }
                if adjusted != base {
                    debug!(
                        ?agent_kind,
                        base_k = base,
                        adjusted_k = adjusted,
                        avg_margin = avg,
                        "Adaptive k adjustment"
                    );
                }
                return adjusted.max(1);
            }
        }

        base
    }

    fn check_sampling_triggers(
        &self,
        context: &Context,
        step_id: usize,
        stage: &str,
    ) -> Option<WaitState> {
        let metrics = context.metrics().step_metrics(step_id)?;
        if self.options.human_red_flag_threshold > 0
            && metrics.red_flags.len() >= self.options.human_red_flag_threshold
        {
            return Some(WaitState {
                step_id,
                trigger: format!("{stage}_red_flags"),
                details: format!(
                    "{} samples were red-flagged during {}",
                    metrics.red_flags.len(),
                    stage
                ),
            });
        }
        if self.options.human_resample_threshold > 0
            && metrics.resamples >= self.options.human_resample_threshold
        {
            return Some(WaitState {
                step_id,
                trigger: format!("{stage}_resamples"),
                details: format!(
                    "{} resample attempts exceeded the allowed budget during {}",
                    metrics.resamples, stage
                ),
            });
        }
        None
    }

    fn check_vote_triggers(
        &self,
        context: &Context,
        step_id: usize,
        stage: &str,
    ) -> Option<WaitState> {
        let metrics = context.metrics().step_metrics(step_id)?;
        if self.options.human_low_margin_threshold > 0 {
            if let Some(margin) = metrics.vote_margin {
                if margin <= self.options.human_low_margin_threshold {
                    return Some(WaitState {
                        step_id,
                        trigger: format!("{stage}_low_margin"),
                        details: format!(
                            "Vote margin ({}) during {} fell below threshold",
                            margin, stage
                        ),
                    });
                }
            }
        }
        None
    }

    fn agent_configs(&self, domain: &DomainConfig) -> HashMap<AgentKind, AgentConfig> {
        let mut map = HashMap::new();
        map.insert(
            AgentKind::Decomposition,
            self.build_agent_config(AgentKind::Decomposition, &domain.agents.decomposition),
        );
        map.insert(
            AgentKind::DecompositionDiscriminator,
            self.build_agent_config(
                AgentKind::DecompositionDiscriminator,
                &domain.agents.decomposition_discriminator,
            ),
        );
        map.insert(
            AgentKind::Solver,
            self.build_agent_config(AgentKind::Solver, &domain.agents.solver),
        );
        map.insert(
            AgentKind::SolutionDiscriminator,
            self.build_agent_config(
                AgentKind::SolutionDiscriminator,
                &domain.agents.solution_discriminator,
            ),
        );
        map
    }

    fn build_agent_config(&self, kind: AgentKind, definition: &AgentDefinition) -> AgentConfig {
        AgentConfig {
            kind,
            prompt_template: definition.prompt_template.clone(),
            model: definition.model.clone(),
            samples: definition
                .samples
                .unwrap_or(self.options.default_samples)
                .max(1),
            k: definition.k.or(Some(self.options.default_k)),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RunnerOutcome {
    Completed,
    Paused(WaitState),
}

#[derive(Debug, Clone, Copy)]
pub struct RunnerOptions {
    pub default_samples: usize,
    pub default_k: usize,
    pub adaptive_k: bool,
    pub max_decomposition_depth: usize,
    pub min_words_for_decomposition: usize,
    pub human_red_flag_threshold: usize,
    pub human_resample_threshold: usize,
    pub human_low_margin_threshold: usize,
}

impl RunnerOptions {
    pub fn from_cli(samples: usize, k: usize, adaptive_k: bool) -> Self {
        Self {
            default_samples: samples.max(1),
            default_k: k.max(1),
            adaptive_k,
            max_decomposition_depth: 2,
            min_words_for_decomposition: 8,
            human_red_flag_threshold: 4,
            human_resample_threshold: 4,
            human_low_margin_threshold: 1,
        }
    }
}

impl Default for RunnerOptions {
    fn default() -> Self {
        Self {
            default_samples: 2,
            default_k: 2,
            adaptive_k: false,
            max_decomposition_depth: 2,
            min_words_for_decomposition: 8,
            human_red_flag_threshold: 4,
            human_resample_threshold: 4,
            human_low_margin_threshold: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::MicrofactoryConfig,
        context::{Context, StepStatus},
        llm::LlmClient,
    };
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use std::{collections::VecDeque, sync::Mutex};

    struct ScriptedLlm {
        batches: Mutex<VecDeque<Vec<String>>>,
    }

    impl ScriptedLlm {
        fn new(script: Vec<Vec<String>>) -> Self {
            Self {
                batches: Mutex::new(script.into_iter().collect()),
            }
        }
    }

    #[async_trait]
    impl LlmClient for ScriptedLlm {
        async fn sample(&self, prompt: &str, model: Option<&str>) -> Result<String> {
            let mut single = self.sample_n(prompt, 1, model).await?;
            Ok(single.pop().unwrap())
        }

        async fn sample_n(&self, _: &str, n: usize, _: Option<&str>) -> Result<Vec<String>> {
            let mut guard = self.batches.lock().unwrap();
            let batch = guard.pop_front().expect("no scripted responses left");
            if batch.len() == n {
                Ok(batch)
            } else if batch.len() == 1 && n > 1 {
                Ok(vec![batch[0].clone(); n])
            } else {
                Err(anyhow!("Scripted response length mismatch"))
            }
        }
    }

    #[tokio::test]
    async fn executes_linear_flow_with_scripted_llm() {
        let yaml = r#"
        domains:
          code:
            agents:
              decomposition:
                prompt_template: "decompose"
                model: "model-a"
                samples: 2
              decomposition_discriminator:
                prompt_template: "vote-decompose"
                model: "model-b"
                samples: 2
                k: 2
              solver:
                prompt_template: "solve"
                model: "model-c"
                samples: 2
              solution_discriminator:
                prompt_template: "vote-solution"
                model: "model-d"
                samples: 2
                k: 2
        "#;
        let config = Arc::new(MicrofactoryConfig::from_str(yaml).unwrap());
        let llm: Arc<dyn LlmClient> = Arc::new(ScriptedLlm::new(vec![
            vec![
                "- step one\n- step two".into(),
                "- step one\n- step two".into(),
            ],
            vec!["1".into(), "1".into()],
            vec!["solution one".into(), "solution one alt".into()],
            vec!["1".into(), "1".into()],
            vec!["solution two".into(), "solution two alt".into()],
            vec!["2".into(), "2".into()],
        ]));

        let options = RunnerOptions {
            default_samples: 2,
            default_k: 2,
            adaptive_k: false,
            max_decomposition_depth: 1,
            min_words_for_decomposition: 3,
            human_red_flag_threshold: 5,
            human_resample_threshold: 5,
            human_low_margin_threshold: 1,
        };

        let runner = FlowRunner::new(config, Some(llm), options);
        let mut context = Context::new("Fix the bug", "code");
        let outcome = runner.execute(&mut context).await.unwrap();
        assert!(matches!(outcome, RunnerOutcome::Completed));

        let completed = context
            .steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Completed))
            .count();
        assert_eq!(completed, 2, "two subtasks solved");
    }
}
