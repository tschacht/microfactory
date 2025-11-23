//! Directional port definitions for the clean architecture rings.
//! Inbound ports represent commands/events entering the core,
//! while outbound ports are services the core/application call out to.

pub mod inbound;
pub mod outbound;

pub use outbound::*;
