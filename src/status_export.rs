use serde::{Deserialize, Serialize};

use crate::{
    adapters::persistence::{SessionMetadata, SessionRecord, SessionSummary},
    context::{Context, WaitState, WorkflowMetrics},
};

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionListExport {
    pub sessions: Vec<SessionSummaryExport>,
}

impl SessionListExport {
    pub fn from_summaries(summaries: Vec<SessionSummary>) -> Self {
        Self {
            sessions: summaries
                .into_iter()
                .map(SessionSummaryExport::from)
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionSummaryExport {
    pub session_id: String,
    pub status: String,
    pub prompt: String,
    pub domain: String,
    pub updated_at: i64,
}

impl From<SessionSummary> for SessionSummaryExport {
    fn from(value: SessionSummary) -> Self {
        Self {
            session_id: value.session_id,
            status: value.status.as_str().to_string(),
            prompt: value.prompt,
            domain: value.domain,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionDetailExport {
    pub session_id: String,
    pub status: String,
    pub prompt: String,
    pub domain: String,
    pub updated_at: i64,
    pub wait_state: Option<WaitState>,
    pub pending_decompositions: Option<Vec<crate::context::DecompositionProposal>>,
    pub pending_solutions: Option<Vec<String>>,
    pub metadata: SessionMetadata,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub steps: Vec<crate::context::WorkflowStep>,
    pub metrics: WorkflowMetrics,
}

impl SessionDetailExport {
    pub fn from_record(record: &SessionRecord) -> Self {
        let context = &record.envelope.context;

        let mut pending_proposals = None;
        let mut pending_solutions = None;
        if let Some(wait) = &context.wait_state {
            if let Some(props) = context.pending_decompositions.get(&wait.step_id) {
                pending_proposals = Some(props.clone());
            }
            if let Some(sols) = context.pending_solutions.get(&wait.step_id) {
                pending_solutions = Some(sols.clone());
            }
        }

        Self {
            session_id: context.session_id.clone(),
            status: record.status.as_str().to_string(),
            prompt: context.prompt.clone(),
            domain: context.domain.clone(),
            updated_at: record.updated_at,
            wait_state: context.wait_state.clone(),
            pending_decompositions: pending_proposals,
            pending_solutions,
            metadata: record.envelope.metadata.clone(),
            completed_steps: count_completed_steps(context),
            total_steps: context.steps.len(),
            steps: context.steps.clone(),
            metrics: context.metrics.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PendingProposals {
    pub step_id: usize,
    pub proposals: Vec<crate::context::DecompositionProposal>,
}

pub fn count_completed_steps(ctx: &Context) -> usize {
    ctx.steps
        .iter()
        .filter(|step| matches!(step.status, crate::context::StepStatus::Completed))
        .count()
}
