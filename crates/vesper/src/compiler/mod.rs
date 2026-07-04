//! The boundary with the dusk compiler crate.
//!
//! This is the only module that names `dusk`. Everything above it works in
//! Language Server Protocol terms. When the dusk front end changes shape, this
//! is the one place vesper has to follow.

mod analyze;

pub use analyze::{semantic_diagnostics, syntax_diagnostics, FileDiagnostics};
