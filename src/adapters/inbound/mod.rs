//! Inbound adapters translate external stimuli (CLI/HTTP) into application service calls.
//! These are driving adapters that consume the `WorkflowService` trait.

pub mod cli;
pub mod server;

pub use cli::{Cli, CliAdapter, Commands, InspectMode, LlmProvider, RunArgs, ServeArgs};
pub use server::{ServeOptions, ServerAdapter};
