use std::{cmp::max, sync::Arc};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use rig::client::{ProviderValue, builder::DynClientBuilder};
use rig::completion::Prompt;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::cli::LlmProvider;
use crate::core::error::Error as CoreError;
use crate::core::ports::{LlmClient as CoreLlmClient, LlmOptions};

/// Abstraction over whichever LLM backend is configured.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn sample(&self, prompt: &str, model: Option<&str>) -> Result<String>;

    async fn sample_n(&self, prompt: &str, n: usize, model: Option<&str>) -> Result<Vec<String>> {
        let mut outputs = Vec::with_capacity(n);
        for _ in 0..n {
            outputs.push(self.sample(prompt, model).await?);
        }
        Ok(outputs)
    }
}

/// Concrete [`LlmClient`] backed by `rig`'s OpenAI provider.
#[derive(Clone)]
pub struct RigLlmClient {
    inner: Arc<RigLlmClientInner>,
}

struct RigLlmClientInner {
    provider: LlmProvider,
    default_model: String,
    api_key: String,
    semaphore: Arc<Semaphore>,
}

impl RigLlmClient {
    pub fn new(
        provider: LlmProvider,
        api_key: impl Into<String>,
        model: impl Into<String>,
        max_concurrent: usize,
    ) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.trim().is_empty() {
            return Err(anyhow!("API key may not be empty"));
        }

        let default_model = model.into();
        if default_model.trim().is_empty() {
            return Err(anyhow!("Model identifier may not be empty"));
        }

        let limit = max(1, max_concurrent);
        Ok(Self {
            inner: Arc::new(RigLlmClientInner {
                provider,
                default_model,
                api_key,
                semaphore: Arc::new(Semaphore::new(limit)),
            }),
        })
    }
}

impl std::fmt::Debug for RigLlmClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RigLlmClient")
            .field("provider", &self.inner.provider)
            .field("default_model", &self.inner.default_model)
            .finish()
    }
}

#[async_trait]
impl LlmClient for RigLlmClient {
    async fn sample(&self, prompt: &str, model_override: Option<&str>) -> Result<String> {
        let permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .context("Semaphore closed while waiting for LLM slot")?;

        let model = model_override.unwrap_or(&self.inner.default_model);
        let agent = {
            let builder = DynClientBuilder::new();
            let agent_builder = builder
                .agent_with_api_key_val(
                    self.inner.provider.provider_id(),
                    model,
                    ProviderValue::Simple(self.inner.api_key.clone()),
                )
                .map_err(|err| anyhow!("Failed to create agent: {err}"))?;
            agent_builder.build()
        };
        let response = agent
            .prompt(prompt)
            .await
            .map_err(|err| anyhow!("LLM prompt failed: {err}"));

        drop(permit);
        response
    }

    async fn sample_n(&self, prompt: &str, n: usize, model: Option<&str>) -> Result<Vec<String>> {
        if n == 0 {
            return Ok(Vec::new());
        }

        let mut join_set = JoinSet::new();
        let model_owned = model.map(|m| m.to_string());
        for _ in 0..n {
            let prompt = prompt.to_owned();
            let client = self.clone();
            let model = model_owned.clone();
            join_set.spawn(async move { client.sample(&prompt, model.as_deref()).await });
        }

        let mut outputs = Vec::with_capacity(n);
        while let Some(result) = join_set.join_next().await {
            let value = result.context("LLM task panic or cancellation")??;
            outputs.push(value);
        }

        Ok(outputs)
    }
}

#[async_trait]
impl CoreLlmClient for RigLlmClient {
    async fn chat_completion(
        &self,
        model: &str,
        prompt: &str,
        options: &LlmOptions,
    ) -> crate::core::Result<String> {
        let _permit = self
            .inner
            .semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| CoreError::System(format!("Semaphore error: {e}")))?;

        let agent = {
            let builder = DynClientBuilder::new();
            let mut agent_builder = builder
                .agent_with_api_key_val(
                    self.inner.provider.provider_id(),
                    model,
                    ProviderValue::Simple(self.inner.api_key.clone()),
                )
                .map_err(|err| CoreError::LlmProvider {
                    provider: self.inner.provider.as_str().to_string(),
                    details: format!("Failed to create agent: {err}"),
                    retryable: false,
                })?;

            if let Some(temp) = options.temperature {
                agent_builder = agent_builder.temperature(f64::from(temp));
            }

            agent_builder.build()
        };

        let response = agent
            .prompt(prompt)
            .await
            .map_err(|err| CoreError::LlmProvider {
                provider: self.inner.provider.as_str().to_string(),
                details: err.to_string(),
                retryable: true,
            })?;

        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_api_key() {
        let err = RigLlmClient::new(LlmProvider::Openai, "   ", "model", 1).unwrap_err();
        assert!(err.to_string().contains("API key"));
    }

    #[test]
    fn rejects_empty_model() {
        let err = RigLlmClient::new(LlmProvider::Openai, "key", "   ", 1).unwrap_err();
        assert!(err.to_string().contains("Model"));
    }
}
