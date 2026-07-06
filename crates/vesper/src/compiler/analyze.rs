//! Runs the dusk passes and turns their diagnostics into protocol diagnostics.
//!
//! Two entry points, split by what they cost. [`syntax_diagnostics`] works on a
//! single in-memory buffer and touches no other file, so it runs on every edit.
//! [`semantic_diagnostics`] loads the whole program from disk and checks names
//! and types across modules, so it runs on open and save.

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range};

use dusk::diag::Diagnostic as DuskDiag;
use dusk::loader::{self, FileSrc};

use super::guard::{self, TooComplex};
use crate::position::LineIndex;

/// Lexes, parses, and paradigm gates one buffer, returning the diagnostics that
/// need no cross file context. Paradigm gating runs only when the buffer lexes
/// and parses clean, so a half typed line does not pile a paradigm error on top
/// of the parse error the editor already shows.
pub fn syntax_diagnostics(text: &str) -> Vec<Diagnostic> {
    let index = LineIndex::new(text);
    let (tokens, lex_errs) = dusk::lexer::lex(text);

    // Hold a pathologically shaped buffer back from the parser. The dusk parser
    // is recursive descent with no depth limit, so a wall of brackets or a chain
    // thousands deep would overflow its stack and abort the process, which Rust
    // cannot catch. One clear note stands in for the analysis that is skipped.
    if let Some((why, at)) = guard::check(&tokens) {
        return vec![guard_diag(&index, text, why, at)];
    }

    let (module, parse_errs) = dusk::parser::parse(tokens);

    let mut out = Vec::new();
    for d in lex_errs.iter().chain(parse_errs.iter()) {
        out.push(to_lsp(&index, text, d));
    }
    if lex_errs.is_empty() && parse_errs.is_empty() {
        for d in dusk::sema::paradigm::check(&module) {
            out.push(to_lsp(&index, text, &d));
        }
    }
    out
}

/// The diagnostics for one file in a loaded program, already ranged against that
/// file's own source.
pub struct FileDiagnostics {
    pub path: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Loads `path` and everything it imports, then resolves names, checks types,
/// and runs monomorphization over the merged program. Each diagnostic maps back
/// to the file its span falls in, so an error in an imported module points at
/// that module rather than the file that imported it.
///
/// This reads from disk. Vesper calls it on open and save, when the buffer and
/// the file on disk agree, not on every keystroke.
pub fn semantic_diagnostics(path: &str) -> Vec<FileDiagnostics> {
    // Guard the root before the loader parses it. The loader reads and parses the
    // file itself, so a pathological root would overflow the parser mid load. The
    // syntax pass already showed the reason against the buffer, so here just skip
    // the load rather than repeat it.
    if let Ok(text) = std::fs::read_to_string(path) {
        let (tokens, _) = dusk::lexer::lex(&text);
        if guard::check(&tokens).is_some() {
            return Vec::new();
        }
    }

    let prog = loader::load(path);

    let Some(module) = prog.module.as_ref() else {
        // The root file failed to lex or parse. The syntax pass already reports
        // that against the buffer with precise spans, so do not repeat it here as
        // a spanless whole file error.
        return Vec::new();
    };

    // The root parsed, but a loader error remains: an import that did not
    // resolve, or a file it pulled in that did not lex or parse. The imported
    // items are missing from the merged module, so running the checker over it
    // would both bury the real error and invent undefined name errors for every
    // missing symbol. Mirror the compiler's own pipeline and stop here, surfacing
    // the loader errors instead. They come pre-rendered without spans, so they
    // land on the root file rather than a precise range.
    if !prog.errors.is_empty() {
        let diagnostics = prog.errors.iter().map(|m| whole_file_diag(m)).collect();
        return vec![FileDiagnostics {
            path: path.to_string(),
            diagnostics,
        }];
    }

    let desugared = dusk::desugar::run(module);
    // sema::check now hands back a span to type map alongside its diagnostics.
    // Vesper does not use the types yet, so keep only the diagnostics.
    let (diags, _types) = dusk::sema::check(&desugared);
    group_by_file(&prog.files, &diags)
}

/// Splits merged program diagnostics into per file lists, ranging each span
/// against the file it belongs to. This mirrors the loader's own file lookup:
/// spans are shifted by each file's base at load time, so the owning file is the
/// last one whose base is not past the span.
fn group_by_file(files: &[FileSrc], diags: &[DuskDiag]) -> Vec<FileDiagnostics> {
    let indices: Vec<LineIndex> = files.iter().map(|f| LineIndex::new(&f.src)).collect();
    let mut out: Vec<FileDiagnostics> = files
        .iter()
        .map(|f| FileDiagnostics {
            path: f.path.clone(),
            diagnostics: Vec::new(),
        })
        .collect();

    for d in diags {
        let Some(i) = owning_file(files, d.span.lo) else {
            continue;
        };
        let file = &files[i];
        let lo = d.span.lo - file.base;
        let hi = d.span.hi.saturating_sub(file.base);
        let range = Range::new(
            indices[i].position(&file.src, lo),
            indices[i].position(&file.src, hi),
        );
        out[i].diagnostics.push(dusk_diag(range, &d.msg));
    }

    out.retain(|f| !f.diagnostics.is_empty());
    out
}

/// The index of the file a merged span belongs to.
fn owning_file(files: &[FileSrc], lo: u32) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .rev()
        .find(|(_, f)| lo >= f.base)
        .map(|(i, _)| i)
}

