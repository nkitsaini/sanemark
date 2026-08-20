//! Conversions between LSP [`Position`]s and offsets into a [`Rope`].
//!
//! LSP positions are `(line, character)` pairs where the meaning of
//! `character` depends on the negotiated *position encoding*: UTF-16 code units
//! (the protocol default) or UTF-8 bytes. All conversions here are driven by a
//! [`PositionEncoding`] so the rest of the server never has to think about it.
//!
//! Everything is clamped rather than panicking, because clients occasionally
//! send positions that are slightly out of range (e.g. at the very end of a
//! document during rapid edits).

use ropey::Rope;
use tower_lsp_server::ls_types::{Position, Range};

/// The negotiated position encoding for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEncoding {
    /// `character` counts UTF-8 bytes.
    Utf8,
    /// `character` counts UTF-16 code units (the LSP default).
    Utf16,
}

/// Convert an LSP [`Position`] into a character index into `rope`.
pub fn position_to_char(rope: &Rope, pos: Position, enc: PositionEncoding) -> usize {
    let line = pos.line as usize;
    if line >= rope.len_lines() {
        return rope.len_chars();
    }
    let line_start_char = rope.line_to_char(line);
    let line_slice = rope.line(line);

    match enc {
        PositionEncoding::Utf8 => {
            let line_start_byte = rope.line_to_byte(line);
            let max_byte = line_start_byte + line_slice.len_bytes();
            let target_byte = (line_start_byte + pos.character as usize).min(max_byte);
            rope.byte_to_char(target_byte)
        }
        PositionEncoding::Utf16 => {
            let line_start_u16 = rope.char_to_utf16_cu(line_start_char);
            let line_end_char = line_start_char + line_slice.len_chars();
            let line_end_u16 = rope.char_to_utf16_cu(line_end_char);
            let target_u16 = (line_start_u16 + pos.character as usize).min(line_end_u16);
            rope.utf16_cu_to_char(target_u16)
        }
    }
}

/// Convert a character index into `rope` back into an LSP [`Position`].
pub fn char_to_position(rope: &Rope, char_idx: usize, enc: PositionEncoding) -> Position {
    let char_idx = char_idx.min(rope.len_chars());
    let line = rope.char_to_line(char_idx);
    let line_start_char = rope.line_to_char(line);

    let character = match enc {
        PositionEncoding::Utf8 => rope.char_to_byte(char_idx) - rope.char_to_byte(line_start_char),
        PositionEncoding::Utf16 => {
            rope.char_to_utf16_cu(char_idx) - rope.char_to_utf16_cu(line_start_char)
        }
    };

    Position::new(line as u32, character as u32)
}

/// Convert a byte offset into `rope` into an LSP [`Position`].
pub fn byte_to_position(rope: &Rope, byte: usize, enc: PositionEncoding) -> Position {
    let byte = byte.min(rope.len_bytes());
    let char_idx = rope.byte_to_char(byte);
    char_to_position(rope, char_idx, enc)
}

/// Build an LSP [`Range`] from a `[start, end)` byte span into `rope`.
pub fn range_from_bytes(rope: &Rope, start: usize, end: usize, enc: PositionEncoding) -> Range {
    Range::new(
        byte_to_position(rope, start, enc),
        byte_to_position(rope, end, enc),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_roundtrip() {
        let rope = Rope::from_str("hello\nworld\n");
        let pos = Position::new(1, 3);
        let c = position_to_char(&rope, pos, PositionEncoding::Utf16);
        assert_eq!(rope.char(c), 'l'); // "wor|ld"
        assert_eq!(char_to_position(&rope, c, PositionEncoding::Utf16), pos);
    }

    #[test]
    fn multibyte_utf16_vs_utf8() {
        // "😀" is one char, 2 UTF-16 code units, 4 UTF-8 bytes.
        let rope = Rope::from_str("a😀b");
        // The 'b' is after the emoji.
        let b_char = 2;
        let p16 = char_to_position(&rope, b_char, PositionEncoding::Utf16);
        assert_eq!(p16, Position::new(0, 3)); // a(1) + 😀(2) = 3
        let p8 = char_to_position(&rope, b_char, PositionEncoding::Utf8);
        assert_eq!(p8, Position::new(0, 5)); // a(1) + 😀(4) = 5

        assert_eq!(
            position_to_char(&rope, p16, PositionEncoding::Utf16),
            b_char
        );
        assert_eq!(position_to_char(&rope, p8, PositionEncoding::Utf8), b_char);
    }

    #[test]
    fn crlf_lines() {
        let rope = Rope::from_str("a\r\nb\r\n");
        let pos = Position::new(1, 0);
        let c = position_to_char(&rope, pos, PositionEncoding::Utf16);
        assert_eq!(rope.char(c), 'b');
    }

    #[test]
    fn out_of_range_is_clamped() {
        let rope = Rope::from_str("hi\n");
        let pos = Position::new(99, 99);
        let c = position_to_char(&rope, pos, PositionEncoding::Utf16);
        assert_eq!(c, rope.len_chars());
    }
}
