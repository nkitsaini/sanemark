//! In-memory document representation with incremental editing.

use ropey::Rope;
use tower_lsp_server::ls_types::TextDocumentContentChangeEvent;

use crate::encoding::{position_to_char, PositionEncoding};

/// A single open text document: its content (as a [`Rope`] for cheap edits) and
/// the last version number reported by the client.
#[derive(Debug, Clone)]
pub struct Document {
    pub rope: Rope,
    pub version: i32,
}

impl Document {
    pub fn new(text: &str, version: i32) -> Self {
        Self {
            rope: Rope::from_str(text),
            version,
        }
    }

    /// The full document text as a `String`.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Apply a single incremental (or full) content change from the client.
    ///
    /// A change with a `range` is treated as a replacement of that range; a
    /// change without a range replaces the whole document.
    pub fn apply_change(&mut self, change: &TextDocumentContentChangeEvent, enc: PositionEncoding) {
        match change.range {
            Some(range) => {
                let start =
                    position_to_char(&self.rope, range.start, enc).min(self.rope.len_chars());
                let end = position_to_char(&self.rope, range.end, enc)
                    .min(self.rope.len_chars())
                    .max(start);
                self.rope.remove(start..end);
                self.rope.insert(start, &change.text);
            }
            None => {
                self.rope = Rope::from_str(&change.text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::{Position, Range};

    fn change(range: Option<Range>, text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range,
            range_length: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn incremental_insert() {
        let mut doc = Document::new("hello world", 1);
        let r = Range::new(Position::new(0, 5), Position::new(0, 5));
        doc.apply_change(&change(Some(r), ","), PositionEncoding::Utf16);
        assert_eq!(doc.text(), "hello, world");
    }

    #[test]
    fn incremental_replace() {
        let mut doc = Document::new("hello world", 1);
        let r = Range::new(Position::new(0, 6), Position::new(0, 11));
        doc.apply_change(&change(Some(r), "there"), PositionEncoding::Utf16);
        assert_eq!(doc.text(), "hello there");
    }

    #[test]
    fn full_replace() {
        let mut doc = Document::new("old", 1);
        doc.apply_change(&change(None, "brand new"), PositionEncoding::Utf16);
        assert_eq!(doc.text(), "brand new");
    }

    #[test]
    fn multiline_delete() {
        let mut doc = Document::new("a\nb\nc\n", 1);
        let r = Range::new(Position::new(0, 1), Position::new(2, 0));
        doc.apply_change(&change(Some(r), ""), PositionEncoding::Utf16);
        assert_eq!(doc.text(), "ac\n");
    }
}
