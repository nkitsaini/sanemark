//! List normalisation: a minimal-edit counterpart to the reference
//! implementation's remark-stringify list handling.
//!
//! The reference (`linker.ts`) re-serialises the whole document with
//! remark-stringify (`bullet: "-"`, `listItemIndent: "one"`,
//! `incrementListMarker: true`), which normalises list markers as a side effect.
//! We reproduce the *visible* list normalisation as **position-based text
//! edits** — matching this crate's formatter philosophy of leaving untouched
//! text verbatim — namely:
//!
//! * every unordered bullet (`*` / `+`) becomes `-`;
//! * ordered lists are renumbered to increment from their first item's number
//!   (so `1. 1. 1.` → `1. 2. 3.`, while `3. 4.` stays `3. 4.`);
//! * the gap between a marker and its content collapses to a single space
//!   (`-   x` → `- x`, `1.   x` → `1. x`), re-indenting continuation lines and
//!   nested items so the block stays well-formed.
//!
//! Unlike the reference we deliberately *don't* reflow beyond what's needed to
//! keep markers valid: top-level indentation is preserved (no dedenting), the
//! ordinal delimiter (`.` vs `)`) is kept, lazy paragraph continuations are left
//! where they are, and adjacent lists with different bullet characters are not
//! forced apart with a blank line. This mirrors the existing reference-link
//! formatter, which also omits remark-stringify's cosmetic rewrites.

use markdown::mdast::{List, Node, Root};
use markdown::{to_mdast, ParseOptions};

/// A single text replacement over `[start, end)` bytes.
struct Edit {
    start: usize,
    end: usize,
    new: String,
}

/// Normalise every list in `source`, leaving all other text verbatim.
pub fn format_lists(source: &str, gfm: bool) -> String {
    let tree = parse(source, gfm);
    let starts = line_starts(source);
    let mut edits: Vec<Edit> = Vec::new();
    walk(&tree, source, &starts, &mut edits);
    apply_edits(source, edits)
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

/// Byte offset of the start of every line (index 0 is always offset 0).
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Zero-based line index containing byte `offset`.
fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&s| s <= offset).saturating_sub(1)
}

/// Descend the tree looking for *top-level* lists (those not nested inside
/// another list item). Each is handed to [`process_list`], which recurses into
/// its own nested lists; we therefore stop descending once we enter a list.
fn walk(node: &Node, source: &str, starts: &[usize], edits: &mut Vec<Edit>) {
    if let Node::List(list) = node {
        // A top-level list keeps its markers at their original column.
        process_list(list, None, source, starts, edits);
        return;
    }
    if let Some(children) = node.children() {
        for child in children {
            walk(child, source, starts, edits);
        }
    }
}

