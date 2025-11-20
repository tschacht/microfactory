use std::{collections::HashMap, fmt::Write as _, sync::Arc, time::Instant};

use anyhow::{Context as AnyhowContext, Result, anyhow};
use async_trait::async_trait;
use handlebars::Handlebars;
use regex::RegexBuilder;
use serde_json::json;
use tracing::{debug, info, warn};

use tokio::task::JoinSet;

use crate::{
    context::{
        AgentConfig, AgentKind, Context, DecompositionProposal, RedFlagIncident, StepStatus,
    },
    llm::LlmClient,
    red_flaggers::{RedFlagMatch, RedFlagPipeline},
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
    WinnerSelected { step_id: usize },
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
    red_flags: Arc<RedFlagPipeline>,
    handlebars: Arc<Handlebars<'static>>,
}

impl DecompositionTask {
    pub fn new(
        step_id: usize,
        prompt: String,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        red_flags: Arc<RedFlagPipeline>,
        handlebars: Arc<Handlebars<'static>>,
    ) -> Self {
        Self {
            step_id,
            prompt,
            agent,
            llm,
            red_flags,
            handlebars,
        }
    }
}

#[async_trait]
impl MicroTask for DecompositionTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let start = Instant::now();
        let samples = self.agent.samples.max(1);
        let rendered_prompt = render_prompt(
            &self.handlebars,
            &self.agent.prompt_template,
            &self.prompt,
            "decomposition",
        )?;
        ctx.mark_step_status(self.step_id, StepStatus::Running);
        let responses = SampleCollector::new(
            ctx,
            self.step_id,
            self.llm.clone(),
            self.red_flags.clone(),
            "decomposition",
        )
        .collect(rendered_prompt, samples, &self.agent.model)
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

        ctx.metrics
            .record_duration_ms(self.step_id, start.elapsed().as_millis());
        ctx.register_decomposition(self.step_id, proposals);
        if let Some(step) = ctx.step(self.step_id) {
            debug!(
                step_id = self.step_id,
                depth = step.depth,
                "Decomposition proposals ready"
            );
        }
        Ok(TaskResult::continue_with(TaskEffect::None))
    }
}

pub struct DecompositionVoteTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
    vote_k: usize,
    handlebars: Arc<Handlebars<'static>>,
}

impl DecompositionVoteTask {
    pub fn new(
        step_id: usize,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        vote_k: usize,
        handlebars: Arc<Handlebars<'static>>,
    ) -> Self {
        Self {
            step_id,
            agent,
            llm,
            vote_k,
            handlebars,
        }
    }
}

#[async_trait]
impl MicroTask for DecompositionVoteTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let start = Instant::now();
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
            &self.handlebars,
            &self.agent.prompt_template,
            &prompt_body,
            "decomposition_vote",
        )?;
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

        let k = self.vote_k.max(1);
        let winner_idx = first_to_ahead_by_k(&votes, k)
            .or_else(|| majority_vote(&votes))
            .unwrap_or(0)
            .min(proposals.len() - 1);
        let (winner_votes, runner_up_votes) = vote_counts(&votes, proposals.len(), winner_idx);
        ctx.metrics.record_vote(
            self.step_id,
            AgentKind::DecompositionDiscriminator,
            winner_votes,
            runner_up_votes,
        );
        ctx.metrics
            .record_duration_ms(self.step_id, start.elapsed().as_millis());
        let winner = proposals[winner_idx].clone();
        let mut new_steps = Vec::new();
        for subtask in winner.subtasks.iter() {
            let child_id = ctx.add_child_step(self.step_id, subtask.clone());
            new_steps.push(child_id);
        }

        debug!(
            step_id = self.step_id,
            winner_idx,
            winner_votes,
            runner_up_votes,
            vote_k = k,
            "Decomposition vote completed"
        );

        Ok(TaskResult::continue_with(TaskEffect::SpawnedSteps(
            new_steps,
        )))
    }
}

pub struct SolveTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
    red_flags: Arc<RedFlagPipeline>,
    handlebars: Arc<Handlebars<'static>>,
}

