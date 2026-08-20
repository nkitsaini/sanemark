//! Parse a document into an owned, position-resolved summary.
//!
//! We parse with [`markdown`] (markdown-rs), whose mdast mirrors the remark AST
//! used by the reference formatter and, crucially, exposes byte-accurate
//! offsets for every node. The resulting [`Analysis`] is config-independent and
//! cached per document version; individual features derive their results from it
//! plus the live [`Config`](crate::config::Config).

use markdown::mdast::{Node, Root};
use markdown::unist::Position as MdPosition;
use markdown::{to_mdast, ParseOptions};

/// Kinds of foldable block structures we recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    List,
    ListItem,
    Code,
    BlockQuote,
    Table,
    FrontMatter,
}

/// A heading, used for section folding and the document outline.
#[derive(Debug, Clone)]
pub struct HeadingInfo {
    /// Heading level, 1-6.
    pub level: u8,
    /// Plain-text content of the heading.
    pub text: String,
    /// Zero-based line of the heading.
    pub start_line: usize,
    /// Byte range of the whole heading node.
    pub start_byte: usize,
    pub end_byte: usize,
}

/// A foldable block with its zero-based line span.
#[derive(Debug, Clone)]
pub struct BlockInfo {
    pub kind: BlockKind,
    pub start_line: usize,
    pub end_line: usize,
}

/// A link/image/definition destination with the byte span of the URL itself.
#[derive(Debug, Clone)]
pub struct LinkTarget {
    pub url: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub is_image: bool,
    /// Byte span of the *whole* link/image node (e.g. `[text](url)`), used for
    /// go-to-definition matching when the cursor is anywhere inside the link.
    pub node_start: usize,
    pub node_end: usize,
}

/// A reference-style usage: `[text][id]`, `[text][]` or the shortcut `[id]`.
#[derive(Debug, Clone)]
pub struct RefUsage {
    /// Normalised link identifier (matches the corresponding [`DefInfo`]).
    pub identifier: String,
    /// Byte span of the whole reference node.
    pub start_byte: usize,
    pub end_byte: usize,
    pub is_image: bool,
}

/// A link reference definition: `[id]: url "title"`.
#[derive(Debug, Clone)]
pub struct DefInfo {
    /// Normalised definition identifier.
    pub identifier: String,
    pub url: String,
    /// Byte span of the whole definition node.
    pub start_byte: usize,
    pub end_byte: usize,
    /// Byte span of the destination (URL) within the definition.
    pub url_start: usize,
    pub url_end: usize,
}

/// Everything the features need, derived once per document version.
#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub version: i32,
    pub headings: Vec<HeadingInfo>,
    pub blocks: Vec<BlockInfo>,
    pub link_targets: Vec<LinkTarget>,
    pub references: Vec<RefUsage>,
    pub definitions: Vec<DefInfo>,
}

/// Parse options: always a GFM + front-matter superset. Analysis is used only
/// for read-only features (folding, diagnostics, symbols, hover), so parsing the
/// widest grammar is safe regardless of the `gfm` config flag (which only
/// affects the formatter's autolink handling).
pub fn parse_options() -> ParseOptions {
    let mut opts = ParseOptions::gfm();
    opts.constructs.frontmatter = true;
    opts
}

/// Parse `text` and build an [`Analysis`] tagged with `version`.
pub fn analyze(text: &str, version: i32) -> Analysis {
    let tree = to_mdast(text, &parse_options()).unwrap_or_else(|_| {
        Node::Root(Root {
            children: Vec::new(),
            position: None,
        })
    });

    let mut analysis = Analysis {
        version,
        ..Default::default()
    };
    walk(&tree, text, &mut analysis);
    analysis.headings.sort_by_key(|h| h.start_line);
    analysis
}

/// Convert an mdast [`MdPosition`] into a zero-based `[start_line, end_line]`
/// span. When a block ends exactly at the start of the following line (column
/// 1), we pull the end back so we don't fold a trailing blank line.
fn line_range(pos: &MdPosition) -> (usize, usize) {
    let start = pos.start.line.saturating_sub(1);
    let mut end = pos.end.line.saturating_sub(1);
    if pos.end.column == 1 && end > start {
        end -= 1;
    }
    (start, end)
}

fn walk(node: &Node, text: &str, out: &mut Analysis) {
    collect(node, text, out);
    if let Some(children) = node.children() {
        for child in children {
            walk(child, text, out);
        }
    }
}