/// Normalise one list. `target_marker_col`, when set, forces each item's marker
/// to start at that column (used for nested lists so they re-anchor to the
/// parent item's new content column); `None` preserves the original column.
fn process_list(
    list: &List,
    target_marker_col: Option<usize>,
    source: &str,
    starts: &[usize],
    edits: &mut Vec<Edit>,
) {
    let start_number = list.start.unwrap_or(1) as u64;
    let mut ordinal = start_number;
    for child in &list.children {
        let Node::ListItem(item) = child else {
            continue;
        };
        let Some(item_pos) = &item.position else {
            continue;
        };

        let number = if list.ordered { Some(ordinal) } else { None };
        ordinal = ordinal.saturating_add(1);

        let item_start = item_pos.start.offset;
        let marker_line = line_of(starts, item_start);
        let line_start = starts[marker_line];

        // The marker begins at the first non-whitespace byte of the item.
        let marker_start = item_start
            + source[item_start..]
                .bytes()
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count();

        // Parse the marker token and build its canonical replacement.
        let Some((marker_token_end, marker_str)) =
            marker_token(source, marker_start, list.ordered, number)
        else {
            continue;
        };

        let marker_col_new = target_marker_col.unwrap_or(marker_start - line_start);
        let indent_new = " ".repeat(marker_col_new);
        // Content sits one space after the marker (listItemIndent: "one").
        let content_col_new = marker_col_new + marker_str.chars().count() + 1;

        // Whitespace between the marker and whatever follows it (content, or a
        // GFM task checkbox `[ ]`, which must be preserved). We stop at the end
        // of the line, so a bare marker line (content on the next line) yields
        // no following content here.
        let rest_start = marker_token_end
            + source[marker_token_end..]
                .bytes()
                .take_while(|b| *b == b' ' || *b == b'\t')
                .count();
        let content_on_marker_line = source
            .as_bytes()
            .get(rest_start)
            .is_some_and(|b| *b != b'\n' && *b != b'\r');

        // Rewrite the marker plus its trailing gap. When content follows on the
        // same line we normalise the gap to one space (which also applies the
        // indentation shift); otherwise just the marker token is touched.
        let delta = if content_on_marker_line {
            edits.push(Edit {
                start: line_start,
                end: rest_start,
                new: format!("{indent_new}{marker_str} "),
            });
            content_col_new as isize - (rest_start - line_start) as isize
        } else {
            edits.push(Edit {
                start: line_start,
                end: marker_token_end,
                new: format!("{indent_new}{marker_str}"),
            });
            // Content (if any) begins on a later line; align it to the new
            // content column via the continuation-line shift below.
            match item.children.first().and_then(|c| c.position()) {
                Some(p) => {
                    let old = p.start.offset - starts[line_of(starts, p.start.offset)];
                    content_col_new as isize - old as isize
                }
                None => 0,
            }
        };

        // Line indices covered by nested child lists: those are re-indented by
        // recursion, so exclude them from this item's continuation shifting.
        let mut nested_ranges: Vec<(usize, usize)> = Vec::new();
        for c in &item.children {
            if let Node::List(nested) = c {
                if let Some(p) = &nested.position {
                    let s = line_of(starts, p.start.offset);
                    let mut e = line_of(starts, p.end.offset);
                    if p.end.offset > 0 && source.as_bytes()[p.end.offset - 1] == b'\n' && e > s {
                        e -= 1;
                    }
                    nested_ranges.push((s, e));
                }
                process_list(nested, Some(content_col_new), source, starts, edits);
            }
        }

        // Shift the leading whitespace of this item's own continuation lines.
        if delta != 0 {
            let item_end_line = {
                let e = line_of(starts, item_pos.end.offset);
                if item_pos.end.offset > 0
                    && source.as_bytes()[item_pos.end.offset - 1] == b'\n'
                    && e > marker_line
                {
                    e - 1
                } else {
                    e
                }
            };
            for line in (marker_line + 1)..=item_end_line {
                if nested_ranges.iter().any(|(s, e)| line >= *s && line <= *e) {
                    continue;
                }
                shift_line_indent(source, starts, line, delta, edits);
            }
        }
    }
}

/// Parse the marker token starting at `marker_start`, returning the byte offset
/// just past it and its canonical replacement text. For ordered lists the
/// original delimiter (`.` or `)`) is preserved and `number` supplies the new
/// ordinal; unordered markers always become `-`.
fn marker_token(
    source: &str,
    marker_start: usize,
    ordered: bool,
    number: Option<u64>,
) -> Option<(usize, String)> {
    let bytes = source.as_bytes();
    if ordered {
        let mut i = marker_start;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == marker_start || i >= bytes.len() {
            return None;
        }
        let delim = bytes[i];
        if delim != b'.' && delim != b')' {
            return None;
        }
        let n = number.unwrap_or(1);
        Some((i + 1, format!("{n}{}", delim as char)))
    } else {
        match bytes.get(marker_start) {
            Some(b'-') | Some(b'*') | Some(b'+') => Some((marker_start + 1, "-".to_string())),
            _ => None,
        }
    }
}