impl SolveTask {
    pub fn new(
        step_id: usize,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        red_flags: Arc<RedFlagPipeline>,
        handlebars: Arc<Handlebars<'static>>,
    ) -> Self {
        Self {
            step_id,
            agent,
            llm,
            red_flags,
            handlebars,
        }
    }
}

#[async_trait]
impl MicroTask for SolveTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let start = Instant::now();
        let step = ctx
            .step(self.step_id)
            .with_context(|| format!("Unknown step {}", self.step_id))?;
        let prompt = render_prompt(
            &self.handlebars,
            &self.agent.prompt_template,
            &step.description,
            "solve",
        )?;
        let samples = self.agent.samples.max(1);
        let responses = SampleCollector::new(
            ctx,
            self.step_id,
            self.llm.clone(),
            self.red_flags.clone(),
            "solve",
        )
        .collect(prompt, samples, &self.agent.model)
        .await?;
        if responses.is_empty() {
            return Err(anyhow!("Solver agent produced no candidates"));
        }
        ctx.metrics
            .record_duration_ms(self.step_id, start.elapsed().as_millis());
        ctx.register_solutions(self.step_id, responses);
        debug!(
            step_id = self.step_id,
            "Solver produced candidate solutions"
        );
        Ok(TaskResult::continue_with(TaskEffect::SolutionsReady {
            step_id: self.step_id,
        }))
    }
}

pub struct SolutionVoteTask {
    step_id: usize,
    agent: AgentConfig,
    llm: Arc<dyn LlmClient>,
    vote_k: usize,
    handlebars: Arc<Handlebars<'static>>,
}

impl SolutionVoteTask {
    pub fn new(
        step_id: usize,
        agent: AgentConfig,
        llm: Arc<dyn LlmClient>,
        vote_k: usize,
        handlebars: Arc<Handlebars<'static>>,
    ) -> Self {
        Self {
            step_id,
            agent,
            llm,
            vote_k,
            handlebars,
        }
    }
}

#[async_trait]
impl MicroTask for SolutionVoteTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let start = Instant::now();
        let solutions = ctx
            .take_solutions(self.step_id)
            .with_context(|| format!("No solutions queued for step {}", self.step_id))?;
        let prompt_body = enumerate_options(solutions.clone());
        let vote_prompt = render_prompt(
            &self.handlebars,
            &self.agent.prompt_template,
            &prompt_body,
            "solution_vote",
        )?;
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
        let k = self.vote_k.max(1);
        let winner_idx = first_to_ahead_by_k(&votes, k)
            .or_else(|| majority_vote(&votes))
            .unwrap_or(0)
            .min(solutions.len() - 1);
        let (winner_votes, runner_up_votes) = vote_counts(&votes, solutions.len(), winner_idx);
        ctx.metrics.record_vote(
            self.step_id,
            AgentKind::SolutionDiscriminator,
            winner_votes,
            runner_up_votes,
        );
        ctx.metrics
            .record_duration_ms(self.step_id, start.elapsed().as_millis());
        let winner = solutions[winner_idx].clone();
        ctx.mark_step_solution(self.step_id, winner);
        debug!(
            step_id = self.step_id,
            winner_idx,
            winner_votes,
            runner_up_votes,
            vote_k = k,
            "Solution vote completed"
        );
        Ok(TaskResult::continue_with(TaskEffect::WinnerSelected {
            step_id: self.step_id,
        }))
    }
}

pub struct ApplyVerifyTask {
    step_id: usize,
    applier: Option<String>,
    verifier: Option<String>,
}

impl ApplyVerifyTask {
    pub fn new(step_id: usize, applier: Option<String>, verifier: Option<String>) -> Self {
        Self {
            step_id,
            applier,
            verifier,
        }
    }
}

