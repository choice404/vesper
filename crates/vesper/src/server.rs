//! The Language Server Protocol backend. It keeps the open documents, answers
//! the lifecycle requests, and publishes diagnostics as files open, change, and
//! save.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::compiler;
use crate::config::Config;
use crate::store::Store;
use crate::workspace::Workspace;

/// The server state shared across every request task.
pub struct Backend {
    client: Client,
    store: Arc<Store>,
    /// The semantic diagnostics from the last open or save, per document. The
    /// syntax pass runs on every edit, but the semantic pass needs the file on
    /// disk, so between saves the cached set is merged back in. Without it a
    /// name or type error would vanish on the first keystroke and only return on
    /// the next save.
    semantic: Arc<DashMap<Url, Vec<Diagnostic>>>,
    /// The project wide index that backs references, workspace symbols, and cross
    /// file definitions.
    workspace: Arc<Workspace>,
    /// The workspace root directories, captured at initialize and walked once the
    /// client signals it is ready.
    roots: Mutex<Vec<PathBuf>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            store: Arc::new(Store::new()),
            semantic: Arc::new(DashMap::new()),
            workspace: Arc::new(Workspace::new()),
            roots: Mutex::new(Vec::new()),
        }
    }

    /// Reindexes a file in the workspace from its current buffer, so references
    /// and symbols follow edits without waiting for a save.
    fn reindex(&self, uri: &Url) {
        if let Some(text) = self.store.text(uri) {
            self.workspace.index(uri.clone(), &text);
        }
    }

    /// Recomputes and publishes diagnostics for one file.
    ///
    /// The syntax pass always runs, on the in-memory buffer. When `run_semantic`
    /// is set and the file has a path on disk, the semantic pass runs too, its
    /// diagnostics for this file replace the cached set, and it joins the
    /// publish. Otherwise the cached semantic set joins instead, so an error
    /// from the last save stays visible while the buffer is edited.
    ///
    /// The document version is captured up front and checked again before every
    /// publish. A slow semantic load can outlast a fresh edit, and this makes
    /// the stale result step aside for the newer one rather than overwrite it.
    async fn refresh(&self, uri: Url, run_semantic: bool) {
        let Some((text, version)) = self.store.snapshot(&uri) else {
            return;
        };
        let mut diagnostics = compiler::syntax_diagnostics(&text);

        let semantic = if run_semantic {
            self.run_semantic(&uri).await
        } else {
            self.semantic.get(&uri).map(|e| e.clone()).unwrap_or_default()
        };
        diagnostics.extend(semantic);

        // The semantic pass yields at an await, so a newer edit may have landed.
        // Let that edit's refresh publish instead of overwriting it with a set
        // built from older text and tagged with an older version.
        if self.superseded(&uri, version) {
            return;
        }
        self.client
            .publish_diagnostics(uri, diagnostics, Some(version))
            .await;
    }

    /// Runs the semantic pass on a background thread and returns this file's own
    /// diagnostics, updating the cache so later syntax only publishes can merge
    /// them back in. A document with no path on disk has no semantic pass and
    /// yields an empty set.
    async fn run_semantic(&self, uri: &Url) -> Vec<Diagnostic> {
        let Some(path) = uri
            .to_file_path()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
        else {
            return Vec::new();
        };
        let for_task = path.clone();
        let per_file =
            tokio::task::spawn_blocking(move || compiler::semantic_diagnostics(&for_task))
                .await
                .unwrap_or_default();
        let mut mine = Vec::new();
        for file in per_file {
            if file.path == path {
                mine.extend(file.diagnostics);
            }
        }
        self.semantic.insert(uri.clone(), mine.clone());
        mine
    }

    /// True when the document has been edited past the version a refresh started
    /// from, so its result should be dropped in favor of the newer refresh.
    fn superseded(&self, uri: &Url, version: i32) -> bool {
        self.store.version(uri) != Some(version)
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        *self.roots.lock().unwrap() = workspace_roots(&params);
        Config::from_value(params.initialization_options).apply();

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "vesper".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                // Advertise open, change, and save explicitly, so every client
                // sends the save notification the semantic pass depends on.
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: compiler::legend(),
                            range: Some(false),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Walk the workspace off the async runtime and index every dusk file, so
        // references and workspace symbols work before a file is even opened.
        let roots = self.roots.lock().unwrap().clone();
        let workspace = self.workspace.clone();
        let indexed = tokio::task::spawn_blocking(move || {
            let mut count = 0usize;
            for root in &roots {
                for path in dusk_files(root) {
                    if let (Ok(text), Ok(uri)) =
                        (std::fs::read_to_string(&path), Url::from_file_path(&path))
                    {
                        // Do not clobber a file an editor already opened and
                        // indexed from a newer buffer.
                        workspace.index_if_absent(uri, &text);
                        count += 1;
                    }
                }
            }
            count
        })
        .await
        .unwrap_or(0);

        self.client
            .log_message(MessageType::INFO, format!("vesper ready, indexed {indexed} files"))
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let Some(text) = self.store.text(&params.text_document.uri) else {
            return Ok(None);
        };
        let data = compiler::semantic_tokens(&text);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let pos = params.text_document_position_params;
        let Some(text) = self.store.text(&pos.text_document.uri) else {
            return Ok(None);
        };
        // The file's own declarations and builtin types answer first, precisely.
        if let Some(hover) = compiler::hover(&text, pos.position) {
            return Ok(Some(hover));
        }
        // Otherwise fall back to a declaration elsewhere in the workspace, so a
        // call into another file still shows a signature.
        let Some((name, range)) = compiler::name_at(&text, pos.position) else {
            return Ok(None);
        };
        Ok(self.workspace.detail(&name).map(|detail| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```dusk\n{detail}\n```"),
            }),
            range: Some(range),
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params;
        let uri = pos.text_document.uri;
        let Some(text) = self.store.text(&uri) else {
            return Ok(None);
        };
        let Some((name, _)) = compiler::name_at(&text, pos.position) else {
            return Ok(None);
        };
        // The workspace index holds this file too, so its own definitions are
        // covered along with every other file's.
        let defs = self.workspace.definitions(&name);
        Ok(match defs.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(defs.into_iter().next().unwrap())),
            _ => Some(GotoDefinitionResponse::Array(defs)),
        })
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let pos = params.text_document_position;
        let uri = pos.text_document.uri;
        let Some(text) = self.store.text(&uri) else {
            return Ok(None);
        };
        let Some((name, _)) = compiler::name_at(&text, pos.position) else {
            return Ok(None);
        };
        // A name the workspace declares is a global and ranges over every file; a
        // name it does not is likely a local, so it stays in this one.
        let restrict = if self.workspace.declares(&name) {
            None
        } else {
            Some(&uri)
        };
        let mut refs = self.workspace.references(&name, restrict);
        // Honor the client's request to leave declarations out of the results.
        if !params.context.include_declaration {
            let defs = self.workspace.definitions(&name);
            refs.retain(|r| {
                !defs
                    .iter()
                    .any(|d| d.uri == r.uri && d.range == r.range)
            });
        }
        Ok(Some(refs))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        Ok(Some(self.workspace.symbols(&params.query)))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let Some(text) = self.store.text(&params.text_document.uri) else {
            return Ok(None);
        };
        let symbols = compiler::document_symbols(&text)
            .into_iter()
            .map(to_document_symbol)
            .collect();
        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.store.open(doc.uri.clone(), doc.text, doc.version);
        self.reindex(&doc.uri);
        self.refresh(doc.uri, true).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // Full sync sends the whole document as the last change, so the last text
        // wins.
        let Some(change) = params.content_changes.into_iter().last() else {
            return;
        };
        let uri = params.text_document.uri;
        self.store
            .update(uri.clone(), change.text, params.text_document.version);
        self.reindex(&uri);
        self.refresh(uri, false).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        self.reindex(&uri);
        self.refresh(uri, true).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.store.close(&uri);
        self.semantic.remove(&uri);
        // The buffer is gone, so the index must reflect the file on disk again,
        // or drop the file if it no longer exists. Otherwise abandoned edits would
        // haunt references and definitions for the rest of the session.
        match uri
            .to_file_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
        {
            Some(text) => self.workspace.index(uri.clone(), &text),
            None => self.workspace.remove(&uri),
        }
        // Clear anything the editor still shows for a file it no longer tracks.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

