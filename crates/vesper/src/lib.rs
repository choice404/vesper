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
    install_memory_backstop();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}

/// Caps this process's address space, so a runaway in the linked compiler aborts
/// at a ceiling instead of exhausting the machine.
///
/// The dusk front end is young and can loop or allocate without bound on some
/// inputs. Vesper guards the buffer shapes it can see before they reach the
/// parser, but this is the last line behind that guard. If any pass ever climbs
/// past the cap, the allocator fails and the process aborts, which restarts one
/// server rather than taking the machine into swap. The cap is generous, far
/// above any real workspace. Move it with `VESPER_MEMORY_CAP_MB`, or set that to
/// `0` to lift it.
#[cfg(unix)]
pub fn install_memory_backstop() {
    const DEFAULT_CAP_MB: u64 = 8 * 1024;
    let cap_mb = std::env::var("VESPER_MEMORY_CAP_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CAP_MB);
    if cap_mb == 0 {
        return;
    }
    let cap = cap_mb.saturating_mul(1024 * 1024);

    // Safety: both calls take a pointer to an `rlimit` this function owns and
    // outlives the calls. We read the current limits, then only lower the soft
    // limit, never above the hard limit the OS already enforces.
    unsafe {
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_AS, &mut lim) != 0 {
            return;
        }
        let want = if lim.rlim_max == libc::RLIM_INFINITY {
            cap
        } else {
            cap.min(lim.rlim_max)
        };
        lim.rlim_cur = want;
        let _ = libc::setrlimit(libc::RLIMIT_AS, &lim);
    }
}

/// On platforms without POSIX resource limits vesper leans on the buffer guard
/// alone. The address space backstop is a no op here.
#[cfg(not(unix))]
pub fn install_memory_backstop() {}
