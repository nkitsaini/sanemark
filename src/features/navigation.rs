//! Go-to-definition and find-references for Markdown links.
//!
//! Two related navigations, both derived from the cached [`Analysis`]:
//!
//! * **Definition** (`textDocument/definition`):
//!   - on a file link/image (`[t](./a.md)`) → jump to that file (resolving a
//!     `#heading` fragment to the heading's line when possible);
//!   - on a reference link (`[t][id]` / `[id]`) → jump to its `[id]: url`
//!     definition;
//!   - on a `#anchor` link → jump to the matching heading in this document.
//! * **References** (`textDocument/references`):
//!   - on a reference id or its definition → every `[..][id]` usage (and,
//!     optionally, the definition itself);
//!   - on a file link → every other link in this document pointing at the same
//!     file.

use std::path::{Path, PathBuf};

use ropey::Rope;
use tower_lsp_server::ls_types::{Location, Position, Range, Uri};

use crate::analysis::Analysis;
use crate::encoding::{position_to_char, range_from_bytes, PositionEncoding};
use crate::links;
use crate::uri;

/// Per-request document context shared by the navigation functions.
pub struct NavContext<'a> {
    pub rope: &'a Rope,
    pub enc: PositionEncoding,
    pub doc_uri: &'a Uri,
    pub doc_path: Option<&'a Path>,
    pub workspace_root: Option<&'a Path>,
}

/// Resolve go-to-definition for the symbol under `position`.
pub fn goto_definition(
    analysis: &Analysis,
    position: Position,
    ctx: &NavContext,
) -> Option<Vec<Location>> {
    let byte = cursor_byte(ctx.rope, position, ctx.enc);

    // 1. A reference usage jumps to its definition line.
    if let Some(r) = analysis
        .references
        .iter()
        .find(|r| within(byte, r.start_byte, r.end_byte))
    {
        let def = analysis
            .definitions
            .iter()
            .find(|d| d.identifier == r.identifier)?;
        let range = range_from_bytes(ctx.rope, def.start_byte, def.end_byte, ctx.enc);
        return Some(vec![Location::new(ctx.doc_uri.clone(), range)]);
    }

    // 2. A link/image/definition destination jumps to the target file/anchor.
    let target = analysis
        .link_targets
        .iter()
        .find(|t| within(byte, t.node_start, t.node_end))?;

    resolve_url(&target.url, analysis, ctx).map(|loc| vec![loc])
}

/// Find all references to the symbol under `position` (within this document).
pub fn references(
    analysis: &Analysis,
    position: Position,
    ctx: &NavContext,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let byte = cursor_byte(ctx.rope, position, ctx.enc);

    // Reference usage -> all usages of that identifier.
    if let Some(r) = analysis
        .references
        .iter()
        .find(|r| within(byte, r.start_byte, r.end_byte))
    {
        return Some(identifier_locations(
            analysis,
            ctx,
            &r.identifier,
            include_declaration,
        ));
    }

    // Definition: on the URL, treat as a file reference; otherwise as the id.
    if let Some(d) = analysis
        .definitions
        .iter()
        .find(|d| within(byte, d.start_byte, d.end_byte))
    {
        if within(byte, d.url_start, d.url_end) {
            if let Some(locs) = file_references(&d.url, analysis, ctx) {
                return Some(locs);
            }
        }
        return Some(identifier_locations(
            analysis,
            ctx,
            &d.identifier,
            include_declaration,
        ));
    }

    // A file link -> all links in this document pointing at the same file.
    if let Some(t) = analysis
        .link_targets
        .iter()
        .find(|t| within(byte, t.node_start, t.node_end))
    {
        return file_references(&t.url, analysis, ctx);
    }

    None
}

/// Locations of every `[..][id]` usage, plus the definition when requested.
fn identifier_locations(
    analysis: &Analysis,
    ctx: &NavContext,
    identifier: &str,
    include_declaration: bool,
) -> Vec<Location> {
    let mut locs = Vec::new();
    for r in analysis
        .references
        .iter()
        .filter(|r| r.identifier == identifier)
    {
        let range = range_from_bytes(ctx.rope, r.start_byte, r.end_byte, ctx.enc);
        locs.push(Location::new(ctx.doc_uri.clone(), range));
    }
    if include_declaration {
        for d in analysis
            .definitions
            .iter()
            .filter(|d| d.identifier == identifier)
        {
            let range = range_from_bytes(ctx.rope, d.start_byte, d.end_byte, ctx.enc);
            locs.push(Location::new(ctx.doc_uri.clone(), range));
        }
    }
    locs
}

/// Locations of every link/definition in the document that resolves to the same
/// local file as `url`.
fn file_references(url: &str, analysis: &Analysis, ctx: &NavContext) -> Option<Vec<Location>> {
    let wanted = resolve_path(url, ctx.doc_path, ctx.workspace_root)?;
    let mut locs = Vec::new();
    for t in &analysis.link_targets {
        if resolve_path(&t.url, ctx.doc_path, ctx.workspace_root).as_deref()
            == Some(wanted.as_path())
        {
            let range = range_from_bytes(ctx.rope, t.start_byte, t.end_byte, ctx.enc);
            locs.push(Location::new(ctx.doc_uri.clone(), range));
        }
    }
    if locs.is_empty() {
        None
    } else {
        Some(locs)
    }
}

