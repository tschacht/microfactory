#![warn(clippy::uninlined_format_args)]

pub mod adapters;
pub mod application;
pub mod config;
pub mod core;
pub mod paths;
pub mod red_flaggers;
pub mod status_export;
pub mod tracing_inspect;
pub mod tracing_setup;
pub mod utils;

// Re-export commonly used types for backward compatibility
pub use adapters::inbound::cli::{Cli, Commands, LlmProvider};
pub use application::{runner, service, tasks};

// Legacy re-exports for backward compatibility during transition
// TODO: These can be removed once all consumers migrate to the new paths
pub mod cli {
    pub use crate::adapters::inbound::cli::*;
}

pub mod server {
    pub use crate::adapters::inbound::server::*;
}