#[async_trait]
impl MicroTask for ApplyVerifyTask {
    async fn run(&self, ctx: &mut Context) -> Result<TaskResult> {
        let start = Instant::now();
        let step = ctx
            .step(self.step_id)
            .with_context(|| format!("Unknown step {}", self.step_id))?;

        let _solution = step
            .winning_solution
            .clone()
            .ok_or_else(|| anyhow!("No winning solution to apply for step {}", self.step_id))?;

        if ctx.dry_run {
            info!(step_id = self.step_id, "Dry run: skipping apply/verify");
            ctx.mark_step_status(self.step_id, StepStatus::Completed);
            return Ok(TaskResult::continue_with(TaskEffect::StepCompleted {
                step_id: self.step_id,
            }));
        }

        // Apply
        if let Some(applier_cmd) = &self.applier {
            if applier_cmd == "patch_file" {
                info!(
                    step_id = self.step_id,
                    "Applying solution via built-in patch_file (mock)"
                );
            } else if applier_cmd == "overwrite_file" {
                let solution = step.winning_solution.as_ref().unwrap();
                let files = extract_xml_files(solution);

                if !files.is_empty() {
                    let mut success = true;
                    for (path_str, content) in files {
                        match validate_target_path(&path_str) {
                            Ok(safe_path) => {
                                if let Some(parent) = safe_path.parent() {
                                    std::fs::create_dir_all(parent).ok();
                                }
                                match std::fs::write(&safe_path, content) {
                                    Ok(_) => {
                                        info!(
                                            step_id = self.step_id,
                                            path = %safe_path.display(),
                                            "Overwrote file (XML block)"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            step_id = self.step_id,
                                            path = %safe_path.display(),
                                            error = ?e,
                                            "Failed to overwrite file"
                                        );
                                        success = false;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    step_id = self.step_id,
                                    path = %path_str,
                                    error = ?e,
                                    "Target path failed safety validation"
                                );
                                success = false;
                            }
                        }
                    }
                    if !success {
                        ctx.mark_step_status(self.step_id, StepStatus::Failed);
                        return Ok(TaskResult::continue_with(TaskEffect::None));
                    }
                } else {
                    // Fallback to legacy single-file heuristic
                    let target_path = extract_target_path(&step.description);
                    if let Some(path_str) = target_path {
                        match validate_target_path(&path_str) {
                            Ok(safe_path) => {
                                let content =
                                    extract_code_content(step.winning_solution.as_ref().unwrap());
                                if let Some(parent) = safe_path.parent() {
                                    std::fs::create_dir_all(parent).ok();
                                }
                                match std::fs::write(&safe_path, content) {
                                    Ok(_) => {
                                        info!(step_id = self.step_id, path = %safe_path.display(), "Overwrote file (legacy heuristic)");
                                    }
                                    Err(e) => {
                                        warn!(step_id = self.step_id, path = %safe_path.display(), error = ?e, "Failed to overwrite file");
                                        ctx.mark_step_status(self.step_id, StepStatus::Failed);
                                        return Ok(TaskResult::continue_with(TaskEffect::None));
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(
                                    step_id = self.step_id,
                                    path = %path_str,
                                    error = ?e,
                                    "Target path failed safety validation"
                                );
                                ctx.mark_step_status(self.step_id, StepStatus::Failed);
                                return Ok(TaskResult::continue_with(TaskEffect::None));
                            }
                        }
                    } else {
                        warn!(
                            step_id = self.step_id,
                            description = %step.description,
                            "Could not determine target file path from description for overwrite_file"
                        );
                        ctx.mark_step_status(self.step_id, StepStatus::Failed);
                        return Ok(TaskResult::continue_with(TaskEffect::None));
                    }
                }
            } else {
                info!(
                    step_id = self.step_id,
                    command = applier_cmd,
                    "Running applier command"
                );
                // In a real implementation, we'd pipe the solution to the command
            }
        }

        // Verify
        let mut verified = true;
        if let Some(verifier_cmd) = &self.verifier {
            info!(
                step_id = self.step_id,
                command = verifier_cmd,
                "Running verification"
            );
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(verifier_cmd)
                .output()
            {
                Ok(output) => {
                    verified = output.status.success();
                    if !verified {
                        warn!(
                            step_id = self.step_id,
                            stderr = ?String::from_utf8_lossy(&output.stderr),
                            "Verification failed"
                        );
                    }
                }
                Err(e) => {
                    warn!(step_id = self.step_id, error = ?e, "Failed to execute verifier");
                    verified = false;
                }
            }
        }

        ctx.metrics
            .record_duration_ms(self.step_id, start.elapsed().as_millis());
        ctx.step_metrics_mut(self.step_id).verification_passed = Some(verified);

        if verified {
            ctx.mark_step_status(self.step_id, StepStatus::Completed);
            Ok(TaskResult::continue_with(TaskEffect::StepCompleted {
                step_id: self.step_id,
            }))
        } else {
            ctx.mark_step_status(self.step_id, StepStatus::Failed);
            Ok(TaskResult::continue_with(TaskEffect::None))
        }
    }
}

fn render_prompt(
    handlebars: &Handlebars,
    template: &str,
    body: &str,
    role: &str,
) -> Result<String> {
    let data = json!({
        "prompt": body,
        "task": body,
        "role": role,
    });

    handlebars
        .render_template(template, &data)
        .with_context(|| format!("Failed to render prompt template for role '{role}'"))
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
        if let Ok(value) = digits.parse::<usize>()
            && (1..=max_index).contains(&value)
        {
            return Some(value - 1);
        }
    }
    None
}

fn enumerate_options(options: Vec<String>) -> String {
    let mut body = String::new();
    for (idx, option) in options.iter().enumerate() {
        let _ = writeln!(&mut body, "Option {}:\n{option}\n", idx + 1);
    }
    body
}

fn vote_counts(votes: &[usize], candidate_count: usize, winner_idx: usize) -> (usize, usize) {
    if candidate_count == 0 {
        return (0, 0);
    }
    let mut counts = vec![0usize; candidate_count];
    for &vote in votes {
        if vote < candidate_count {
            counts[vote] += 1;
        }
    }
    let winner = counts.get(winner_idx).copied().unwrap_or(0);
    let mut runner_up = 0;
    for (idx, count) in counts.iter().enumerate() {
        if idx == winner_idx {
            continue;
        }
        if *count > runner_up {
            runner_up = *count;
        }
    }
    (winner, runner_up)
}

struct SampleCollector<'ctx> {
    ctx: &'ctx mut Context,
    step_id: usize,
    llm: Arc<dyn LlmClient>,
    pipeline: Arc<RedFlagPipeline>,
    stage: &'static str,
}

impl<'ctx> SampleCollector<'ctx> {
    fn new(
        ctx: &'ctx mut Context,
        step_id: usize,
        llm: Arc<dyn LlmClient>,
        pipeline: Arc<RedFlagPipeline>,
        stage: &'static str,
    ) -> Self {
        Self {
            ctx,
            step_id,
            llm,
            pipeline,
            stage,
        }
    }

