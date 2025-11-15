use anyhow::Result;
use async_trait::async_trait;

/// Abstraction over whichever LLM backend is configured.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn sample(&self, prompt: &str) -> Result<String>;

    async fn sample_n(&self, prompt: &str, n: usize) -> Result<Vec<String>> {
        let mut outputs = Vec::with_capacity(n);
        for _ in 0..n {
            outputs.push(self.sample(prompt).await?);
        }
        Ok(outputs)
    }
}
