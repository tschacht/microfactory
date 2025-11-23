use std::collections::HashMap;

use crate::core::domain::{AgentConfig, AgentKind, RedFlaggerDescriptor};

/// Runtime settings for a single agent role within a domain.
#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub prompt_template: String,
    pub model: String,
    pub samples: Option<usize>,
    pub k: Option<usize>,
    pub red_flaggers: Option<Vec<RedFlaggerDescriptor>>,
}

impl AgentSettings {
    pub fn as_agent_config(&self, kind: AgentKind, defaults: &AgentDefaults) -> AgentConfig {
        AgentConfig {
            kind,
            prompt_template: self.prompt_template.clone(),
            model: self.model.clone(),
            samples: self.samples.unwrap_or(defaults.samples).max(1),
            k: self.k.or(Some(defaults.k)),
            red_flaggers: self.red_flaggers.clone(),
        }
    }
}

/// Default overrides derived from CLI flags.
#[derive(Debug, Clone, Copy)]
pub struct AgentDefaults {
    pub samples: usize,
    pub k: usize,
}

/// Runtime representation of a domain after configuration parsing.
#[derive(Debug, Clone)]
pub struct DomainRuntimeConfig {
    pub name: String,
    pub agents: HashMap<AgentKind, AgentSettings>,
    pub applier: Option<String>,
    pub verifier: Option<String>,
    pub red_flaggers: Vec<RedFlaggerDescriptor>,
}

impl DomainRuntimeConfig {
    pub fn agent_settings(&self, kind: AgentKind) -> Option<&AgentSettings> {
        self.agents.get(&kind)
    }
}
