//! Directional port definitions for the clean architecture rings.
//! Inbound ports represent the application service interface that driving adapters consume,
//! while outbound ports are services the core/application call out to.

pub mod inbound;
pub mod outbound;

pub use inbound::*;
pub use outbound::*;