/// Turn a link destination into a jump [`Location`].
fn resolve_url(url: &str, analysis: &Analysis, ctx: &NavContext) -> Option<Location> {
    let url = url.trim();

    // Pure same-document anchor.
    if let Some(anchor) = url.strip_prefix('#') {
        let line = heading_line(analysis, anchor)?;
        return Some(Location::new(
            ctx.doc_uri.clone(),
            line_range(ctx.rope, line, ctx.enc),
        ));
    }

    let resolved = resolve_path(url, ctx.doc_path, ctx.workspace_root)?;
    if !resolved.exists() {
        return None;
    }
    let target_uri = uri::from_path(&resolved)?;

    // A `file#anchor` fragment jumps to that heading inside the target file.
    let fragment = url.split_once('#').map(|(_, frag)| frag).unwrap_or("");
    let mut range = Range::new(Position::new(0, 0), Position::new(0, 0));
    if !fragment.is_empty() {
        if let Ok(text) = std::fs::read_to_string(&resolved) {
            let target_analysis = crate::analysis::analyze(&text, 0);
            if let Some(line) = heading_line(&target_analysis, fragment) {
                let target_rope = Rope::from_str(&text);
                range = line_range(&target_rope, line, ctx.enc);
            }
        }
    }
    Some(Location::new(target_uri, range))
}

/// Resolve a link destination to a normalized filesystem path (or `None` for
/// external/anchor destinations).
fn resolve_path(
    url: &str,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Option<PathBuf> {
    let local = links::local_target(url)?;
    let base = if local.starts_with('/') {
        workspace_root
    } else {
        doc_path.and_then(Path::parent)
    }?;
    Some(uri::normalize(&base.join(local.trim_start_matches('/'))))
}

/// Line (0-based) of the heading whose slug matches `anchor`, if any.
fn heading_line(analysis: &Analysis, anchor: &str) -> Option<usize> {
    let wanted = slug(anchor);
    analysis
        .headings
        .iter()
        .find(|h| slug(&h.text) == wanted)
        .map(|h| h.start_line)
}

/// A zero-width [`Range`] at the start of `line`.
fn line_range(rope: &Rope, line: usize, enc: PositionEncoding) -> Range {
    let line = line.min(rope.len_lines().saturating_sub(1));
    let byte = rope.line_to_byte(line);
    range_from_bytes(rope, byte, byte, enc)
}

/// GitHub-style heading slug: lowercase, drop punctuation, spaces to hyphens.
fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in text.trim().chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            prev_dash = false;
        } else if c == '_' {
            out.push('_');
            prev_dash = false;
        } else if (c.is_whitespace() || c == '-') && !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn cursor_byte(rope: &Rope, position: Position, enc: PositionEncoding) -> usize {
    let char_idx = position_to_char(rope, position, enc);
    rope.char_to_byte(char_idx)
}

fn within(byte: usize, start: usize, end: usize) -> bool {
    byte >= start && byte < end
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use std::fs;
    use tempfile::tempdir;

    fn uri_for(path: &Path) -> Uri {
        uri::from_path(path).unwrap()
    }

    fn pos_of(text: &str, needle: &str) -> Position {
        let idx = text.find(needle).unwrap();
        let line = text[..idx].matches('\n').count();
        let col = idx - text[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        Position::new(line as u32, col as u32)
    }

    #[test]
    fn definition_jumps_to_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "# hi\n").unwrap();
        let doc = dir.path().join("index.md");
        let text = "see [a](./a.md) here";
        fs::write(&doc, text).unwrap();

        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc_uri = uri_for(&doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(&doc),
            workspace_root: Some(dir.path()),
        };
        let locs = goto_definition(&analysis, pos_of(text, "a.md"), &ctx).unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, uri_for(&dir.path().join("a.md")));
    }

    #[test]
    fn definition_resolves_fragment_to_heading() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.md"),
            "# Intro\n\n## My Section\n\ntext\n",
        )
        .unwrap();
        let doc = dir.path().join("index.md");
        let text = "see [a](./a.md#my-section)";
        fs::write(&doc, text).unwrap();

        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc_uri = uri_for(&doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(&doc),
            workspace_root: Some(dir.path()),
        };
        let locs = goto_definition(&analysis, pos_of(text, "a.md"), &ctx).unwrap();
        // "## My Section" is on line 2 (0-based) of a.md.
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn reference_link_jumps_to_definition() {
        let text = "See [one][a] and more.\n\n[a]: https://example.com\n";
        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc = Path::new("/tmp/x.md");
        let doc_uri = uri_for(doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(doc),
            workspace_root: None,
        };
        let locs = goto_definition(&analysis, pos_of(text, "[one]"), &ctx).unwrap();
        // Definition is on line 2.
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn anchor_jumps_within_document() {
        let text = "[go](#my-heading)\n\n# My Heading\n\ntext\n";
        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc = Path::new("/tmp/x.md");
        let doc_uri = uri_for(doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(doc),
            workspace_root: None,
        };
        let locs = goto_definition(&analysis, pos_of(text, "#my-heading"), &ctx).unwrap();
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn references_of_identifier() {
        let text = "[one][a] and [two][a].\n\n[a]: https://example.com\n";
        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc = Path::new("/tmp/x.md");
        let doc_uri = uri_for(doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(doc),
            workspace_root: None,
        };
        let locs = references(&analysis, pos_of(text, "[one]"), &ctx, true).unwrap();
        // Two usages + the declaration.
        assert_eq!(locs.len(), 3);
    }

    #[test]
    fn references_of_file_link() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        let doc = dir.path().join("index.md");
        let text = "[first](./a.md) then [second](./a.md) and [other](./b.md)";
        fs::write(&doc, text).unwrap();

        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        let doc_uri = uri_for(&doc);
        let ctx = NavContext {
            rope: &rope,
            enc: PositionEncoding::Utf16,
            doc_uri: &doc_uri,
            doc_path: Some(&doc),
            workspace_root: Some(dir.path()),
        };
        let locs = references(&analysis, pos_of(text, "./a.md"), &ctx, true).unwrap();
        // Both links to a.md, not the one to b.md.
        assert_eq!(locs.len(), 2);
    }
}
