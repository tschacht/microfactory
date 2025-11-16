use std::{collections::HashMap, fmt::Write as _, sync::Arc};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use async_trait::async_trait;

use crate::{
    context::{AgentConfig, Context, DecompositionProposal, StepStatus},
    llm::LlmClient,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextAction {
    Continue,
    WaitForInput,
    GoTo(usize),
    End,
    Error(&'static str),
}

#[derive(Debug, Clone)]
pub enum TaskEffect {
    None,
    SpawnedSteps(Vec<usize>),
    SolutionsReady { step_id: usize },
    StepCompleted { step_id: usize },
}

#[derive(Debug, Clone)]
pub struct TaskResult {
    pub action: NextAction,
    pub effect: TaskEffect,
}

impl TaskResult {
    pub fn continue_with(effect: TaskEffect) -> Self {
        Self {
            action: NextAction::Continue,
            effect,
        }
    }
}

impl Default for TaskResult {
    fn default() -> Self {
        TaskResult::continue_with(TaskEffect::None)
    }
}

#[async_trait]
pub trait MicroTask: Send + Sync {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult>;
}

pub struct DecompositionTask {
    step_id: usize,
    prompt: String,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
}

impl DecompositionTask {
    pub fn new(
        step_id: usize,
        prompt: String,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
    ) -> Self {
        Self {
            step_id,
            prompt,
            agent,
            llm,
        }
    }
}

#[async_trait]
impl MicroTask for DecompositionTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let samples = self.agent.samples.max(1);
        let rendered_prompt =
            render_prompt(&self.agent.prompt_template, &self.prompt, "decomposition");
        ctx.mark_step_status(self.step_id, StepStatus::Running);
        let responses = self
            .llm
            .sample_n(&rendered_prompt, samples, Some(self.agent.model.as_str()))
            .await?;

        let proposals = responses
            .into_iter()
            .enumerate()
            .map(|(idx, raw)| {
                let mut subtasks = parse_subtasks(&raw);
                if subtasks.is_empty() {
                    subtasks.push(self.prompt.clone());
                }
                DecompositionProposal::new(idx, raw, subtasks)
            })
            .collect::<Vec<_>>();

        if proposals.is_empty() {
            return Err(anyhow!("LLM returned no decomposition proposals"));
        }

        ctx.register_decomposition(self.step_id, proposals);
        Ok(TaskResult::continue_with(TaskEffect::None))
    }
}

pub struct DecompositionVoteTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
    default_k: usize,
}

impl DecompositionVoteTask {
    pub fn new(
        step_id: usize,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        default_k: usize,
    ) -> Self {
        Self {
            step_id,
            agent,
            llm,
            default_k,
        }
    }
}

#[async_trait]
impl MicroTask for DecompositionVoteTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let proposals = ctx
            .take_decomposition(self.step_id)
            .with_context(|| format!("No proposals available for step {}", self.step_id))?;

        let prompt_body = enumerate_options(
            proposals
                .iter()
                .map(|p| p.subtasks.join("\n"))
                .collect::<Vec<_>>(),
        );
        let rendered_prompt = render_prompt(
            &self.agent.prompt_template,
            &prompt_body,
            "decomposition_vote",
        );
        let samples = self.agent.samples.max(1);
        let raw_votes = self
            .llm
            .sample_n(&rendered_prompt, samples, Some(self.agent.model.as_str()))
            .await?;
        let mut votes = Vec::new();
        for raw in raw_votes {
            if let Some(choice) = parse_vote_response(&raw, proposals.len()) {
                votes.push(choice);
            }
        }

        let k = self.agent.k.unwrap_or(self.default_k).max(1);
        let winner_idx = first_to_ahead_by_k(&votes, k)
            .or_else(|| majority_vote(&votes))
            .unwrap_or(0)
            .min(proposals.len() - 1);
        let winner = proposals[winner_idx].clone();
        let mut new_steps = Vec::new();
        for subtask in winner.subtasks.iter() {
            let child_id = ctx.add_child_step(self.step_id, subtask.clone());
            new_steps.push(child_id);
        }

        Ok(TaskResult::continue_with(TaskEffect::SpawnedSteps(
            new_steps,
        )))
    }
}

pub struct SolveTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
}

impl SolveTask {
    pub fn new(step_id: usize, agent: AgentConfig, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            step_id,
            agent,
            llm,
        }
    }
}

#[async_trait]
impl MicroTask for SolveTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let step = ctx
            .step(self.step_id)
            .with_context(|| format!("Unknown step {}", self.step_id))?;
        let prompt = render_prompt(&self.agent.prompt_template, &step.description, "solve");
        let samples = self.agent.samples.max(1);
        let responses = self
            .llm
            .sample_n(&prompt, samples, Some(self.agent.model.as_str()))
            .await?;
        if responses.is_empty() {
            return Err(anyhow!("Solver agent produced no candidates"));
        }
        ctx.register_solutions(self.step_id, responses);
        Ok(TaskResult::continue_with(TaskEffect::SolutionsReady {
            step_id: self.step_id,
        }))
    }
}