/// Builds a diagnostic for a message that has no span, pinned to the start of
/// the file. Loader errors arrive already rendered as text, so this is the best
/// place to show them until the loader hands back spans of its own.
fn whole_file_diag(message: &str) -> Diagnostic {
    let start = tower_lsp::lsp_types::Position::new(0, 0);
    dusk_diag(Range::new(start, start), message)
}

/// Converts one in-file diagnostic to a protocol diagnostic against `text`.
fn to_lsp(index: &LineIndex, text: &str, d: &DuskDiag) -> Diagnostic {
    let range = Range::new(index.position(text, d.span.lo), index.position(text, d.span.hi));
    dusk_diag(range, &d.msg)
}

/// Builds a protocol diagnostic tagged as coming from dusk. The compiler reports
/// every diagnostic as an error today, so severity is fixed until the compiler
/// grows a severity of its own.
fn dusk_diag(range: Range, message: &str) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("dusk".to_string()),
        message: message.to_string(),
        ..Default::default()
    }
}

/// Builds the note vesper shows where it held a buffer back from the parser. It
/// is tagged as coming from vesper, not dusk, and warns rather than errors, since
/// the code may be perfectly valid, only too deep to analyze without risking the
/// parser. The range covers the token that first crossed the limit.
fn guard_diag(index: &LineIndex, text: &str, why: TooComplex, at: u32) -> Diagnostic {
    let start = index.position(text, at);
    let end = index.position(text, at + 1);
    Diagnostic {
        range: Range::new(start, end),
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("vesper".to_string()),
        message: why.reason().to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_program_has_no_syntax_diagnostics() {
        let src = "@paradigm procedural\nfunc main() -> int32 {\n    return 0\n}\n";
        assert!(syntax_diagnostics(src).is_empty(), "{:?}", syntax_diagnostics(src));
    }

    #[test]
    fn a_broken_program_reports_a_syntax_diagnostic() {
        let src = "func main( -> int32 {\n}\n";
        assert!(!syntax_diagnostics(src).is_empty());
    }

    #[test]
    fn a_pathologically_nested_buffer_is_guarded_not_parsed() {
        // Without the guard this wall of brackets overflows the parser stack and
        // aborts the process. The test running to its assertions at all is the
        // proof the guard stops that; the assertions prove it says why, once,
        // tagged as vesper's own note rather than a dusk error.
        let deep = format!(
            "func main() -> int32 {{\n    return {}0{}\n}}\n",
            "(".repeat(30_000),
            ")".repeat(30_000)
        );
        let diags = syntax_diagnostics(&deep);
        assert_eq!(diags.len(), 1, "one guard note stands in for analysis");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diags[0].source.as_deref(), Some("vesper"));
    }
}
