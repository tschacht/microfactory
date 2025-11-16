use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow};
use serde_yaml::Value;

use crate::config::RedFlaggerConfig;

/// Describes a single red-flag incident that caused a sample to be rejected.
#[derive(Debug, Clone)]
pub struct RedFlagMatch {
    pub flagger: String,
    pub reason: String,
}

pub trait RedFlagger: Send + Sync {
    fn name(&self) -> &str;
    fn flag(&self, candidate: &str) -> Option<String>;
}

#[derive(Default)]
pub struct RedFlagPipeline {
    flaggers: Vec<Box<dyn RedFlagger>>,
}

impl RedFlagPipeline {
    pub fn from_configs(configs: &[RedFlaggerConfig]) -> Result<Self> {
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
                other => {
                    return Err(anyhow!("Unknown red flagger type: {other}"));
                }
            };
            flaggers.push(flagger);
        }
        Ok(Self { flaggers })
    }

    pub fn evaluate(&self, candidate: &str) -> Vec<RedFlagMatch> {
        let mut matches = Vec::new();
        for flagger in &self.flaggers {
            if let Some(reason) = flagger.flag(candidate) {
                matches.push(RedFlagMatch {
                    flagger: flagger.name().to_string(),
                    reason,
                });
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

impl RedFlagger for LengthRedFlagger {
    fn name(&self) -> &str {
        "length"
    }

    fn flag(&self, candidate: &str) -> Option<String> {
        let tokens = candidate.split_whitespace().count();
        if tokens > self.max_tokens {
            Some(format!(
                "response used {tokens} tokens exceeding limit {}",
                self.max_tokens
            ))
        } else {
            None
        }
    }
}

struct SyntaxRedFlagger {
    language: String,
}

impl RedFlagger for SyntaxRedFlagger {
    fn name(&self) -> &str {
        "syntax"
    }

    fn flag(&self, candidate: &str) -> Option<String> {
        if is_unbalanced(candidate) {
            Some(format!("{} delimiters appear unbalanced", self.language))
        } else {
            None
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

    #[test]
    fn length_flagger_detects_overflow() {
        let flagger = LengthRedFlagger { max_tokens: 3 };
        assert!(flagger.flag("one two three four").is_some());
        assert!(flagger.flag("one two").is_none());
    }

    #[test]
    fn syntax_flagger_detects_unbalanced() {
        let flagger = SyntaxRedFlagger {
            language: "python".into(),
        };
        assert!(flagger.flag("def foo(:").is_some());
        assert!(flagger.flag("def foo(): pass").is_none());
    }

    #[test]
    fn pipeline_builds_from_config() {
        let configs = vec![RedFlaggerConfig {
            kind: "length".into(),
            params: BTreeMap::from([(String::from("max_tokens"), Value::from(2))]),
        }];
        let pipeline = RedFlagPipeline::from_configs(&configs).unwrap();
        let matches = pipeline.evaluate("one two three");
        assert_eq!(matches.len(), 1);
    }
}