/// Adjust the leading whitespace of `line` by `delta` columns (clamped at zero),
/// skipping blank lines. New indentation is written as spaces.
fn shift_line_indent(
    source: &str,
    starts: &[usize],
    line: usize,
    delta: isize,
    edits: &mut Vec<Edit>,
) {
    let start = starts[line];
    let end = *starts.get(line + 1).unwrap_or(&source.len());
    let text = &source[start..end];
    let ws_len = text
        .bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count();
    // A line that is only whitespace (blank line) needs no re-indentation.
    if ws_len == text.trim_end_matches(['\n', '\r']).len() {
        return;
    }
    let new_len = (ws_len as isize + delta).max(0) as usize;
    if new_len == ws_len && source[start..start + ws_len].bytes().all(|b| b == b' ') {
        return;
    }
    edits.push(Edit {
        start,
        end: start + ws_len,
        new: " ".repeat(new_len),
    });
}

/// Apply edits right-to-left, skipping any that would overlap an already applied
/// edit. Working backwards keeps earlier byte offsets valid.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(input: &str) -> String {
        format_lists(input, true)
    }

    #[test]
    fn star_and_plus_bullets_become_dash() {
        assert_eq!(fmt("* one\n* two\n* three\n"), "- one\n- two\n- three\n");
        assert_eq!(fmt("+ one\n+ two\n"), "- one\n- two\n");
    }

    #[test]
    fn ordered_lists_are_renumbered_incrementally() {
        assert_eq!(fmt("1. a\n1. b\n1. c\n"), "1. a\n2. b\n3. c\n");
        assert_eq!(fmt("2. a\n5. b\n1. c\n"), "2. a\n3. b\n4. c\n");
    }

    #[test]
    fn ordered_list_start_is_preserved() {
        assert_eq!(fmt("3. start\n4. next\n"), "3. start\n4. next\n");
    }

    #[test]
    fn digit_width_change_reindents() {
        assert_eq!(fmt("9. a\n9. b\n9. c\n"), "9. a\n10. b\n11. c\n");
    }

    #[test]
    fn marker_gap_collapses_to_one_space() {
        assert_eq!(fmt("-   spaced\n-   spaced2\n"), "- spaced\n- spaced2\n");
        assert_eq!(fmt("1.   a\n2.   b\n"), "1. a\n2. b\n");
    }

    #[test]
    fn ordinal_delimiter_is_preserved() {
        assert_eq!(fmt("1) a\n1) b\n"), "1) a\n2) b\n");
    }

    #[test]
    fn nested_markers_reanchor_to_parent() {
        assert_eq!(
            fmt("- top\n  * nested\n  * nested2\n- top2\n"),
            "- top\n  - nested\n  - nested2\n- top2\n"
        );
    }

    #[test]
    fn nested_ordered_is_renumbered() {
        assert_eq!(
            fmt("1. a\n   1. sub\n   2. sub2\n2. b\n"),
            "1. a\n   1. sub\n   2. sub2\n2. b\n"
        );
    }

    #[test]
    fn gap_collapse_reindents_continuation() {
        assert_eq!(
            fmt("-   item\n\n    continuation\n-   item2\n"),
            "- item\n\n  continuation\n- item2\n"
        );
    }

    #[test]
    fn gap_collapse_reindents_deeply_nested() {
        assert_eq!(
            fmt("-   a\n    -   b\n        -   c\n"),
            "- a\n  - b\n    - c\n"
        );
    }

    #[test]
    fn gap_collapse_reindents_code_block() {
        assert_eq!(
            fmt("-   a\n\n    ```\n    code\n    ```\n"),
            "- a\n\n  ```\n  code\n  ```\n"
        );
    }

    #[test]
    fn task_markers_are_left_intact() {
        assert_eq!(fmt("- [ ] todo\n- [x] done\n"), "- [ ] todo\n- [x] done\n");
    }

    #[test]
    fn simple_lists_are_unchanged() {
        let input = "- a\n- b\n- c\n";
        assert_eq!(fmt(input), input);
    }

    #[test]
    fn non_list_text_is_untouched() {
        let input = "# Heading\n\nSome text with 1. not a list here.\n";
        assert_eq!(fmt(input), input);
    }
}
