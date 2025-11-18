use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};

/// Runtime context shared across microtasks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Context {
    pub session_id: String,
    pub prompt: String,
    pub domain: String,
    pub steps: Vec<WorkflowStep>,
    pub current_step: usize,
    pub metrics: WorkflowMetrics,
    pub domain_data: HashMap<String, String>,
    pub dry_run: bool,
    pub next_step_id: usize,
    pub root_step_id: Option<usize>,
    pub pending_decompositions: HashMap<usize, Vec<DecompositionProposal>>,
    pub pending_solutions: HashMap<usize, Vec<String>>,
    pub work_queue: VecDeque<WorkItem>,
    pub wait_state: Option<WaitState>,
}

impl Context {
    pub fn new(prompt: impl Into<String>, domain: impl Into<String>) -> Self {
        let prompt_str = prompt.into();
        let mut ctx = Self {
            prompt: prompt_str.clone(),
            domain: domain.into(),
            ..Self::default()
        };
        let root = ctx.create_step(prompt_str, None, 0);
        ctx.root_step_id = Some(root);
        ctx.current_step = root;
        ctx.enqueue_work(WorkItem::Decomposition { step_id: root });
        ctx
    }

    pub fn ensure_root(&mut self) -> usize {
        if let Some(id) = self.root_step_id {
            id
        } else {
            let prompt = if self.prompt.is_empty() {
                "root task".to_string()
            } else {
                self.prompt.clone()
            };
            let id = self.create_step(prompt, None, 0);
            self.root_step_id = Some(id);
            self.current_step = id;
            id
        }
    }

    pub fn add_child_step(&mut self, parent: usize, description: impl Into<String>) -> usize {
        let depth = self
            .steps
            .iter()
            .find(|step| step.id == parent)
            .map(|step| step.depth + 1)
            .unwrap_or(0);
        let id = self.create_step(description.into(), Some(parent), depth);
        if let Some(parent_step) = self.steps.iter_mut().find(|step| step.id == parent) {
            parent_step.children.push(id);
        }
        id
    }

    pub fn step(&self, step_id: usize) -> Option<&WorkflowStep> {
        self.steps.iter().find(|step| step.id == step_id)
    }

    pub fn step_mut(&mut self, step_id: usize) -> Option<&mut WorkflowStep> {
        self.steps.iter_mut().find(|step| step.id == step_id)
    }

    pub fn step_metrics_mut(&mut self, step_id: usize) -> &mut StepMetrics {
        self.metrics.step_metrics_mut(step_id)
    }

    pub fn metrics(&self) -> &WorkflowMetrics {
        &self.metrics
    }

    pub fn metrics_mut(&mut self) -> &mut WorkflowMetrics {
        &mut self.metrics
    }

    pub fn register_decomposition(
        &mut self,
        step_id: usize,
        proposals: Vec<DecompositionProposal>,
    ) {
        self.pending_decompositions.insert(step_id, proposals);
        self.metrics.decomposition_runs += 1;
    }

    pub fn take_decomposition(&mut self, step_id: usize) -> Option<Vec<DecompositionProposal>> {
        self.pending_decompositions.remove(&step_id)
    }

    pub fn register_solutions(&mut self, step_id: usize, candidates: Vec<String>) {
        if let Some(step) = self.step_mut(step_id) {
            step.candidate_solutions = candidates.clone();
        }
        self.pending_solutions.insert(step_id, candidates);
        self.metrics.solve_runs += 1;
    }

    pub fn take_solutions(&mut self, step_id: usize) -> Option<Vec<String>> {
        self.pending_solutions.remove(&step_id)
    }

    pub fn mark_step_solution(&mut self, step_id: usize, winner: String) {
        if let Some(step) = self.step_mut(step_id) {
            step.winning_solution = Some(winner);
            step.status = StepStatus::Completed;
        }
    }

    pub fn mark_step_status(&mut self, step_id: usize, status: StepStatus) {
        if let Some(step) = self.step_mut(step_id) {
            step.status = status;
        }
    }

    pub fn root_step_id(&self) -> Option<usize> {
        self.root_step_id
    }

    pub fn dequeue_work(&mut self) -> Option<WorkItem> {
        self.work_queue.pop_front()
    }

    pub fn enqueue_work(&mut self, item: WorkItem) {
        self.work_queue.push_back(item);
    }

    pub fn enqueue_work_front(&mut self, item: WorkItem) {
        self.work_queue.push_front(item);
    }

    pub fn has_pending_work(&self) -> bool {
        !self.work_queue.is_empty()
    }

    pub fn clear_wait_state(&mut self) {
        if let Some(wait) = self.wait_state.take()
            && let Some(step) = self.step_mut(wait.step_id)
            && matches!(step.status, StepStatus::WaitingOnInput)
        {
            step.status = StepStatus::Pending;
        }
    }

    pub fn set_wait_state(
        &mut self,
        step_id: usize,
        trigger: impl Into<String>,
        details: impl Into<String>,
    ) {
        self.wait_state = Some(WaitState {
            step_id,
            trigger: trigger.into(),
            details: details.into(),
        });
        self.mark_step_status(step_id, StepStatus::WaitingOnInput);
    }

