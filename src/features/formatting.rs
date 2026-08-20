//! Reference-link formatting: a Rust port of the TypeScript `linker.ts`.
//!
//! Two complementary transforms:
//!
//! * [`to_reference_links`] rewrites inline links `[t](url)` into reference
//!   links `[t][id]` and consolidates every used definition under a
//!   `# References` heading at the bottom of the file.
//! * [`to_inline_links`] is the inverse.
//!
//! Unlike the reference implementation (which re-serialises the whole document
//! via remark-stringify), we operate as *position-based text edits* using
//! markdown-rs's byte offsets. Untouched text is preserved verbatim, which
//! keeps diffs minimal and sidesteps the reference's task-marker-escaping
//! workarounds.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use markdown::mdast::{Node, Root};
use markdown::unist::Position as MdPosition;
use markdown::{to_mdast, ParseOptions};

use crate::analysis::node_text;
use crate::config::FormattingConfig;
use crate::features::{lists, tables};

/// Run the full document formatter: table normalisation followed (optionally) by
/// consolidating reference links at the bottom. This is what both
/// `textDocument/formatting` and the `format` CLI run.
pub fn format_document(source: &str, gfm: bool, config: &FormattingConfig) -> String {
    let mut out = if config.format_tables {
        tables::format_tables(source)
    } else {
        source.to_string()
    };
    if config.format_lists {
        out = lists::format_lists(&out, gfm);
    }
    if config.move_references_to_bottom {
        out = to_reference_links(&out, gfm, true, &config.references_heading);
    }
    out
}

/// A single text replacement over `[start, end)` bytes.
struct Edit {
    start: usize,
    end: usize,
    new: String,
}

