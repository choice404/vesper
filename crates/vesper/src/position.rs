//! Position conversion between the compiler and the protocol. The dusk compiler
//! reports spans as byte offsets into a file. The Language Server Protocol wants
//! a line and a character, where the character counts UTF-16 code units by
//! default. This module bridges the two.

use tower_lsp::lsp_types::Position;

/// A line map for one source string: the byte offset each line starts at. A
/// byte offset finds its line by binary search, then its column by counting
/// UTF-16 code units from the line start.
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    /// Builds the line map for `text`.
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push((i + 1) as u32);
            }
        }
        LineIndex { starts }
    }

    /// Maps a byte offset into `text` to a protocol position. An offset past the
    /// end of the text clamps to the end, and an offset that lands inside a
    /// multibyte character snaps back to that character's start, so a span the
    /// lexer reports mid character never panics.
    pub fn position(&self, text: &str, offset: u32) -> Position {
        let mut offset = (offset as usize).min(text.len());
        while offset > 0 && !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = match self.starts.binary_search(&(offset as u32)) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let line_start = self.starts[line] as usize;
        let col16: u32 = text[line_start..offset]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        Position::new(line as u32, col16)
    }

    /// Maps a protocol position back to a byte offset into `text`. A line or
    /// character past the content clamps to the nearest boundary, so an out of
    /// range request never panics.
    pub fn offset(&self, text: &str, pos: Position) -> u32 {
        let line = pos.line as usize;
        if line >= self.starts.len() {
            return text.len() as u32;
        }
        let line_start = self.starts[line] as usize;
        let line_end = if line + 1 < self.starts.len() {
            // Drop the newline that ends the line, so a column never lands on it.
            self.starts[line + 1] as usize - 1
        } else {
            text.len()
        };
        let mut col16 = 0u32;
        let mut off = line_start;
        for c in text[line_start..line_end].chars() {
            if col16 >= pos.character {
                break;
            }
            col16 += c.len_utf16() as u32;
            off += c.len_utf8();
        }
        off as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_offsets_on_a_single_line() {
        let text = "func main";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 0), Position::new(0, 0));
        assert_eq!(index.position(text, 5), Position::new(0, 5));
    }

    #[test]
    fn maps_offsets_across_lines() {
        let text = "ab\ncd\n";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 0), Position::new(0, 0));
        assert_eq!(index.position(text, 3), Position::new(1, 0));
        assert_eq!(index.position(text, 4), Position::new(1, 1));
    }

    #[test]
    fn counts_columns_in_utf16_code_units() {
        // The e with an acute accent is one UTF-16 unit but two UTF-8 bytes, so
        // the byte offset after it is 3 while the column is 2.
        let text = "café";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 5), Position::new(0, 4));
        // Byte offset 3 sits just after the accented letter.
        let after_e = "caf".len() as u32 + "é".len() as u32;
        assert_eq!(index.position(text, after_e), Position::new(0, 4));
    }

    #[test]
    fn a_position_round_trips_to_its_offset() {
        let text = "one\ntwo\nthree";
        let index = LineIndex::new(text);
        for off in [0u32, 2, 4, 7, 8, 12] {
            let pos = index.position(text, off);
            assert_eq!(index.offset(text, pos), off, "offset {off} did not round trip");
        }
    }

    #[test]
    fn an_offset_inside_a_character_does_not_panic() {
        // The lexer can report a span whose end lands between the two bytes of a
        // multibyte character, as it does for a bad escape before an accented
        // letter. The offset must snap back to the character start.
        let text = "é";
        let index = LineIndex::new(text);
        assert_eq!(index.position(text, 1), Position::new(0, 0));
        assert_eq!(index.position(text, 2), Position::new(0, 1));
    }

    #[test]
    fn out_of_range_positions_clamp() {
        let text = "hi\n";
        let index = LineIndex::new(text);
        assert_eq!(index.offset(text, Position::new(99, 0)), text.len() as u32);
        assert_eq!(index.offset(text, Position::new(0, 99)), 2);
    }
}
