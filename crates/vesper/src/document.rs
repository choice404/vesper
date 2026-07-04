//! An open document and its line map.

use crate::position::LineIndex;

/// A file the client has opened: its full text, a line map for position
/// conversion, and the version the client last sent. Vesper keeps the whole
/// text and rebuilds the map on each change, which is cheap for the file sizes
/// dusk programs run to.
pub struct Document {
    pub text: String,
    pub index: LineIndex,
    pub version: i32,
}

impl Document {
    pub fn new(text: String, version: i32) -> Self {
        let index = LineIndex::new(&text);
        Document {
            text,
            index,
            version,
        }
    }
}
