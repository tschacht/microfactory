use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_yaml::Value;

use crate::config::RedFlaggerConfig;
use crate::llm::LlmClient;

/// Describes a single red-flag incident that caused a sample to be rejected.
#[derive(Debug, Clone)]
pub struct RedFlagMatch {
    pub flagger: String,
    pub reason: String,
}

#[async_trait]
pub trait RedFlagger: Send + Sync {
    fn name(&self) -> &str;
    async fn flag(&self, candidate: &str) -> Result<Option<String>>;
}

#[derive(Default)]
pub struct RedFlagPipeline {
    flaggers: Vec<Box<dyn RedFlagger>>,
}

impl RedFlagPipeline {
    pub fn from_configs(
        configs: &[RedFlaggerConfig],
        llm: Option<Arc<dyn LlmClient>>,
    ) -> Result<Self> {
        let mut flaggers: Vec<Box<dyn RedFlagger>> = Vec::new();
        for cfg in configs {
            let flagger: Box<dyn RedFlagger> = match cfg.kind.as_str() {
                "length" => {
                    let max_tokens = extract_usize(&cfg.params, "max_tokens")?;
                    Box::new(LengthRedFlagger { max_tokens })
                }
                "syntax" => {
                    let language = extract_string(&cfg.params, "language")?;
                    Box::new(SyntaxRedFlagger { language })
                }
                "llm_critique" => {
                    let client = llm
                        .clone()
                        .context("LLM client required for llm_critique red flagger")?;
                    let model = extract_string(&cfg.params, "model")?;
                    let prompt_template = extract_string(&cfg.params, "prompt_template")?;
                    Box::new(LlmRedFlagger {
                        client,
                        model,
                        prompt_template,
                    })
                }
                other => {
                    return Err(anyhow!("Unknown red flagger type: {other}"));
                }
            };
            flaggers.push(flagger);
        }
        Ok(Self { flaggers })
    }

    pub async fn evaluate(&self, candidate: &str) -> Vec<RedFlagMatch> {
        let mut matches = Vec::new();
        for flagger in &self.flaggers {
            match flagger.flag(candidate).await {
                Ok(Some(reason)) => {
                    matches.push(RedFlagMatch {
                        flagger: flagger.name().to_string(),
                        reason,
                    });
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        flagger = flagger.name(),
                        error = ?e,
                        "Red flagger failed to execute"
                    );
                }
            }
        }
        matches
    }

    pub fn is_empty(&self) -> bool {
        self.flaggers.is_empty()
    }
}

struct LengthRedFlagger {
    max_tokens: usize,
}

#[async_trait]
impl RedFlagger for LengthRedFlagger {
    fn name(&self) -> &str {
        "length"
    }

    async fn flag(&self, candidate: &str) -> Result<Option<String>> {
        let tokens = candidate.split_whitespace().count();
        if tokens > self.max_tokens {
            Ok(Some(format!(
                "response used {tokens} tokens exceeding limit {}",
                self.max_tokens
            )))
        } else {
            Ok(None)
        }
    }
}

struct SyntaxRedFlagger {
    language: String,
}

#[async_trait]
impl RedFlagger for SyntaxRedFlagger {
    fn name(&self) -> &str {
        "syntax"
    }

    async fn flag(&self, candidate: &str) -> Result<Option<String>> {
        if is_unbalanced(candidate) {
            Ok(Some(format!(
                "{} delimiters appear unbalanced",
                self.language
            )))
        } else {
            Ok(None)
        }
    }
}

struct LlmRedFlagger {
    client: Arc<dyn LlmClient>,
    model: String,
    prompt_template: String,
}

#[async_trait]
impl RedFlagger for LlmRedFlagger {
    fn name(&self) -> &str {
        "llm_critique"
    }

    async fn flag(&self, candidate: &str) -> Result<Option<String>> {
        let prompt = self.prompt_template.replace("{{candidate}}", candidate);
        let response = self.client.sample(&prompt, Some(&self.model)).await?;
        let trimmed = response.trim().to_lowercase();
        // Expecting the LLM to say "yes" if it's bad, or "no" if it's good, or some structured output.
        // Let's assume the prompt asks "Is this code invalid? Answer YES or NO."
        if trimmed.starts_with("yes") {
            Ok(Some(format!("LLM critique flagged content: {}", response)))
        } else {
            Ok(None)
        }
    }
}

fn is_unbalanced(text: &str) -> bool {
    let mut stack = Vec::new();
    for ch in text.chars() {
        match ch {
            '(' | '[' | '{' => stack.push(ch),
            ')' => {
                if stack.pop() != Some('(') {
                    return true;
                }
            }
            ']' => {
                if stack.pop() != Some('[') {
                    return true;
                }
            }
            '}' => {
                if stack.pop() != Some('{') {
                    return true;
                }
            }
            _ => {}
        }
    }
    !stack.is_empty()
}

fn extract_usize(map: &BTreeMap<String, Value>, key: &str) -> Result<usize> {
    map.get(key)
        .context(format!("Missing red flagger parameter '{key}'"))?
        .as_u64()
        .map(|v| v as usize)
        .context(format!("Parameter '{key}' must be a positive integer"))
}

fn extract_string(map: &BTreeMap<String, Value>, key: &str) -> Result<String> {
    map.get(key)
        .context(format!("Missing red flagger parameter '{key}'"))?
        .as_str()
        .map(|s| s.to_string())
        .context(format!("Parameter '{key}' must be a string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn length_flagger_detects_overflow() {
        let flagger = LengthRedFlagger { max_tokens: 3 };
        assert!(flagger.flag("one two three four").await.unwrap().is_some());
        assert!(flagger.flag("one two").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn syntax_flagger_detects_unbalanced() {
        let flagger = SyntaxRedFlagger {
            language: "python".into(),
        };
        assert!(flagger.flag("def foo(:").await.unwrap().is_some());
        assert!(flagger.flag("def foo(): pass").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn pipeline_builds_from_config() {
        let configs = vec![RedFlaggerConfig {
            kind: "length".into(),
            params: BTreeMap::from([(String::from("max_tokens"), Value::from(2))]),
        }];
        let pipeline = RedFlagPipeline::from_configs(&configs, None).unwrap();
        let matches = pipeline.evaluate("one two three").await;
        assert_eq!(matches.len(), 1);
    }
}
