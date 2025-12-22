use std::{cmp::max, sync::Arc};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use rig::{
    client::CompletionClient,
    completion::Prompt,
    providers::{anthropic, gemini, openai, xai},
};
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
    http_client: reqwest::Client,
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

        let http_client = build_http_client()?;
        let limit = max(1, max_concurrent);
        Ok(Self {
            inner: Arc::new(RigLlmClientInner {
                provider,
                default_model,
                api_key,
                http_client,
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
        let response = self
            .prompt_once(model, prompt, None)
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

        self.prompt_once(model, prompt, options.temperature.map(f64::from))
            .await
            .map_err(|err| CoreError::LlmProvider {
                provider: self.inner.provider.as_str().to_string(),
                details: err.to_string(),
                retryable: true,
            })
    }
}

impl RigLlmClient {
    async fn prompt_once(
        &self,
        model: &str,
        prompt: &str,
        temperature: Option<f64>,
    ) -> Result<String> {
        match self.inner.provider {
            LlmProvider::Openai => {
                let client: openai::Client<reqwest::Client> =
                    openai::Client::<reqwest::Client>::builder()
                        .api_key(&self.inner.api_key)
                        .http_client(self.inner.http_client.clone())
                        .build()
                        .map_err(|err| anyhow!("Failed to create OpenAI client: {err}"))?;

                let mut agent_builder = client.agent(model);
                if let Some(temp) = temperature {
                    agent_builder = agent_builder.temperature(temp);
                }
                agent_builder
                    .build()
                    .prompt(prompt)
                    .await
                    .map_err(|err| anyhow!("OpenAI prompt error: {err}"))
            }
            LlmProvider::Anthropic => {
                let client: anthropic::Client<reqwest::Client> =
                    anthropic::Client::<reqwest::Client>::builder()
                        .api_key(&self.inner.api_key)
                        .http_client(self.inner.http_client.clone())
                        .build()
                        .map_err(|err| anyhow!("Failed to create Anthropic client: {err}"))?;

                let mut agent_builder = client.agent(model);
                if let Some(temp) = temperature {
                    agent_builder = agent_builder.temperature(temp);
                }
                agent_builder
                    .build()
                    .prompt(prompt)
                    .await
                    .map_err(|err| anyhow!("Anthropic prompt error: {err}"))
            }
            LlmProvider::Gemini => {
                let client: gemini::Client<reqwest::Client> =
                    gemini::Client::<reqwest::Client>::builder()
                        .api_key(&self.inner.api_key)
                        .http_client(self.inner.http_client.clone())
                        .build()
                        .map_err(|err| anyhow!("Failed to create Gemini client: {err}"))?;

                let mut agent_builder = client.agent(model);
                if let Some(temp) = temperature {
                    agent_builder = agent_builder.temperature(temp);
                }
                agent_builder
                    .build()
                    .prompt(prompt)
                    .await
                    .map_err(|err| anyhow!("Gemini prompt error: {err}"))
            }
            LlmProvider::Grok => {
                let client: xai::Client<reqwest::Client> =
                    xai::Client::<reqwest::Client>::builder()
                        .api_key(&self.inner.api_key)
                        .http_client(self.inner.http_client.clone())
                        .build()
                        .map_err(|err| anyhow!("Failed to create xAI client: {err}"))?;

                let mut agent_builder = client.agent(model);
                if let Some(temp) = temperature {
                    agent_builder = agent_builder.temperature(temp);
                }
                agent_builder
                    .build()
                    .prompt(prompt)
                    .await
                    .map_err(|err| anyhow!("xAI prompt error: {err}"))
            }
        }
    }
}

fn build_http_client() -> Result<reqwest::Client> {
    // `reqwest::Client::default()` can consult OS-level proxy settings.
    // On macOS this can involve `system-configuration`, which has been observed to panic in
    // sandboxed/restricted environments. We avoid that path by default.
    //
    // If one explicitly wants OS-level proxy discovery, opt in with:
    // `MICROFACTORY_ENABLE_SYSTEM_PROXY=1`.
    let mut builder = reqwest::Client::builder();
    if std::env::var_os("MICROFACTORY_ENABLE_SYSTEM_PROXY").is_none() {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|err| anyhow!("Failed to build HTTP client: {err}"))
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
