use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::Path,
};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_yaml::Value;

#[derive(Debug, Deserialize, Clone)]
pub struct MicrofactoryConfig {
    pub domains: HashMap<String, DomainConfig>,
}

impl MicrofactoryConfig {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let raw = fs::read_to_string(path_ref)
            .with_context(|| format!("Failed to read config file at {}", path_ref.display()))?;
        Self::from_str(&raw)
            .with_context(|| format!("Invalid configuration in {}", path_ref.display()))
    }

    pub fn from_str(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).context("Unable to parse config YAML")
    }

    pub fn domain(&self, name: &str) -> Option<&DomainConfig> {
        self.domains.get(name)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct DomainConfig {
    pub agents: AgentsConfig,
    #[serde(default)]
    pub step_granularity: StepGranularity,
    #[serde(default)]
    pub verifier: Option<String>,
    #[serde(default)]
    pub applier: Option<String>,
    #[serde(default)]
    pub red_flaggers: Vec<RedFlaggerConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentsConfig {
    pub decomposition: AgentDefinition,
    pub decomposition_discriminator: AgentDefinition,
    pub solver: AgentDefinition,
    pub solution_discriminator: AgentDefinition,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentDefinition {
    pub prompt_template: String,
    pub model: String,
    #[serde(default)]
    pub samples: Option<usize>,
    #[serde(default)]
    pub k: Option<usize>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StepGranularity {
    #[serde(default)]
    pub max_files: Option<usize>,
    #[serde(default)]
    pub max_lines_changed: Option<usize>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RedFlaggerConfig {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_config_from_str() {
        let yaml = r#"
        domains:
          code:
            agents:
              decomposition:
                prompt_template: "a"
                model: "m1"
                samples: 2
              decomposition_discriminator:
                prompt_template: "b"
                model: "m2"
                k: 3
              solver:
                prompt_template: "c"
                model: "m3"
                samples: 4
              solution_discriminator:
                prompt_template: "d"
                model: "m4"
                k: 2
            step_granularity:
              max_files: 1
            verifier: "pytest"
            applier: "patch"
            red_flaggers:
              - type: "length"
                max_tokens: 200
        "#;

        let config = MicrofactoryConfig::from_str(yaml).expect("valid config");
        let domain = config.domain("code").expect("code domain exists");
        assert_eq!(
            domain.red_flaggers.first().expect("red flagger").kind,
            "length"
        );
        assert_eq!(
            domain.step_granularity.max_files,
            Some(1),
            "step granularity parsed"
        );
    }
}