/// The workspace root directories, preferring the folders the client lists and
/// falling back to the single root uri older clients send.
#[allow(deprecated)]
fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(folders) = &params.workspace_folders {
        for folder in folders {
            if let Ok(path) = folder.uri.to_file_path() {
                roots.push(path);
            }
        }
    }
    if roots.is_empty() {
        if let Some(path) = params.root_uri.as_ref().and_then(|u| u.to_file_path().ok()) {
            roots.push(path);
        }
    }
    roots
}

/// Every `.dusk` file under a directory.
fn dusk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_dusk(root, &mut out);
    out
}

/// Walks a directory for dusk files, skipping version control and build output
/// so a large tree does not stall startup.
fn collect_dusk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Use the entry's own type, which does not follow symlinks, so a symlink
        // cycle cannot send the walk into infinite recursion.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            let skip = matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some(".git") | Some("target") | Some("node_modules") | Some(".dawn")
            );
            if !skip {
                collect_dusk(&path, out);
            }
        } else if file_type.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("dusk")
        {
            out.push(path);
        }
    }
}

/// Converts a vesper symbol into the protocol's document symbol. The name range
/// stands in for the full range too, since the tree does not span a whole
/// declaration.
fn to_document_symbol(symbol: compiler::Symbol) -> DocumentSymbol {
    #[allow(deprecated)]
    DocumentSymbol {
        name: symbol.name,
        detail: Some(symbol.detail),
        kind: symbol.kind,
        tags: None,
        deprecated: None,
        range: symbol.name_range,
        selection_range: symbol.name_range,
        children: None,
    }
}
