pub mod inbound;
pub mod outbound;

// Re-export commonly used outbound adapters at the adapters level
pub use outbound::llm;
pub use outbound::persistence;
pub use outbound::templating;
