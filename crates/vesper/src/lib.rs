//! Vesper is a language server for the dusk programming language. It links the
//! dusk compiler front end directly, so the diagnostics, highlighting, and
//! navigation an editor shows come from the same lexer, parser, and checker that
//! build the program.
//!
//! Every reach into the dusk crate lives under [`compiler`]. The rest of vesper
//! works in Language Server Protocol terms, so a breaking change upstream lands
//! in one module and nowhere else.

pub mod compiler;
pub mod config;
pub mod document;
pub mod position;
pub mod server;
pub mod store;
pub mod workspace;

pub use server::Backend;

use tower_lsp::{LspService, Server};

/// Serves the language server over stdio until the client disconnects.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