    async fn collect(
        self,
        prompt: String,
        target_samples: usize,
        model: &str,
    ) -> Result<Vec<String>> {
        self.collect_inner(prompt, target_samples, model).await
    }

    async fn collect_inner(
        self,
        prompt: String,
        target_samples: usize,
        model: &str,
    ) -> Result<Vec<String>> {
        if target_samples == 0 {
            return Ok(Vec::new());
        }

        if self.pipeline.is_empty() {
            let responses = self
                .llm
                .sample_n(&prompt, target_samples, Some(model))
                .await?;
            self.ctx
                .metrics
                .record_samples(self.step_id, responses.len(), responses.len());
            debug!(
                step_id = self.step_id,
                stage = self.stage,
                collected = responses.len(),
                "Collected samples (no red flags)"
            );
            return Ok(responses);
        }

        let mut accepted = Vec::new();
        let mut attempts = 0usize;
        let max_attempts = target_samples.max(1) * 4;
        while accepted.len() < target_samples {
            attempts += 1;
            let remaining = target_samples - accepted.len();
            let batch = self.llm.sample_n(&prompt, remaining, Some(model)).await?;
            let batch_len = batch.len();
            let before = accepted.len();
            let mut flagged_this_round = 0usize;

            // Evaluate red flags in parallel
            let mut join_set = JoinSet::new();
            for raw in batch {
                let pipeline = self.pipeline.clone();
                let raw_owned = raw.clone();
                join_set.spawn(async move {
                    let matches = pipeline.evaluate(&raw_owned).await;
                    (raw_owned, matches)
                });
            }

            while let Some(result) = join_set.join_next().await {
                let (raw, matches) = result.context("Panic in red-flag evaluation task")?;
                if matches.is_empty() {
                    accepted.push(raw);
                } else {
                    flagged_this_round += 1;
                    let incidents = matches_to_incidents(matches, &raw);
                    if let Some(first) = incidents.first() {
                        warn!(
                            step_id = self.step_id,
                            stage = self.stage,
                            flagger = %first.flagger,
                            reason = %first.reason,
                            "Red-flagged sample discarded"
                        );
                    }
                    self.ctx.metrics.record_red_flags(self.step_id, incidents);
                }
            }

            let accepted_delta = accepted.len() - before;
            self.ctx
                .metrics
                .record_samples(self.step_id, batch_len, accepted_delta);
            if accepted.len() < target_samples {
                self.ctx.metrics.record_resample(self.step_id);
                if attempts >= max_attempts {
                    return Err(anyhow!(
                        "Exceeded red-flag resample budget for step {} during {}",
                        self.step_id,
                        self.stage
                    ));
                }
            }
            if flagged_this_round > 0 {
                warn!(
                    step_id = self.step_id,
                    stage = self.stage,
                    flagged = flagged_this_round,
                    accepted_delta,
                    "Flagged samples in batch"
                );
            }
        }

        debug!(
            step_id = self.step_id,
            stage = self.stage,
            accepted = accepted.len(),
            attempts,
            "Collected samples with guardrails"
        );
        Ok(accepted)
    }
}

fn matches_to_incidents(matches: Vec<RedFlagMatch>, sample: &str) -> Vec<RedFlagIncident> {
    let preview = preview_sample(sample);
    matches
        .into_iter()
        .map(|m| RedFlagIncident {
            flagger: m.flagger,
            reason: m.reason,
            sample_preview: preview.clone(),
        })
        .collect()
}

fn preview_sample(text: &str) -> String {
    let trimmed = text.trim();
    const LIMIT: usize = 160;
    if trimmed.chars().count() > LIMIT {
        let preview: String = trimmed.chars().take(LIMIT).collect();
        format!("{preview}…")
    } else {
        trimmed.to_string()
    }
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

fn validate_target_path(raw: &str) -> Result<std::path::PathBuf> {
    let path = std::path::Path::new(raw);

    // 1. Must be relative
    if path.is_absolute() {
        return Err(anyhow!("Absolute paths are forbidden: {raw}"));
    }

    // 2. Must not contain traversal (..)
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(anyhow!("Path traversal (..) is forbidden: {raw}"));
            }
            std::path::Component::Normal(os_str) => {
                if os_str == ".git" {
                    return Err(anyhow!("Modifying .git directory is forbidden: {raw}"));
                }
            }
            _ => {}
        }
    }

    // 3. Check for .git in path string too (to catch hidden .git inside filenames if OS allows)
    // Just standard component check above covers `.git` folder, but let's be safe.
    if raw.contains("/.git/") || raw.starts_with(".git/") || raw == ".git" {
        return Err(anyhow!("Modifying .git directory is forbidden: {raw}"));
    }

    Ok(path.to_path_buf())
}

