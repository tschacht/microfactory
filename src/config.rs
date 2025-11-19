use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
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
        let mut config = Self::from_yaml_str(&raw)
            .with_context(|| format!("Invalid configuration in {}", path_ref.display()))?;
        let base_dir = path_ref.parent().unwrap_or_else(|| Path::new("."));
        config
            .hydrate_templates(base_dir)
            .with_context(|| format!("Failed to hydrate templates for {}", path_ref.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(yaml).context("Unable to parse config YAML")?;
        config.validate()?;
        Ok(config)
    }

    pub fn domain(&self, name: &str) -> Option<&DomainConfig> {
        self.domains.get(name)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.domains.is_empty(),
            "Configuration must contain at least one domain"
        );
        for (name, domain) in &self.domains {
            domain
                .validate(name)
                .with_context(|| format!("Domain '{name}' failed validation"))?;
        }
        Ok(())
    }

    fn hydrate_templates<P: AsRef<Path>>(&mut self, base_dir: P) -> Result<()> {
        let base = base_dir.as_ref();
        for domain in self.domains.values_mut() {
            domain.hydrate_templates(base)?;
        }
        Ok(())
    }
}

impl FromStr for MicrofactoryConfig {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_yaml_str(s)
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

impl DomainConfig {
    fn hydrate_templates(&mut self, base_dir: &Path) -> Result<()> {
        self.agents.hydrate_templates(base_dir)
    }

