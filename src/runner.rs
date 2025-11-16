use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use petgraph::graph::Graph;
use tracing::info;

use crate::{
    config::{AgentDefinition, DomainConfig, MicrofactoryConfig},
    context::{AgentConfig, AgentKind, Context, StepStatus},
    llm::LlmClient,
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

    pub async fn execute(&self, context: &mut Context) -> Result<()> {
        let llm = self
            .llm
            .clone()
            .ok_or_else(|| anyhow!("LLM client required for execution"))?;

        let domain_cfg = self
            .config
            .domain(&context.domain)
            .with_context(|| format!("Unknown domain: {}", context.domain))?;
        let agent_configs = self.agent_configs(domain_cfg);
        let root_step = context.ensure_root();

        let mut graph: Graph<TaskNodeKind, ()> = Graph::new();
        let mut queue = VecDeque::new();
        let root_idx = graph.add_node(TaskNodeKind::Decomposition { step_id: root_step });
        queue.push_back(root_idx);

        while let Some(idx) = queue.pop_front() {
            let node_kind = *graph
                .node_weight(idx)
                .expect("Workflow node missing during traversal");
            match node_kind {
                TaskNodeKind::Decomposition { step_id } => {
                    let step_desc = context
                        .step(step_id)
                        .map(|s| s.description.clone())
                        .unwrap_or_else(|| context.prompt.clone());
                    let agent = agent_configs
                        .get(&AgentKind::Decomposition)
                        .expect("missing decomposition agent")
                        .clone();
                    let task = DecompositionTask::new(step_id, step_desc, agent, llm.clone());
                    let result = task.run(context).await?;
                    Self::ensure_continue(result.action)?;
                    let vote_idx = graph.add_node(TaskNodeKind::DecompositionVote { step_id });
                    graph.add_edge(idx, vote_idx, ());
                    queue.push_back(vote_idx);
                }
                TaskNodeKind::DecompositionVote { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::DecompositionDiscriminator)
                        .expect("missing decomposition discriminator")
                        .clone();
                    let task = DecompositionVoteTask::new(
                        step_id,
                        agent,
                        llm.clone(),
                        self.options.default_k,
                    );
                    let result = task.run(context).await?;
                    Self::ensure_continue(result.action)?;
                    if let TaskEffect::SpawnedSteps(children) = result.effect {
                        if children.is_empty() {
                            let solve_idx = graph.add_node(TaskNodeKind::Solve { step_id });
                            graph.add_edge(idx, solve_idx, ());
                            queue.push_back(solve_idx);
                        } else {
                            for child in children {
                                let kind = if self.should_recurse(context, child) {
                                    TaskNodeKind::Decomposition { step_id: child }
                                } else {
                                    TaskNodeKind::Solve { step_id: child }
                                };
                                let child_idx = graph.add_node(kind);
                                graph.add_edge(idx, child_idx, ());
                                queue.push_back(child_idx);
                            }
                        }
                    }
                }
                TaskNodeKind::Solve { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::Solver)
                        .expect("missing solver agent")
                        .clone();
                    let task = SolveTask::new(step_id, agent, llm.clone());
                    let result = task.run(context).await?;
                    Self::ensure_continue(result.action)?;
                    if matches!(result.effect, TaskEffect::SolutionsReady { .. }) {
                        let vote_idx = graph.add_node(TaskNodeKind::SolutionVote { step_id });
                        graph.add_edge(idx, vote_idx, ());
                        queue.push_back(vote_idx);
                    }
                }
                TaskNodeKind::SolutionVote { step_id } => {
                    let agent = agent_configs
                        .get(&AgentKind::SolutionDiscriminator)
                        .expect("missing solution discriminator")
                        .clone();
                    let task =
                        SolutionVoteTask::new(step_id, agent, llm.clone(), self.options.default_k);
                    let result = task.run(context).await?;
                    Self::ensure_continue(result.action)?;
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
        Ok(())
    }

    pub fn status(&self, session_id: Option<&str>) -> Result<()> {
        if let Some(id) = session_id {
            info!(session_id = id, "Status inspection placeholder");
        } else {
            info!("Listing recent sessions (placeholder)");
        }
        Ok(())
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

    fn ensure_continue(action: NextAction) -> Result<()> {
        match action {
            NextAction::Continue => Ok(()),
            NextAction::End => Ok(()),
            NextAction::WaitForInput => Err(anyhow!("WaitForInput not supported in Phase 3")),
            NextAction::GoTo(_) => Err(anyhow!("GoTo transitions not implemented yet")),
            NextAction::Error(msg) => Err(anyhow!("Task reported error: {msg}")),
        }
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

#[derive(Debug, Clone, Copy)]
enum TaskNodeKind {
    Decomposition { step_id: usize },
    DecompositionVote { step_id: usize },
    Solve { step_id: usize },
    SolutionVote { step_id: usize },
}

#[derive(Debug, Clone)]
pub struct RunnerOptions {
    pub default_samples: usize,
    pub default_k: usize,
    pub adaptive_k: bool,
    pub max_decomposition_depth: usize,
    pub min_words_for_decomposition: usize,
}

impl RunnerOptions {
    pub fn from_cli(samples: usize, k: usize, adaptive_k: bool) -> Self {
        Self {
            default_samples: samples.max(1),
            default_k: k.max(1),
            adaptive_k,
            max_decomposition_depth: 2,
            min_words_for_decomposition: 8,
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
        };

        let runner = FlowRunner::new(config, Some(llm), options);
        let mut context = Context::new("Fix the bug", "code");
        runner.execute(&mut context).await.unwrap();

        let completed = context
            .steps
            .iter()
            .filter(|step| matches!(step.status, StepStatus::Completed))
            .count();
        assert_eq!(completed, 2, "two subtasks solved");
    }
}
