use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::info;

use crate::{config::MicrofactoryConfig, context::Context, llm::LlmClient};

/// Skeleton workflow runner; the real orchestration graph will arrive in later phases.
pub struct FlowRunner {
    config: Arc<MicrofactoryConfig>,
    llm: Option<Arc<dyn LlmClient>>, // reserved for later phases
}

impl FlowRunner {
    pub fn new(config: Arc<MicrofactoryConfig>, llm: Option<Arc<dyn LlmClient>>) -> Self {
        Self { config, llm }
    }

    pub async fn execute(&self, context: &mut Context) -> Result<()> {
        let domain_cfg = self
            .config
            .domain(&context.domain)
            .ok_or_else(|| anyhow!("Unknown domain: {}", context.domain))?;

        info!(
            domain = %context.domain,
            prompt = %context.prompt,
            red_flaggers = domain_cfg.red_flaggers.len(),
            has_llm = self.llm.is_some(),
            "FlowRunner execution placeholder"
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
}
