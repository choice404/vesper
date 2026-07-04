//! The set of open documents, keyed by URI.

use dashmap::DashMap;
use tower_lsp::lsp_types::Url;

use crate::document::Document;

/// A concurrent map of open documents. The protocol handlers run on many tasks,
/// so the store hands out cloned text rather than a borrow held across an await.
#[derive(Default)]
pub struct Store {
    docs: DashMap<Url, Document>,
}

impl Store {
    pub fn new() -> Self {
        Store::default()
    }

    /// Records a newly opened document.
    pub fn open(&self, uri: Url, text: String, version: i32) {
        self.docs.insert(uri, Document::new(text, version));
    }

    /// Replaces a document's text after a full sync change.
    pub fn update(&self, uri: Url, text: String, version: i32) {
        self.docs.insert(uri, Document::new(text, version));
    }

    /// Drops a document the client closed.
    pub fn close(&self, uri: &Url) {
        self.docs.remove(uri);
    }

    /// The current text of an open document, if it is open.
    pub fn text(&self, uri: &Url) -> Option<String> {
        self.docs.get(uri).map(|d| d.text.clone())
    }

    /// The text and version of an open document, taken together so a later
    /// publish can check whether it is still current.
    pub fn snapshot(&self, uri: &Url) -> Option<(String, i32)> {
        self.docs.get(uri).map(|d| (d.text.clone(), d.version))
    }

    /// The version the client last sent for a document, if it is open.
    pub fn version(&self, uri: &Url) -> Option<i32> {
        self.docs.get(uri).map(|d| d.version)
    }
}