fn collect(node: &Node, text: &str, out: &mut Analysis) {
    match node {
        Node::Heading(h) => {
            if let Some(pos) = &h.position {
                out.headings.push(HeadingInfo {
                    level: h.depth,
                    text: node_text(node),
                    start_line: line_range(pos).0,
                    start_byte: pos.start.offset,
                    end_byte: pos.end.offset,
                });
            }
        }
        Node::List(l) => push_block(&l.position, BlockKind::List, out),
        Node::ListItem(l) => push_block(&l.position, BlockKind::ListItem, out),
        Node::Code(c) => push_block(&c.position, BlockKind::Code, out),
        Node::Blockquote(b) => push_block(&b.position, BlockKind::BlockQuote, out),
        Node::Table(t) => push_block(&t.position, BlockKind::Table, out),
        Node::Yaml(y) => push_block(&y.position, BlockKind::FrontMatter, out),
        Node::Toml(t) => push_block(&t.position, BlockKind::FrontMatter, out),
        Node::Link(link) => {
            if let Some(pos) = &link.position {
                let (start_byte, end_byte) = url_span(text, pos, &link.url, label_end(node), true);
                out.link_targets.push(LinkTarget {
                    url: link.url.clone(),
                    start_byte,
                    end_byte,
                    is_image: false,
                    node_start: pos.start.offset,
                    node_end: pos.end.offset,
                });
            }
        }
        Node::Image(img) => {
            if let Some(pos) = &img.position {
                let (start_byte, end_byte) = url_span(text, pos, &img.url, None, true);
                out.link_targets.push(LinkTarget {
                    url: img.url.clone(),
                    start_byte,
                    end_byte,
                    is_image: true,
                    node_start: pos.start.offset,
                    node_end: pos.end.offset,
                });
            }
        }
        Node::Definition(def) => {
            if let Some(pos) = &def.position {
                let (url_start, url_end) = url_span(text, pos, &def.url, None, true);
                out.link_targets.push(LinkTarget {
                    url: def.url.clone(),
                    start_byte: url_start,
                    end_byte: url_end,
                    is_image: false,
                    node_start: pos.start.offset,
                    node_end: pos.end.offset,
                });
                out.definitions.push(DefInfo {
                    identifier: def.identifier.clone(),
                    url: def.url.clone(),
                    start_byte: pos.start.offset,
                    end_byte: pos.end.offset,
                    url_start,
                    url_end,
                });
            }
        }
        Node::LinkReference(r) => {
            if let Some(pos) = &r.position {
                out.references.push(RefUsage {
                    identifier: r.identifier.clone(),
                    start_byte: pos.start.offset,
                    end_byte: pos.end.offset,
                    is_image: false,
                });
            }
        }
        Node::ImageReference(r) => {
            if let Some(pos) = &r.position {
                out.references.push(RefUsage {
                    identifier: r.identifier.clone(),
                    start_byte: pos.start.offset,
                    end_byte: pos.end.offset,
                    is_image: true,
                });
            }
        }
        _ => {}
    }
}

fn push_block(pos: &Option<MdPosition>, kind: BlockKind, out: &mut Analysis) {
    if let Some(pos) = pos {
        let (start_line, end_line) = line_range(pos);
        out.blocks.push(BlockInfo {
            kind,
            start_line,
            end_line,
        });
    }
}

/// Byte offset just after a link's label (the position of the closing `]`),
/// derived from the last child's end offset. `None` for nodes without children.
fn label_end(node: &Node) -> Option<usize> {
    node.children()
        .and_then(|c| c.last())
        .and_then(|c| c.position())
        .map(|p| p.end.offset)
}

/// Best-effort byte span of a URL inside a node. We search for the (already
/// parsed) `url` string within the node's source slice, preferring the last
/// occurrence and, when known, starting after the label. Falls back to the
/// whole node if the URL can't be located (e.g. it contained escapes).
fn url_span(
    text: &str,
    pos: &MdPosition,
    url: &str,
    search_from: Option<usize>,
    prefer_last: bool,
) -> (usize, usize) {
    let node_start = pos.start.offset;
    let node_end = pos.end.offset.min(text.len());
    let from = search_from
        .unwrap_or(node_start)
        .clamp(node_start, node_end);
    if url.is_empty() || from >= node_end {
        return (node_start, node_end);
    }
    let slice = &text[from..node_end];
    let rel = if prefer_last {
        slice.rfind(url)
    } else {
        slice.find(url)
    };
    match rel {
        Some(r) => (from + r, from + r + url.len()),
        None => (node_start, node_end),
    }
}

/// Concatenate the visible text of a node (text + inline code).
pub fn node_text(node: &Node) -> String {
    let mut out = String::new();
    collect_text(node, &mut out);
    out
}

fn collect_text(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => out.push_str(&t.value),
        Node::InlineCode(c) => out.push_str(&c.value),
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_text(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_have_levels_and_text() {
        let a = analyze("# Title\n\n## Sub\n", 1);
        assert_eq!(a.headings.len(), 2);
        assert_eq!(a.headings[0].level, 1);
        assert_eq!(a.headings[0].text, "Title");
        assert_eq!(a.headings[1].level, 2);
        assert_eq!(a.headings[1].text, "Sub");
    }

    #[test]
    fn code_block_is_a_block() {
        let a = analyze("```rust\nfn main() {}\n```\n", 1);
        assert!(a.blocks.iter().any(|b| b.kind == BlockKind::Code));
    }

    #[test]
    fn link_url_span_points_at_url() {
        let text = "see [x](./a.md) here";
        let a = analyze(text, 1);
        assert_eq!(a.link_targets.len(), 1);
        let t = &a.link_targets[0];
        assert_eq!(&text[t.start_byte..t.end_byte], "./a.md");
    }

    #[test]
    fn definition_url_span() {
        let text = "[id]: ./target.md\n";
        let a = analyze(text, 1);
        let t = a
            .link_targets
            .iter()
            .find(|t| t.url == "./target.md")
            .unwrap();
        assert_eq!(&text[t.start_byte..t.end_byte], "./target.md");
    }

    #[test]
    fn captures_reference_usages_and_definitions() {
        let text = "See [one][a] and [two][a].\n\n[a]: https://example.com\n";
        let a = analyze(text, 1);
        assert_eq!(a.references.len(), 2);
        assert!(a.references.iter().all(|r| r.identifier == "a"));
        assert_eq!(a.definitions.len(), 1);
        let def = &a.definitions[0];
        assert_eq!(def.identifier, "a");
        assert_eq!(def.url, "https://example.com");
        assert_eq!(&text[def.url_start..def.url_end], "https://example.com");
    }

    #[test]
    fn link_node_span_covers_whole_link() {
        let text = "see [x](./a.md) here";
        let a = analyze(text, 1);
        let t = &a.link_targets[0];
        assert_eq!(&text[t.node_start..t.node_end], "[x](./a.md)");
    }
}
