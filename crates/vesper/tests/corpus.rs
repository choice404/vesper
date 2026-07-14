//! The dusk standard library is a body of real, compiling dusk. Vesper's syntax
//! pass must lex and parse every file in it without panicking and without
//! inventing an error, since the same pass runs on every keystroke against
//! partial buffers. This test walks the checkout the server is built against.

use std::fs;
use std::path::{Path, PathBuf};

use vesper::compiler::syntax_diagnostics;

/// The dusk standard library directory, relative to this crate. This is the
/// frozen seed's own stdlib, the body of dusk that matches the front end vesper
/// links, so the syntax pass is expected to parse every file in it clean.
fn stdlib_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../dusk-rust/lib/std")
}

/// Collects every `.dusk` file under `dir`.
fn dusk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dusk_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("dusk") {
            out.push(path);
        }
    }
}

#[test]
fn stdlib_lexes_and_parses_clean() {
    let dir = stdlib_dir();
    if !dir.is_dir() {
        // The dusk checkout is not beside this repo, so there is nothing to walk.
        eprintln!("skipping: no stdlib at {}", dir.display());
        return;
    }

    let mut files = Vec::new();
    dusk_files(&dir, &mut files);
    assert!(!files.is_empty(), "found no dusk files under {}", dir.display());

    let mut dirty = Vec::new();
    for file in &files {
        let src = fs::read_to_string(file).expect("stdlib file should read");
        let diags = syntax_diagnostics(&src);
        if !diags.is_empty() {
            dirty.push(format!("{}: {} diagnostics", file.display(), diags.len()));
        }
    }

    assert!(dirty.is_empty(), "stdlib should lex and parse clean:\n{}", dirty.join("\n"));
}
