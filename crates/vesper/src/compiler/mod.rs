//! The boundary with the dusk compiler crate.
//!
//! This is the only module that names `dusk`. Everything above it works in
//! Language Server Protocol terms. When the dusk front end changes shape, this
//! is the one place vesper has to follow.

mod analyze;
mod builtins;
mod guard;
mod symbols;
mod tokens;

pub use analyze::{semantic_diagnostics, syntax_diagnostics, FileDiagnostics};
pub use builtins::is_primitive;
pub use symbols::{definition, document_symbols, hover, identifiers, name_at, Symbol};
pub use tokens::{legend, semantic_tokens};
