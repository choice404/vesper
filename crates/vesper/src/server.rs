//! The Language Server Protocol backend. It keeps the open documents, answers
//! the lifecycle requests, and publishes diagnostics as files open, change, and
//! save.

use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::compiler;
use crate::config::Config;
use crate::store::Store;

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
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            store: Arc::new(Store::new()),
            semantic: Arc::new(DashMap::new()),
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
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "vesper ready")
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
        Ok(compiler::hover(&text, pos.position))
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
        Ok(compiler::definition(&text, pos.position)
            .map(|range| GotoDefinitionResponse::Scalar(Location::new(uri, range))))
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
        self.refresh(uri, false).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.refresh(params.text_document.uri, true).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.store.close(&uri);
        self.semantic.remove(&uri);
        // Clear anything the editor still shows for a file it no longer tracks.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
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