fn extract_target_path(description: &str) -> Option<String> {
    // Heuristic: find first token that looks like a file path
    for token in description.split_whitespace() {
        let clean = token.trim_matches(|c| {
            c == '(' || c == ')' || c == ':' || c == ',' || c == '\'' || c == '"'
        });
        if (clean.contains('.') || clean.contains('/')) && !clean.ends_with('.') {
            return Some(clean.to_string());
        }
    }
    None
}

fn extract_xml_files(raw: &str) -> Vec<(String, String)> {
    let re = RegexBuilder::new(r#"<file\s+path="([^"]+)">\s*(.*?)\s*</file>"#)
        .dot_matches_new_line(true)
        .build()
        .expect("valid regex");

    re.captures_iter(raw)
        .map(|cap| (cap[1].to_string(), cap[2].trim().to_string()))
        .collect()
}

fn extract_code_content(raw: &str) -> String {
    if let Some(start) = raw.find("```") {
        let rest = &raw[start + 3..];
        if let Some(end) = rest.find("```") {
            let code_block = &rest[..end];
            // skip language identifier line if present
            if let Some(newline) = code_block.find('\n') {
                return code_block[newline + 1..].to_string();
            }
            return code_block.to_string();
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::RedFlaggerConfig, context::Context, red_flaggers::RedFlagPipeline};
    use async_trait::async_trait;
    use serde_yaml::Value;
    use std::{collections::BTreeMap, sync::Mutex};

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

    #[test]
    fn extracts_target_path_from_description() {
        assert_eq!(
            extract_target_path("Create file src/main.rs"),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            extract_target_path("Update config.yaml with..."),
            Some("config.yaml".to_string())
        );
        assert_eq!(extract_target_path("Refactor the login logic"), None);
        assert_eq!(
            extract_target_path("Check (src/lib.rs)"),
            Some("src/lib.rs".to_string())
        );
    }

    #[test]
    fn extracts_code_content_from_markdown() {
        let raw = "Here is the code:\n```rust\nfn main() {}\n```\nEnjoy.";
        assert_eq!(extract_code_content(raw), "fn main() {}\n");

        let raw_no_lang = "```\nplain text\n```";
        assert_eq!(extract_code_content(raw_no_lang), "plain text\n");

        let raw_plain = "Just text";
        assert_eq!(extract_code_content(raw_plain), "Just text");
    }

    #[test]
    fn extracts_multiple_xml_files() {
        let raw = r#"
Here is the plan:

<file path="src/main.rs">
fn main() {
    println!("Hello");
}
</file>

And the lib:
<file path="src/lib.rs">pub fn add(a: i32, b: i32) -> i32 { a + b }</file>
        "#;

        let files = extract_xml_files(raw);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "src/main.rs");
        assert!(files[0].1.contains("println!"));
        assert_eq!(files[1].0, "src/lib.rs");
        assert_eq!(files[1].1, "pub fn add(a: i32, b: i32) -> i32 { a + b }");
    }

    #[test]
    fn validation_rejects_unsafe_paths() {
        assert!(validate_target_path("/etc/passwd").is_err());
        assert!(validate_target_path("../outside").is_err());
        assert!(validate_target_path("src/../../oops").is_err());
        assert!(validate_target_path(".git/config").is_err());
        assert!(validate_target_path("src/.git/info").is_err());

        assert!(validate_target_path("src/main.rs").is_ok());
        assert!(validate_target_path("README.md").is_ok());
        assert!(validate_target_path("nested/deep/file.rs").is_ok());
    }

    #[tokio::test]
    async fn red_flags_trigger_resample() {
        struct ScriptedLlm {
            batches: Mutex<Vec<Vec<String>>>,
        }

        impl ScriptedLlm {
            fn new(batches: Vec<Vec<String>>) -> Self {
                Self {
                    batches: Mutex::new(batches),
                }
            }
        }

        #[async_trait]
        impl LlmClient for ScriptedLlm {
            async fn sample(&self, _: &str, _: Option<&str>) -> Result<String> {
                let mut values = self.sample_n("", 1, None).await?;
                Ok(values.remove(0))
            }

            async fn sample_n(&self, _: &str, n: usize, _: Option<&str>) -> Result<Vec<String>> {
                let mut guard = self.batches.lock().unwrap();
                let batch = guard.remove(0);
                assert_eq!(batch.len(), n);
                Ok(batch)
            }
        }

        let configs = vec![RedFlaggerConfig {
            kind: "length".into(),
            params: BTreeMap::from([(String::from("max_tokens"), Value::from(2))]),
        }];
        let pipeline = Arc::new(RedFlagPipeline::from_configs(&configs, None).unwrap());
        let llm: Arc<dyn LlmClient> = Arc::new(ScriptedLlm::new(vec![
            vec!["one two three".into()],
            vec!["one two".into()],
        ]));
        let mut ctx = Context::new("demo", "code");
        let root_id = ctx.ensure_root();
        let responses = SampleCollector::new(&mut ctx, root_id, llm, pipeline, "test")
            .collect("prompt".to_string(), 1, "model")
            .await
            .expect("collected sample");

        assert_eq!(responses.len(), 1);
        assert_eq!(ctx.metrics.red_flag_hits, 1);
        assert!(ctx.metrics.resample_count >= 1);
    }
}
