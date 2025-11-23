use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use tree_sitter::{Parser, Tree};

use crate::core::domain::RedFlaggerDescriptor;
use crate::core::error::Error as CoreError;
use crate::core::ports::{LlmClient, LlmOptions, RedFlagger};
use crate::utils::extract_xml_files;

/// Describes a single red-flag incident that caused a sample to be rejected.
#[derive(Debug, Clone)]
pub struct RedFlagMatch {
    pub flagger: String,
    pub reason: String,
}

#[derive(Default)]
pub struct RedFlagPipeline {
    flaggers: Vec<Box<dyn RedFlagger>>,
}

impl RedFlagPipeline {
    pub fn from_configs(
        configs: &[RedFlaggerDescriptor],
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
                    let extract_xml = extract_bool(&cfg.params, "extract_xml")?.unwrap_or(false);
                    Box::new(SyntaxRedFlagger {
                        language,
                        extract_xml,
                    })
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
            match flagger.check(candidate).await {
                Ok(()) => {}
                Err(CoreError::RedFlag { flagger: f, reason }) => {
                    matches.push(RedFlagMatch { flagger: f, reason });
                }
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

    async fn check(&self, candidate: &str) -> crate::core::Result<()> {
        let tokens = candidate.split_whitespace().count();
        if tokens > self.max_tokens {
            Err(CoreError::RedFlag {
                flagger: self.name().into(),
                reason: format!(
                    "response used {tokens} tokens exceeding limit {}",
                    self.max_tokens
                ),
            })
        } else {
            Ok(())
        }
    }
}

struct SyntaxRedFlagger {
    language: String,
    extract_xml: bool,
}

#[async_trait]
impl RedFlagger for SyntaxRedFlagger {
    fn name(&self) -> &str {
        "syntax"
    }

    async fn check(&self, candidate: &str) -> crate::core::Result<()> {
        if self.extract_xml {
            let files = extract_xml_files(candidate);
            if !files.is_empty() {
                for (path, content) in files {
                    let lang = infer_language(&path).unwrap_or(&self.language);
                    if let Some(error) = check_syntax(&content, lang)
                        .map_err(|e| CoreError::System(e.to_string()))?
                    {
                        return Err(CoreError::RedFlag {
                            flagger: self.name().into(),
                            reason: format!("Syntax error in {path}: {error}"),
                        });
                    }
                }
                return Ok(());
            }
        }

        // Fallback
        if let Some(error) =
            check_syntax(candidate, &self.language).map_err(|e| CoreError::System(e.to_string()))?
        {
            Err(CoreError::RedFlag {
                flagger: self.name().into(),
                reason: error,
            })
        } else {
            Ok(())
        }
    }
}

fn infer_language(path: &str) -> Option<&str> {
    if path.ends_with(".rs") {
        Some("rust")
    } else if path.ends_with(".py") {
        Some("python")
    } else if path.ends_with(".java") {
        Some("java")
    } else if path.ends_with(".js") || path.ends_with(".ts") {
        Some("javascript")
    } else {
        None
    }
}

fn check_syntax(content: &str, language_name: &str) -> Result<Option<String>> {
    let language = match language_name {
        "python" => tree_sitter_python::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        _ => {
            // Fallback to simple check for non-supported languages
            if is_unbalanced(content) {
                return Ok(Some(format!(
                    "{language_name} delimiters appear unbalanced (simple check)"
                )));
            }
            return Ok(None);
        }
    };

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .context("Error loading grammar")?;

    let tree = parser
        .parse(content, None)
        .context("Failed to parse code")?;

    if let Some(error) = find_syntax_error(&tree) {
        Ok(Some(format!("Syntax error detected: {error}")))
    } else {
        Ok(None)
    }
}

fn find_syntax_error(tree: &Tree) -> Option<String> {
    let mut cursor = tree.walk();

    // Pre-order traversal
    loop {
        let node = cursor.node();
        if node.is_error() {
            return Some(format!(
                "Error at line {}, column {}",
                node.start_position().row + 1,
                node.start_position().column + 1
            ));
        }
        if node.is_missing() {
            return Some(format!(
                "Missing token at line {}, column {}",
                node.start_position().row + 1,
                node.start_position().column + 1
            ));
        }

        if cursor.goto_first_child() {
            continue;
        }

        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return None;
            }
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

    async fn check(&self, candidate: &str) -> crate::core::Result<()> {
        let prompt = self.prompt_template.replace("{{candidate}}", candidate);
        let response = self
            .client
            .chat_completion(&self.model, &prompt, &LlmOptions::default())
            .await
            .map_err(|e| CoreError::LlmProvider {
                provider: "unknown".into(),
                details: e.to_string(),
                retryable: true,
            })?;

        let trimmed = response.trim().to_lowercase();
        if trimmed.starts_with("yes") {
            Err(CoreError::RedFlag {
                flagger: self.name().into(),
                reason: format!("LLM critique flagged content: {response}"),
            })
        } else {
            Ok(())
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

fn extract_usize(map: &HashMap<String, Value>, key: &str) -> Result<usize> {
    map.get(key)
        .context(format!("Missing red flagger parameter '{key}'"))?
        .as_u64()
        .map(|v| v as usize)
        .context(format!("Parameter '{key}' must be a positive integer"))
}

fn extract_string(map: &HashMap<String, Value>, key: &str) -> Result<String> {
    map.get(key)
        .context(format!("Missing red flagger parameter '{key}'"))?
        .as_str()
        .map(|s| s.to_string())
        .context(format!("Parameter '{key}' must be a string"))
}

fn extract_bool(map: &HashMap<String, Value>, key: &str) -> Result<Option<bool>> {
    match map.get(key) {
        Some(val) => val
            .as_bool()
            .map(Some)
            .context(format!("Parameter '{key}' must be a boolean")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn length_flagger_detects_overflow() {
        let flagger = LengthRedFlagger { max_tokens: 3 };
        assert!(flagger.check("one two three four").await.is_err());
        assert!(flagger.check("one two").await.is_ok());
    }

    #[tokio::test]
    async fn syntax_flagger_detects_errors() {
        let flagger = SyntaxRedFlagger {
            language: "python".into(),
            extract_xml: false,
        };
        // Invalid Python
        assert!(flagger.check("def foo() pass").await.is_err());
        // Valid Python
        assert!(flagger.check("def foo(): pass").await.is_ok());

        let rust_flagger = SyntaxRedFlagger {
            language: "rust".into(),
            extract_xml: false,
        };
        // Invalid Rust: missing semicolon
        assert!(rust_flagger.check("fn main() { let x = 1 }").await.is_err());
        // Valid Rust
        assert!(rust_flagger.check("fn main() { let x = 1; }").await.is_ok());

        let java_flagger = SyntaxRedFlagger {
            language: "java".into(),
            extract_xml: false,
        };
        // Invalid Java: missing semicolon
        assert!(
            java_flagger
                .check("class Main { void main() { int x = 1 } }")
                .await
                .is_err()
        );
        // Valid Java
        assert!(
            java_flagger
                .check("class Main { void main() { int x = 1; } }")
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn syntax_flagger_extracts_xml() {
        let flagger = SyntaxRedFlagger {
            language: "python".into(),
            extract_xml: true,
        };

        let valid_xml = r#"
            <file path="script.py">
            def foo():
                pass
            </file>
        "#;
        assert!(flagger.check(valid_xml).await.is_ok());

        let invalid_xml = r#"
            <file path="script.py">
            def foo() pass
            </file>
        "#;
        let err = flagger.check(invalid_xml).await.unwrap_err();
        assert!(err.to_string().contains("Syntax error in script.py"));

        // Mixed languages
        let mixed_xml = r#"
            <file path="script.py">
            def foo(): pass
            </file>
            <file path="main.rs">
            fn main() { let x = 1; }
            </file>
        "#;
        assert!(flagger.check(mixed_xml).await.is_ok());

        let mixed_invalid_xml = r#"
            <file path="script.py">
            def foo(): pass
            </file>
            <file path="main.rs">
            fn main() { let x = 1 }
            </file>
        "#;
        let err = flagger.check(mixed_invalid_xml).await.unwrap_err();
        assert!(err.to_string().contains("Syntax error in main.rs"));
    }

    #[tokio::test]
    async fn pipeline_builds_from_config() {
        let configs = vec![RedFlaggerDescriptor {
            kind: "length".into(),
            params: HashMap::from([(String::from("max_tokens"), Value::from(2))]),
        }];
        // Pass None for LLM since we don't use it in this config
        let pipeline = RedFlagPipeline::from_configs(&configs, None).unwrap();
        let matches = pipeline.evaluate("one two three").await;
        assert_eq!(matches.len(), 1);
    }

    struct MockLlm {
        response: String,
    }

    #[async_trait]
    impl LlmClient for MockLlm {
        async fn chat_completion(
            &self,
            _model: &str,
            _prompt: &str,
            _options: &LlmOptions,
        ) -> crate::core::Result<String> {
            Ok(self.response.clone())
        }
    }

    #[tokio::test]
    async fn llm_flagger_flags_on_yes() {
        let client = Arc::new(MockLlm {
            response: "YES, this code is bad".into(),
        });
        let flagger = LlmRedFlagger {
            client,
            model: "test-model".into(),
            prompt_template: "Critique: {{candidate}}".into(),
        };
        let result = flagger.check("bad code").await.unwrap_err();
        assert!(result.to_string().contains("LLM critique flagged"));
    }

    #[tokio::test]
    async fn llm_flagger_passes_on_no() {
        let client = Arc::new(MockLlm {
            response: "NO, it looks good".into(),
        });
        let flagger = LlmRedFlagger {
            client,
            model: "test-model".into(),
            prompt_template: "Critique: {{candidate}}".into(),
        };
        let result = flagger.check("good code").await;
        assert!(result.is_ok());
    }
}