pub struct SolutionVoteTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
    default_k: usize,
}

impl SolutionVoteTask {
    pub fn new(
        step_id: usize,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        default_k: usize,
    ) -> Self {
        Self {
            step_id,
            agent,
            llm,
            default_k,
        }
    }
}

#[async_trait]
impl MicroTask for SolutionVoteTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let solutions = ctx
            .take_solutions(self.step_id)
            .with_context(|| format!("No solutions queued for step {}", self.step_id))?;
        let prompt_body = enumerate_options(solutions.clone());
        let vote_prompt = render_prompt(&self.agent.prompt_template, &prompt_body, "solution_vote");
        let samples = self.agent.samples.max(1);
        let raw_votes = self
            .llm
            .sample_n(&vote_prompt, samples, Some(self.agent.model.as_str()))
            .await?;
        let mut votes = Vec::new();
        for raw in raw_votes {
            if let Some(choice) = parse_vote_response(&raw, solutions.len()) {
                votes.push(choice);
            }
        }
        let k = self.agent.k.unwrap_or(self.default_k).max(1);
        let winner_idx = first_to_ahead_by_k(&votes, k)
            .or_else(|| majority_vote(&votes))
            .unwrap_or(0)
            .min(solutions.len() - 1);
        let winner = solutions[winner_idx].clone();
        ctx.mark_step_solution(self.step_id, winner);
        Ok(TaskResult::continue_with(TaskEffect::StepCompleted {
            step_id: self.step_id,
        }))
    }
}

fn render_prompt(template: &str, body: &str, role: &str) -> String {
    let replaced = template
        .replace("{{prompt}}", body)
        .replace("{{task}}", body);
    if replaced == template {
        format!("{template}\n\n[{role}] TASK INPUT:\n{body}")
    } else {
        replaced
    }
}

fn parse_subtasks(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let trimmed = trimmed
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim_start_matches('\u{2022}')
                .trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect()
}

fn parse_vote_response(raw: &str, max_index: usize) -> Option<usize> {
    if max_index == 0 {
        return None;
    }
    for token in raw.split_whitespace() {
        let digits: String = token.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        if let Ok(value) = digits.parse::<usize>() {
            if (1..=max_index).contains(&value) {
                return Some(value - 1);
            }
        }
    }
    None
}

fn enumerate_options(options: Vec<String>) -> String {
    let mut body = String::new();
    for (idx, option) in options.iter().enumerate() {
        let _ = writeln!(&mut body, "Option {}:\n{}\n", idx + 1, option);
    }
    body
}

fn first_to_ahead_by_k(votes: &[usize], k: usize) -> Option<usize> {
    if votes.is_empty() {
        return None;
    }
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for &vote in votes {
        let entry = counts.entry(vote).or_insert(0);
        *entry += 1;
        let mut ordered = counts.iter().collect::<Vec<_>>();
        ordered.sort_by(|a, b| b.1.cmp(a.1));
        if ordered.len() == 1 {
            if *ordered[0].1 >= k {
                return Some(*ordered[0].0);
            }
            continue;
        }
        let leader = ordered[0];
        let runner_up = ordered[1];
        if *leader.1 >= runner_up.1 + k {
            return Some(*leader.0);
        }
    }
    None
}

fn majority_vote(votes: &[usize]) -> Option<usize> {
    if votes.is_empty() {
        return None;
    }
    let mut counts: HashMap<usize, usize> = HashMap::new();
    let mut leader = votes[0];
    let mut leader_count = 0;
    for &vote in votes {
        let entry = counts.entry(vote).or_insert(0);
        *entry += 1;
        if *entry > leader_count {
            leader = vote;
            leader_count = *entry;
        }
    }
    Some(leader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_subtasks_from_bullets() {
        let raw = "- step one\n* step two";
        let subtasks = parse_subtasks(raw);
        assert_eq!(subtasks, vec!["step one", "step two"]);
    }

    #[test]
    fn vote_parser_handles_digits() {
        assert_eq!(parse_vote_response("Option 2", 3), Some(1));
        assert_eq!(parse_vote_response("choice #1", 1), Some(0));
        assert_eq!(parse_vote_response("invalid", 2), None);
    }

    #[test]
    fn ahead_by_k_requires_margin() {
        let votes = vec![0, 0, 1, 0];
        assert_eq!(first_to_ahead_by_k(&votes, 2), Some(0));
    }

    #[test]
    fn majority_vote_falls_back() {
        let votes = vec![1, 2, 2, 1, 2];
        assert_eq!(majority_vote(&votes), Some(2));
    }
}