    fn validate(&self, name: &str) -> Result<()> {
        self.agents.validate(name)?;
        self.step_granularity.validate(name)?;
        for (idx, flagger) in self.red_flaggers.iter().enumerate() {
            validate_red_flagger(name, idx, flagger)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentsConfig {
    pub decomposition: AgentDefinition,
    pub decomposition_discriminator: AgentDefinition,
    pub solver: AgentDefinition,
    pub solution_discriminator: AgentDefinition,
}

impl AgentsConfig {
    fn hydrate_templates(&mut self, base_dir: &Path) -> Result<()> {
        self.decomposition.hydrate_template(base_dir)?;
        self.decomposition_discriminator
            .hydrate_template(base_dir)?;
        self.solver.hydrate_template(base_dir)?;
        self.solution_discriminator.hydrate_template(base_dir)?;
        Ok(())
    }

    fn validate(&self, domain: &str) -> Result<()> {
        self.decomposition.validate(domain, "decomposition")?;
        self.decomposition_discriminator
            .validate(domain, "decomposition_discriminator")?;
        self.solver.validate(domain, "solver")?;
        self.solution_discriminator
            .validate(domain, "solution_discriminator")?;
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AgentDefinition {
    pub prompt_template: String,
    pub model: String,
    #[serde(default)]
    pub samples: Option<usize>,
    #[serde(default)]
    pub k: Option<usize>,
    #[serde(default)]
    pub red_flaggers: Option<Vec<RedFlaggerConfig>>,
}

impl AgentDefinition {
    fn hydrate_template(&mut self, base_dir: &Path) -> Result<()> {
        if self.prompt_template.trim().is_empty() {
            return Ok(());
        }
        self.prompt_template = resolve_prompt_template(&self.prompt_template, base_dir)?;
        Ok(())
    }

    fn validate(&self, domain: &str, role: &str) -> Result<()> {
        ensure!(
            !self.prompt_template.trim().is_empty(),
            "Domain '{domain}' role '{role}' must define a prompt_template"
        );
        ensure!(
            !self.model.trim().is_empty(),
            "Domain '{domain}' role '{role}' must define a model"
        );
        if let Some(samples) = self.samples {
            ensure!(
                samples > 0,
                "Domain '{domain}' role '{role}' samples must be > 0"
            );
        }
        if let Some(k) = self.k {
            ensure!(k > 0, "Domain '{domain}' role '{role}' k must be > 0");
        }
        if let Some(flaggers) = &self.red_flaggers {
            for (idx, flagger) in flaggers.iter().enumerate() {
                validate_red_flagger(domain, idx, flagger)
                    .with_context(|| format!("Invalid red_flagger for role '{role}'"))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct StepGranularity {
    #[serde(default)]
    pub max_files: Option<usize>,
    #[serde(default)]
    pub max_lines_changed: Option<usize>,
}

impl StepGranularity {
    fn validate(&self, domain: &str) -> Result<()> {
        if let Some(files) = self.max_files {
            ensure!(
                files > 0,
                "Domain '{domain}' step_granularity.max_files must be > 0"
            );
        }
        if let Some(lines) = self.max_lines_changed {
            ensure!(
                lines > 0,
                "Domain '{domain}' step_granularity.max_lines_changed must be > 0"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RedFlaggerConfig {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: BTreeMap<String, Value>,
}

fn validate_red_flagger(domain: &str, idx: usize, cfg: &RedFlaggerConfig) -> Result<()> {
    match cfg.kind.as_str() {
        "length" => {
            let value = cfg
                .params
                .get("max_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    anyhow!(
                        "Domain '{domain}' red_flaggers[{idx}] of type 'length' must set max_tokens"
                    )
                })?;
            ensure!(
                value > 0,
                "Domain '{domain}' red_flaggers[{idx}] max_tokens must be > 0"
            );
        }
        "syntax" => {
            let language = cfg
                .params
                .get("language")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "Domain '{domain}' red_flaggers[{idx}] of type 'syntax' must set language"
                    )
                })?;
            ensure!(
                !language.trim().is_empty(),
                "Domain '{domain}' red_flaggers[{idx}] language must not be blank"
            );
        }
        "llm_critique" => {
            let model = cfg
                .params
                .get("model")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "Domain '{domain}' red_flaggers[{idx}] of type 'llm_critique' must set model"
                    )
                })?;
            ensure!(
                !model.trim().is_empty(),
                "Domain '{domain}' red_flaggers[{idx}] model must not be blank"
            );
            let prompt_template = cfg
                .params
                .get("prompt_template")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    anyhow!(
                        "Domain '{domain}' red_flaggers[{idx}] of type 'llm_critique' must set prompt_template"
                    )
                })?;
            ensure!(
                !prompt_template.trim().is_empty(),
                "Domain '{domain}' red_flaggers[{idx}] prompt_template must not be blank"
            );
        }
        other => {
            return Err(anyhow!(
                "Domain '{domain}' references unknown red flagger type '{other}'"
            ));
        }
    }
    Ok(())
}

fn resolve_prompt_template(raw: &str, base_dir: &Path) -> Result<String> {
    if raw.contains('\n') {
        return Ok(raw.to_string());
    }

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    let candidate = Path::new(trimmed);
    let joined: PathBuf = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    };

    if joined.exists() && joined.is_file() {
        return fs::read_to_string(&joined)
            .with_context(|| format!("Failed to read prompt template {}", joined.display()));
    }

    if looks_like_template_path(trimmed) {
        return Err(anyhow!(
            "Prompt template '{}' was not found relative to {}",
            trimmed,
            base_dir.display()
        ));
    }

    Ok(raw.to_string())
}

fn looks_like_template_path(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || value.ends_with(".hbs")
        || value.ends_with(".handlebars")
        || value.ends_with(".tmpl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

        let config = MicrofactoryConfig::from_yaml_str(yaml).expect("valid config");
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

    #[test]
    fn from_path_hydrates_templates() {
        let temp = tempdir().unwrap();
        let templates = temp.path().join("templates");
        fs::create_dir(&templates).unwrap();
        let template_path = templates.join("demo.hbs");
        fs::write(&template_path, "Hydrated {{prompt}}").unwrap();

        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            r#"
domains:
  code:
    agents:
      decomposition:
        prompt_template: "templates/demo.hbs"
        model: "m1"
        samples: 1
      decomposition_discriminator:
        prompt_template: "templates/demo.hbs"
        model: "m2"
        k: 2
      solver:
        prompt_template: "templates/demo.hbs"
        model: "m3"
        samples: 1
      solution_discriminator:
        prompt_template: "templates/demo.hbs"
        model: "m4"
        k: 2
"#,
        )
        .unwrap();

        let config = MicrofactoryConfig::from_path(&config_path).expect("config loads");
        let domain = config.domain("code").expect("domain exists");
        assert_eq!(
            domain.agents.decomposition.prompt_template,
            "Hydrated {{prompt}}"
        );
    }

    #[test]
    fn rejects_invalid_red_flagger() {
        let yaml = r#"
        domains:
          code:
            agents:
              decomposition:
                prompt_template: "p"
                model: "m1"
              decomposition_discriminator:
                prompt_template: "p"
                model: "m2"
                k: 1
              solver:
                prompt_template: "p"
                model: "m3"
              solution_discriminator:
                prompt_template: "p"
                model: "m4"
                k: 1
            red_flaggers:
              - type: "length"
        "#;

        let err = MicrofactoryConfig::from_yaml_str(yaml).unwrap_err();
        let messages: Vec<String> = err.chain().map(|cause| cause.to_string()).collect();
        assert!(
            messages.iter().any(|msg| msg.contains("max_tokens")),
            "error chain missing max_tokens context: {messages:?}"
        );
    }
}
