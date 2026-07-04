//! A workspace wide index over the dusk files a project holds.
//!
//! Each file contributes the names it declares and every identifier it mentions.
//! Since the compiler does not hand back a resolved symbol per use, references
//! and cross file definitions are matched by name. A name the workspace declares
//! is treated as a global and searched everywhere; a name it does not is likely
//! a local and stays in its own file.

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use tower_lsp::lsp_types::{Location, Range, SymbolInformation, Url};

use crate::compiler;

/// One file's contribution to the index.
struct FileIndex {
    symbols: Vec<compiler::Symbol>,
    idents: Vec<(String, Range)>,
}

/// The project index, keyed by file URI.
#[derive(Default)]
pub struct Workspace {
    files: DashMap<Url, FileIndex>,
}

impl Workspace {
    pub fn new() -> Self {
        Workspace::default()
    }

    /// Indexes a file from its current text, replacing any earlier entry.
    pub fn index(&self, uri: Url, text: &str) {
        self.files.insert(uri, build(text));
    }

    /// Indexes a file only if it is not already indexed. The startup scan uses
    /// this so it cannot clobber a file an editor already opened and indexed from
    /// a newer, unsaved buffer.
    pub fn index_if_absent(&self, uri: Url, text: &str) {
        if let Entry::Vacant(slot) = self.files.entry(uri) {
            slot.insert(build(text));
        }
    }

    /// Drops a file from the index.
    pub fn remove(&self, uri: &Url) {
        self.files.remove(uri);
    }

    /// Whether any file declares `name`, which marks it a global worth searching
    /// the whole workspace for.
    pub fn declares(&self, name: &str) -> bool {
        self.files
            .iter()
            .any(|f| f.value().symbols.iter().any(|s| s.name == name))
    }

    /// The definition locations for a name, across every file.
    pub fn definitions(&self, name: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for file in self.files.iter() {
            for symbol in &file.value().symbols {
                if symbol.name == name {
                    out.push(Location::new(file.key().clone(), symbol.name_range));
                }
            }
        }
        out
    }

    /// A signature detail for a name, for a cross file hover. When several files
    /// declare the name, the one with the smallest URI is chosen, so the hover is
    /// stable rather than depending on map iteration order.
    pub fn detail(&self, name: &str) -> Option<String> {
        let mut best: Option<(String, String)> = None;
        for file in self.files.iter() {
            let uri = file.key().as_str().to_string();
            for symbol in &file.value().symbols {
                if symbol.name == name {
                    if best.as_ref().is_none_or(|(u, _)| uri < *u) {
                        best = Some((uri.clone(), symbol.detail.clone()));
                    }
                    break;
                }
            }
        }
        best.map(|(_, detail)| detail)
    }

    /// Every occurrence of a name. When `restrict` is set, only that file is
    /// searched, which keeps a likely local from matching unrelated names in
    /// other files.
    pub fn references(&self, name: &str, restrict: Option<&Url>) -> Vec<Location> {
        let mut out = Vec::new();
        for file in self.files.iter() {
            if let Some(only) = restrict {
                if file.key() != only {
                    continue;
                }
            }
            for (n, range) in &file.value().idents {
                if n == name {
                    out.push(Location::new(file.key().clone(), *range));
                }
            }
        }
        out
    }

    /// The declared symbols whose name contains `query`, case insensitively. An
    /// empty query returns everything.
    pub fn symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let needle = query.to_lowercase();
        let mut out = Vec::new();
        for file in self.files.iter() {
            for symbol in &file.value().symbols {
                if needle.is_empty() || symbol.name.to_lowercase().contains(&needle) {
                    out.push(symbol_information(symbol, file.key()));
                }
            }
        }
        out
    }
}

/// Builds one file's index from its text.
fn build(text: &str) -> FileIndex {
    FileIndex {
        symbols: compiler::document_symbols(text),
        idents: compiler::identifiers(text),
    }
}

/// Builds a workspace symbol entry pointing at a declaration.
fn symbol_information(symbol: &compiler::Symbol, uri: &Url) -> SymbolInformation {
    #[allow(deprecated)]
    SymbolInformation {
        name: symbol.name.clone(),
        kind: symbol.kind,
        tags: None,
        deprecated: None,
        location: Location::new(uri.clone(), symbol.name_range),
        container_name: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///{name}")).unwrap()
    }

    fn built() -> (Workspace, Url, Url) {
        let ws = Workspace::new();
        let a = uri("a.dusk");
        let b = uri("b.dusk");
        ws.index(a.clone(), "func foo() -> int32 {\n    return 0\n}\n");
        ws.index(b.clone(), "func main() -> int32 {\n    return foo()\n}\n");
        (ws, a, b)
    }

    #[test]
    fn a_declared_name_is_a_global() {
        let (ws, _, _) = built();
        assert!(ws.declares("foo"));
        assert!(ws.declares("main"));
        assert!(!ws.declares("nope"));
    }

    #[test]
    fn definition_points_at_the_declaring_file() {
        let (ws, a, _) = built();
        let defs = ws.definitions("foo");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].uri, a);
    }

    #[test]
    fn references_span_every_file() {
        let (ws, _, _) = built();
        // The declaration in a.dusk and the call in b.dusk.
        assert_eq!(ws.references("foo", None).len(), 2);
    }

    #[test]
    fn references_can_stay_in_one_file() {
        let (ws, _, b) = built();
        // Restricted to b.dusk, only the call remains.
        let refs = ws.references("foo", Some(&b));
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].uri, b);
    }

    #[test]
    fn symbols_match_by_substring() {
        let (ws, _, _) = built();
        let hits: Vec<String> = ws.symbols("oo").into_iter().map(|s| s.name).collect();
        assert_eq!(hits, vec!["foo".to_string()]);
        assert_eq!(ws.symbols("").len(), 2, "an empty query returns all");
    }

    #[test]
    fn removing_a_file_drops_its_symbols() {
        let (ws, a, _) = built();
        ws.remove(&a);
        assert!(!ws.declares("foo"));
        assert!(ws.declares("main"));
    }

    #[test]
    fn index_if_absent_does_not_overwrite_an_open_file() {
        let ws = Workspace::new();
        let a = uri("a.dusk");
        ws.index(a.clone(), "func foo() -> int32 {\n    return 0\n}\n");
        // The startup scan reaching an already-open file must not replace it.
        ws.index_if_absent(a.clone(), "func bar() -> int32 {\n    return 0\n}\n");
        assert!(ws.declares("foo"));
        assert!(!ws.declares("bar"));
    }

    #[test]
    fn detail_is_stable_across_files_declaring_a_name() {
        let ws = Workspace::new();
        ws.index(uri("z.dusk"), "func foo() -> int64 {\n    return 0\n}\n");
        ws.index(uri("a.dusk"), "func foo() -> int32 {\n    return 0\n}\n");
        // The smallest URI, a.dusk, wins, every time.
        let d = ws.detail("foo").unwrap();
        assert!(d.contains("-> int32"), "{d}");
    }
}
