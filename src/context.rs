//! Legacy location for workflow runtime state; re-export from `core::domain`.
//! The rest of this crate still uses `crate::context`, so we keep this shim in
//! place until all internal modules migrate to `core::domain` explicitly.

pub use crate::core::domain::*;
