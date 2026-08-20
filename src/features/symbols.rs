//! Document outline from headings (`textDocument/documentSymbol`).

use ropey::Rope;
use tower_lsp_server::ls_types::{DocumentSymbol, Range, SymbolKind};

use crate::analysis::{Analysis, HeadingInfo};
use crate::encoding::{range_from_bytes, PositionEncoding};

/// Build a nested outline where deeper headings become children of the nearest
/// shallower heading above them.
pub fn document_symbols(
    analysis: &Analysis,
    rope: &Rope,
    enc: PositionEncoding,
) -> Vec<DocumentSymbol> {
    let mut roots: Vec<DocumentSymbol> = Vec::new();
    // Stack of (level, index-path) into the tree being built.
    let mut stack: Vec<u8> = Vec::new();

    for h in &analysis.headings {
        let symbol = make_symbol(h, rope, enc);
        // Pop until the top of the stack is a strictly shallower heading.
        while stack.last().is_some_and(|&lvl| lvl >= h.level) {
            stack.pop();
        }
        insert_at(&mut roots, &stack, symbol);
        stack.push(h.level);
    }

    roots
}

fn make_symbol(h: &HeadingInfo, rope: &Rope, enc: PositionEncoding) -> DocumentSymbol {
    let range: Range = range_from_bytes(rope, h.start_byte, h.end_byte, enc);
    let name = if h.text.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        h.text.clone()
    };
    #[allow(deprecated)]
    DocumentSymbol {
        name,
        detail: Some(format!("H{}", h.level)),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: Some(Vec::new()),
    }
}

/// Descend `stack.len()` levels into `roots` (always taking the last child) and
/// push `symbol` there.
fn insert_at(roots: &mut Vec<DocumentSymbol>, stack: &[u8], symbol: DocumentSymbol) {
    let mut current = roots;
    for _ in stack {
        let last = current
            .last_mut()
            .expect("stack depth matches tree depth by construction");
        current = last.children.get_or_insert_with(Vec::new);
    }
    current.push(symbol);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    #[test]
    fn nests_by_level() {
        let text = "# A\n\n## B\n\n## C\n\n# D\n";
        let a = analyze(text, 1);
        let rope = Rope::from_str(text);
        let syms = document_symbols(&a, &rope, PositionEncoding::Utf16);
        assert_eq!(syms.len(), 2); // A, D
        assert_eq!(syms[0].name, "A");
        let a_children = syms[0].children.as_ref().unwrap();
        assert_eq!(a_children.len(), 2); // B, C
        assert_eq!(a_children[0].name, "B");
        assert_eq!(syms[1].name, "D");
    }
}