    fn create_step(&mut self, description: String, parent: Option<usize>, depth: usize) -> usize {
        let id = self.next_step_id;
        self.next_step_id += 1;
        self.steps
            .push(WorkflowStep::new(id, description, parent, depth));
        id
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: usize,
    pub description: String,
    pub parent: Option<usize>,
    pub depth: usize,
    pub status: StepStatus,
    pub children: Vec<usize>,
    pub candidate_solutions: Vec<String>,
    pub winning_solution: Option<String>,
}

impl WorkflowStep {
    fn new(id: usize, description: String, parent: Option<usize>, depth: usize) -> Self {
        Self {
            id,
            description,
            parent,
            depth,
            status: StepStatus::Pending,
            children: Vec::new(),
            candidate_solutions: Vec::new(),
            winning_solution: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    Running,
    WaitingOnInput,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecompositionProposal {
    pub id: usize,
    pub raw: String,
    pub subtasks: Vec<String>,
}

impl DecompositionProposal {
    pub fn new(id: usize, raw: String, subtasks: Vec<String>) -> Self {
        Self { id, raw, subtasks }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WorkflowMetrics {
    pub sample_count: usize,
    pub resample_count: usize,
    pub vote_attempts: usize,
    pub decomposition_runs: usize,
    pub solve_runs: usize,
    pub red_flag_hits: usize,
    pub per_step: HashMap<usize, StepMetrics>,
    pub vote_history: HashMap<AgentKind, VoteStats>,
}

impl WorkflowMetrics {
    pub fn step_metrics_mut(&mut self, step_id: usize) -> &mut StepMetrics {
        self.per_step.entry(step_id).or_default()
    }

    pub fn step_metrics(&self, step_id: usize) -> Option<&StepMetrics> {
        self.per_step.get(&step_id)
    }

    pub fn record_samples(&mut self, step_id: usize, requested: usize, retained: usize) {
        self.sample_count += requested;
        let metrics = self.step_metrics_mut(step_id);
        metrics.samples_requested += requested;
        metrics.samples_retained += retained;
    }

    pub fn record_resample(&mut self, step_id: usize) {
        self.resample_count += 1;
        self.step_metrics_mut(step_id).resamples += 1;
    }

    pub fn record_red_flags(
        &mut self,
        step_id: usize,
        incidents: impl IntoIterator<Item = RedFlagIncident>,
    ) {
        let mut added = 0usize;
        {
            let metrics = self.step_metrics_mut(step_id);
            for incident in incidents {
                metrics.red_flags.push(incident);
                added += 1;
            }
        }
        self.red_flag_hits += added;
    }

    pub fn record_vote(
        &mut self,
        step_id: usize,
        agent_kind: AgentKind,
        winner_count: usize,
        runner_up_count: usize,
    ) {
        self.vote_attempts += 1;
        let margin = winner_count.saturating_sub(runner_up_count).max(1);
        self.step_metrics_mut(step_id).vote_margin = Some(margin);
        let stats = self.vote_history.entry(agent_kind).or_default();
        stats.total_votes += 1;
        if stats.recent_margins.len() >= 8 {
            stats.recent_margins.pop_front();
        }
        stats.recent_margins.push_back(margin);
    }

    pub fn record_duration_ms(&mut self, step_id: usize, duration_ms: u128) {
        let metrics = self.step_metrics_mut(step_id);
        let accumulated = metrics.duration_ms.unwrap_or(0) + duration_ms;
        metrics.duration_ms = Some(accumulated);
    }

    pub fn vote_stats(&self, agent_kind: AgentKind) -> Option<&VoteStats> {
        self.vote_history.get(&agent_kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    Decomposition,
    DecompositionDiscriminator,
    Solver,
    SolutionDiscriminator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub kind: AgentKind,
    pub prompt_template: String,
    pub model: String,
    pub samples: usize,
    pub k: Option<usize>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct StepMetrics {
    pub samples_requested: usize,
    pub samples_retained: usize,
    pub resamples: usize,
    pub red_flags: Vec<RedFlagIncident>,
    pub vote_margin: Option<usize>,
    pub duration_ms: Option<u128>,
    pub verification_passed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedFlagIncident {
    pub flagger: String,
    pub reason: String,
    pub sample_preview: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct VoteStats {
    pub recent_margins: VecDeque<usize>,
    pub total_votes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkItem {
    Decomposition { step_id: usize },
    DecompositionVote { step_id: usize },
    Solve { step_id: usize },
    SolutionVote { step_id: usize },
    ApplyVerify { step_id: usize },
}

impl WorkItem {
    pub fn step_id(&self) -> usize {
        match *self {
            WorkItem::Decomposition { step_id }
            | WorkItem::DecompositionVote { step_id }
            | WorkItem::Solve { step_id }
            | WorkItem::SolutionVote { step_id }
            | WorkItem::ApplyVerify { step_id } => step_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitState {
    pub step_id: usize,
    pub trigger: String,
    pub details: String,
}
