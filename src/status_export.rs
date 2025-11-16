use serde::{Deserialize, Serialize};

use crate::{
    context::{Context, WaitState, WorkflowMetrics},
    persistence::{SessionMetadata, SessionRecord, SessionSummary},
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
    pub metadata: SessionMetadata,
    pub completed_steps: usize,
    pub total_steps: usize,
    pub metrics: WorkflowMetrics,
}

impl SessionDetailExport {
    pub fn from_record(record: &SessionRecord) -> Self {
        let context = &record.envelope.context;
        Self {
            session_id: context.session_id.clone(),
            status: record.status.as_str().to_string(),
            prompt: context.prompt.clone(),
            domain: context.domain.clone(),
            updated_at: record.updated_at,
            wait_state: context.wait_state.clone(),
            metadata: record.envelope.metadata.clone(),
            completed_steps: count_completed_steps(context),
            total_steps: context.steps.len(),
            metrics: context.metrics.clone(),
        }
    }
}

pub fn count_completed_steps(ctx: &Context) -> usize {
    ctx.steps
        .iter()
        .filter(|step| matches!(step.status, crate::context::StepStatus::Completed))
        .count()
}
