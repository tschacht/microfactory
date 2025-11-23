//! Inbound adapters translate external stimuli (CLI/HTTP) into application commands.
//! For now we re-export the legacy modules while they are migrated.

pub mod cli {
    pub use crate::cli::*;
}

pub mod server {
    pub use crate::server::*;
}
