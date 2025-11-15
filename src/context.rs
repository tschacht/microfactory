use std::collections::HashMap;

/// Runtime context shared across microtasks.
#[derive(Debug, Default)]
pub struct Context {
    pub session_id: String,
    pub prompt: String,
    pub domain: String,
    pub steps: Vec<WorkflowStep>,
    pub current_step: usize,
    pub metrics: WorkflowMetrics,
    pub domain_data: HashMap<String, String>,
    pub dry_run: bool,
}

impl Context {
    pub fn new(prompt: impl Into<String>, domain: impl Into<String>) -> Self {
        Self {
            session_id: String::new(),
            prompt: prompt.into(),
            domain: domain.into(),
            steps: Vec::new(),
            current_step: 0,
            metrics: WorkflowMetrics::default(),
            domain_data: HashMap::new(),
            dry_run: false,
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct WorkflowStep {
    pub description: String,
}

#[derive(Debug, Default)]
pub struct WorkflowMetrics {
    pub sample_count: usize,
    pub resample_count: usize,
    pub vote_attempts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Decomposition,
    DecompositionDiscriminator,
    Solver,
    SolutionDiscriminator,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub kind: AgentKind,
    pub prompt_template: String,
    pub model: String,
    pub samples: usize,
    pub k: Option<usize>,
}