/// Convert inline links to reference links.
///
/// * `move_to_end` — remove any pre-existing references heading so the section
///   is rebuilt at the bottom.
/// * `heading_text` — heading used for the generated section (e.g. `References`).
pub fn to_reference_links(
    source: &str,
    gfm: bool,
    move_to_end: bool,
    heading_text: &str,
) -> String {
    let tree = parse(source, gfm);
    let nodes = all_nodes(&tree);

    // Existing definitions: id -> url, url -> id, and the set of known ids.
    let mut id_to_url: HashMap<String, String> = HashMap::new();
    let mut url_to_id: HashMap<String, String> = HashMap::new();
    let mut existing_ids: HashSet<String> = HashSet::new();
    let mut edits: Vec<Edit> = Vec::new();

    for node in &nodes {
        if let Node::Definition(d) = node {
            existing_ids.insert(d.identifier.clone());
            id_to_url
                .entry(d.identifier.clone())
                .or_insert_with(|| d.url.clone());
            url_to_id
                .entry(d.url.clone())
                .or_insert_with(|| d.identifier.clone());
            if let Some(p) = &d.position {
                remove_lines(source, p, &mut edits);
            }
        }
    }

    // References already written as `[text][id]` keep their definitions alive.
    let mut used: HashSet<String> = HashSet::new();
    for node in &nodes {
        match node {
            Node::LinkReference(r) => {
                used.insert(r.identifier.clone());
            }
            Node::ImageReference(r) => {
                used.insert(r.identifier.clone());
            }
            _ => {}
        }
    }

    // Inline links -> reference links.
    let mut counter = 1usize;
    for node in &nodes {
        let Node::Link(link) = node else { continue };
        let Some(pos) = &link.position else { continue };
        if is_autolink(node, &link.url) {
            continue;
        }

        let id = match url_to_id.get(&link.url) {
            Some(existing) => existing.clone(),
            None => {
                let id = loop {
                    let candidate = counter.to_string();
                    counter += 1;
                    if !existing_ids.contains(&candidate) {
                        break candidate;
                    }
                };
                existing_ids.insert(id.clone());
                url_to_id.insert(link.url.clone(), id.clone());
                id_to_url.insert(id.clone(), link.url.clone());
                id
            }
        };
        used.insert(id.clone());

        let label_end = label_end_offset(node, pos);
        edits.push(Edit {
            start: label_end,
            end: pos.end.offset,
            new: format!("][{id}]"),
        });
    }

    // Drop the old references heading so it can be rebuilt at the bottom.
    if move_to_end {
        let target = heading_text.trim().to_lowercase();
        for node in &nodes {
            if let Node::Heading(h) = node {
                if node_text(node).trim().to_lowercase() == target {
                    if let Some(p) = &h.position {
                        remove_lines(source, p, &mut edits);
                    }
                }
            }
        }
    }

    let body = apply_edits(source, edits);
    let mut result = body.trim_end().to_string();

    let used_defs = collect_used_defs(&used, &id_to_url);
    if !used_defs.is_empty() {
        result.push_str("\n\n");
        result.push_str(&format!("# {}", heading_text.trim()));
        result.push_str("\n\n");
        for (id, url) in used_defs {
            result.push_str(&format!("[{id}]: {url}\n"));
        }
    }
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Convert reference links back into inline links, dropping the definitions and
/// the references heading.
pub fn to_inline_links(source: &str, gfm: bool, heading_text: &str) -> String {
    let tree = parse(source, gfm);
    let nodes = all_nodes(&tree);
    let id_to_url = definition_urls(&nodes);

    let edits = inline_edits(&nodes, source, &id_to_url, heading_text, |_, _| true);

    let body = apply_edits(source, edits);
    let mut result = body.trim_end().to_string();
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Inline the reference links found within the `[start, end)` byte range,
/// returning **only that (transformed) slice** with its links made inline.
///
/// This is the "copy as inlined Markdown" primitive. Definitions are resolved
/// against the *whole* document, so references whose `[id]: url` definitions
/// live elsewhere (typically at the bottom, under a `# References` heading) are
/// still expanded — the point being to lift a self-contained chunk out of a
/// file that keeps its links as tidy references. Any definition lines or a
/// references heading that happen to fall inside the range are dropped.
///
/// `start`/`end` are byte offsets; they're snapped to the nearest `char`
/// boundary so callers can pass raw editor offsets safely.
pub fn inline_links_in_range(
    source: &str,
    gfm: bool,
    heading_text: &str,
    start: usize,
    end: usize,
) -> String {
    let start = snap_start(source, start.min(source.len()));
    let end = snap_end(source, end.min(source.len())).max(start);

    let tree = parse(source, gfm);
    let nodes = all_nodes(&tree);
    let id_to_url = definition_urls(&nodes);

    // Keep only nodes whose whole span sits inside the selection.
    let within = |s: usize, e: usize| s >= start && e <= end;
    let edits = inline_edits(&nodes, source, &id_to_url, heading_text, within);

    // Re-base each edit onto the slice. Whole-line removals (definitions /
    // heading) may spill past the selection when it cuts mid-line; drop those
    // rather than corrupt the copied text.
    let slice = &source[start..end];
    let translated: Vec<Edit> = edits
        .into_iter()
        .filter(|e| e.start >= start && e.end <= end)
        .map(|e| Edit {
            start: e.start - start,
            end: e.end - start,
            new: e.new,
        })
        .collect();

    apply_edits(slice, translated)
}

/// Map of definition identifier -> URL (first definition wins).
fn definition_urls(nodes: &[&Node]) -> HashMap<String, String> {
    let mut id_to_url: HashMap<String, String> = HashMap::new();
    for node in nodes {
        if let Node::Definition(d) = node {
            id_to_url
                .entry(d.identifier.clone())
                .or_insert_with(|| d.url.clone());
        }
    }
    id_to_url
}

/// Collect the edits that inline reference links (and remove the now-unused
/// definitions / references heading). `keep(start, end)` selects which nodes,
/// by byte span, participate — `|_, _| true` for the whole document, or a range
/// predicate for [`inline_links_in_range`].
fn inline_edits(
    nodes: &[&Node],
    source: &str,
    id_to_url: &HashMap<String, String>,
    heading_text: &str,
    keep: impl Fn(usize, usize) -> bool,
) -> Vec<Edit> {
    let mut edits: Vec<Edit> = Vec::new();

    for node in nodes {
        match node {
            Node::LinkReference(r) => {
                if let (Some(url), Some(pos)) = (id_to_url.get(&r.identifier), &r.position) {
                    if keep(pos.start.offset, pos.end.offset) {
                        let label_end = label_end_offset(node, pos);
                        edits.push(Edit {
                            start: label_end,
                            end: pos.end.offset,
                            new: format!("]({url})"),
                        });
                    }
                }
            }
            Node::ImageReference(r) => {
                if let (Some(url), Some(pos)) = (id_to_url.get(&r.identifier), &r.position) {
                    if keep(pos.start.offset, pos.end.offset) {
                        // Images have no child nodes, so locate the label's
                        // closing `]` by scanning the node's source.
                        let node_end = pos.end.offset.min(source.len());
                        if let Some(rel) = source[pos.start.offset..node_end].find(']') {
                            edits.push(Edit {
                                start: pos.start.offset + rel,
                                end: node_end,
                                new: format!("]({url})"),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    for node in nodes {
        if let Node::Definition(d) = node {
            if let Some(p) = &d.position {
                if keep(p.start.offset, p.end.offset) {
                    remove_lines(source, p, &mut edits);
                }
            }
        }
    }

    let target = heading_text.trim().to_lowercase();
    for node in nodes {
        if let Node::Heading(h) = node {
            if node_text(node).trim().to_lowercase() == target {
                if let Some(p) = &h.position {
                    if keep(p.start.offset, p.end.offset) {
                        remove_lines(source, p, &mut edits);
                    }
                }
            }
        }
    }

    edits
}

/// Round `i` down to the nearest `char` boundary of `s`.
fn snap_start(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Round `i` up to the nearest `char` boundary of `s`.
fn snap_end(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i.min(s.len())
}

fn parse(source: &str, gfm: bool) -> Node {
    let mut opts = if gfm {
        ParseOptions::gfm()
    } else {
        ParseOptions::default()
    };
    opts.constructs.frontmatter = true;
    to_mdast(source, &opts).unwrap_or_else(|_| {
        Node::Root(Root {
            children: Vec::new(),
            position: None,
        })
    })
}

fn all_nodes(root: &Node) -> Vec<&Node> {
    let mut out = Vec::new();
    fn rec<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        out.push(node);
        if let Some(children) = node.children() {
            for c in children {
                rec(c, out);
            }
        }
    }
    rec(root, &mut out);
    out
}

/// True for `<url>` / GFM literal autolinks (a link whose sole child is text
/// equal to the URL) — those are left untouched, matching the reference.
fn is_autolink(node: &Node, url: &str) -> bool {
    if let Some(children) = node.children() {
        if children.len() == 1 {
            if let Node::Text(t) = &children[0] {
                return t.value == url;
            }
        }
    }
    false
}

/// Byte offset of a link's closing `]` (end of the last label child), or just
/// after the opening `[` for an empty label.
fn label_end_offset(node: &Node, pos: &MdPosition) -> usize {
    node.children()
        .and_then(|c| c.last())
        .and_then(|c| c.position())
        .map(|p| p.end.offset)
        .unwrap_or(pos.start.offset + 1)
}

/// Queue removal of the whole line(s) spanned by a node.
fn remove_lines(source: &str, pos: &MdPosition, edits: &mut Vec<Edit>) {
    let (start, end) = line_bounds(source, pos.start.offset, pos.end.offset);
    edits.push(Edit {
        start,
        end,
        new: String::new(),
    });
}

/// Expand `[start, end)` to full-line boundaries (including the trailing
/// newline).
fn line_bounds(src: &str, start: usize, end: usize) -> (usize, usize) {
    let start = start.min(src.len());
    let end = end.min(src.len());
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = src[end..]
        .find('\n')
        .map(|i| end + i + 1)
        .unwrap_or(src.len());
    (line_start, line_end)
}

/// Apply edits right-to-left, skipping any that would overlap an already applied
/// edit. Because we only ever mutate text at or after each edit's start (working
/// backwards), earlier byte offsets stay valid.
fn apply_edits(source: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by(|a, b| b.start.cmp(&a.start).then(b.end.cmp(&a.end)));
    let mut out = source.to_string();
    let mut boundary = out.len();
    for e in edits {
        if e.start > e.end || e.end > boundary {
            continue;
        }
        out.replace_range(e.start..e.end, &e.new);
        boundary = e.start;
    }
    out
}

fn collect_used_defs(
    used: &HashSet<String>,
    id_to_url: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut defs: Vec<(String, String)> = used
        .iter()
        .filter_map(|id| id_to_url.get(id).map(|url| (id.clone(), url.clone())))
        .collect();
    defs.sort_by(|a, b| cmp_id(&a.0, &b.0));
    defs
}

/// Numeric ids sort first (by value), then named ids alphabetically.
fn cmp_id(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}
