//! Behavioural tests for the reference-link formatter, ported from the
//! TypeScript reference suite (`reference/linker.test.ts`).
//!
//! Expected outputs are our *canonical position-edit* results: untouched text
//! is preserved verbatim, so they intentionally omit remark-stringify artifacts
//! (autolink `<...>` wrapping, task-marker escaping, blank-line reflowing).

use sanemark::config::FormattingConfig;
use sanemark::features::formatting::{
    format_document, inline_links_in_range, to_inline_links, to_reference_links,
};

fn refs(input: &str) -> String {
    to_reference_links(input, true, true, "References")
}

fn inline(input: &str) -> String {
    to_inline_links(input, true, "References")
}

// ---------------------------------------------------------------------------
// Inline links -> reference links
// ---------------------------------------------------------------------------

#[test]
fn simple_inline_to_reference() {
    let out = refs("Here is a [link](https://google.com) to Google.");
    assert_eq!(
        out,
        "Here is a [link][1] to Google.\n\n# References\n\n[1]: https://google.com\n"
    );
}

#[test]
fn duplicate_urls_reuse_id() {
    let out = refs("[Google](https://google.com) and [Google Again](https://google.com).");
    assert_eq!(
        out,
        "[Google][1] and [Google Again][1].\n\n# References\n\n[1]: https://google.com\n"
    );
}

#[test]
fn preserves_existing_and_appends_new() {
    let input =
        "[Existing](https://existing.com) and [New](https://new.com).\n\n[Existing]: https://existing.com";
    assert_eq!(
        refs(input),
        "[Existing][existing] and [New][1].\n\n# References\n\n[1]: https://new.com\n[existing]: https://existing.com\n"
    );
}

#[test]
fn preserves_layout_without_links() {
    let input = "# Heading 1\n\nSome text.\n\n- List item 1\n- List item 2";
    assert_eq!(
        refs(input),
        "# Heading 1\n\nSome text.\n\n- List item 1\n- List item 2\n"
    );
}

#[test]
fn consolidates_references_at_bottom() {
    let input = "# Intro\n\n[Link](https://a.com)\n\n# References\n\n[old]: https://b.com";
    assert_eq!(
        refs(input),
        "# Intro\n\n[Link][1]\n\n# References\n\n[1]: https://a.com\n"
    );
}

#[test]
fn multiple_links_in_paragraph() {
    let input =
        "Check out [Google](https://google.com) and [GitHub](https://github.com) for more info.";
    assert_eq!(
        refs(input),
        "Check out [Google][1] and [GitHub][2] for more info.\n\n# References\n\n[1]: https://google.com\n[2]: https://github.com\n"
    );
}

#[test]
fn keeps_only_used_references() {
    let input =
        "Check [this][ref1] and [that](https://new.com).\n\n[ref1]: https://ref1.com\n[ref2]: https://ref2.com\n[ref3]: https://ref3.com";
    assert_eq!(
        refs(input),
        "Check [this][ref1] and [that][1].\n\n# References\n\n[1]: https://new.com\n[ref1]: https://ref1.com\n"
    );
}

#[test]
fn removes_all_references_when_no_links() {
    let input = "Just plain text here.\n\n# References\n\n[old1]: https://old1.com\n[old2]: https://old2.com";
    assert_eq!(refs(input), "Just plain text here.\n");
}

#[test]
fn task_items_are_untouched() {
    let input =
        "# My Tasks\n\n- [ ] My task\n- [x] Completed task\n- [ ] Another task with a [link](https://example.com)";
    assert_eq!(
        refs(input),
        "# My Tasks\n\n- [ ] My task\n- [x] Completed task\n- [ ] Another task with a [link][1]\n\n# References\n\n[1]: https://example.com\n"
    );
}

#[test]
fn autolinks_are_left_as_is() {
    // A bare URL is not rewritten; only the real inline link becomes a reference.
    let input = "\n\nhttps://example.com\n[Should be referenced](https://example2.com)\n";
    assert_eq!(
        refs(input),
        "\n\nhttps://example.com\n[Should be referenced][1]\n\n# References\n\n[1]: https://example2.com\n"
    );
}

#[test]
fn preserves_list_blank_line_spacing() {
    let input = "# Random scratch pad\n\n1. Hi\n2. Hi\n\n3. [Hi][1]\n\n# References\n[1]: /file";
    assert_eq!(
        refs(input),
        "# Random scratch pad\n\n1. Hi\n2. Hi\n\n3. [Hi][1]\n\n# References\n\n[1]: /file\n"
    );
}

#[test]
fn reference_without_definition_is_left_intact() {
    // Unlike remark (which escapes these), we leave dangling references as-is.
    let input =
        "Check [this][ref1] and [that](https://new.com).\n\nSome text mentioning [another][ref2] reference.";
    assert_eq!(
        refs(input),
        "Check [this][ref1] and [that][1].\n\nSome text mentioning [another][ref2] reference.\n\n# References\n\n[1]: https://new.com\n"
    );
}

// ---------------------------------------------------------------------------
// Full-document formatting: list normalisation (matches the reference's
// remark-stringify list handling) alongside reference consolidation.
// ---------------------------------------------------------------------------

#[test]
fn format_document_normalises_list_markers() {
    let cfg = FormattingConfig::default();
    let out = format_document("- one\n- two\n\n1. a\n1. b\n", true, &cfg);
    assert_eq!(out, "- one\n- two\n\n1. a\n2. b\n");
}

#[test]
fn format_document_normalises_lists_then_moves_references() {
    let cfg = FormattingConfig::default();
    let input = "* item with a [link](https://a.com)\n*   spaced item\n";
    let out = format_document(input, true, &cfg);
    assert_eq!(
        out,
        "- item with a [link][1]\n- spaced item\n\n# References\n\n[1]: https://a.com\n"
    );
}

#[test]
fn format_document_can_disable_list_formatting() {
    let cfg = FormattingConfig {
        format_lists: false,
        move_references_to_bottom: false,
        ..FormattingConfig::default()
    };
    let input = "* one\n* two\n";
    assert_eq!(format_document(input, true, &cfg), input);
}

// ---------------------------------------------------------------------------
// Reference links -> inline links
// ---------------------------------------------------------------------------

#[test]
fn inline_simple() {
    let input = "Here is a [link][1] to Google.\n\n# References\n\n[1]: https://google.com\n";
    assert_eq!(
        inline(input),
        "Here is a [link](https://google.com) to Google.\n"
    );
}

#[test]
fn inline_named_and_multiple() {
    let input = "Check out [Google][1] and [GitHub][2] for more info.\n\n# References\n\n[1]: https://google.com\n[2]: https://github.com\n";
    assert_eq!(
        inline(input),
        "Check out [Google](https://google.com) and [GitHub](https://github.com) for more info.\n"
    );
}

#[test]
fn inline_shortcut_references() {
    let input =
        "Check out [Google] and [GitHub].\n\n# References\n\n[Google]: https://google.com\n[GitHub]: https://github.com\n";
    assert_eq!(
        inline(input),
        "Check out [Google](https://google.com) and [GitHub](https://github.com).\n"
    );
}

#[test]
fn inline_collapsed_references() {
    let input =
        "Check out [Google][] and [GitHub][].\n\n# References\n\n[Google]: https://google.com\n[GitHub]: https://github.com\n";
    assert_eq!(
        inline(input),
        "Check out [Google](https://google.com) and [GitHub](https://github.com).\n"
    );
}

#[test]
fn inline_mixed_reference_and_inline() {
    let input = "Check [inline](https://inline.com) and [reference][1].\n\n# References\n\n[1]: https://reference.com\n";
    assert_eq!(
        inline(input),
        "Check [inline](https://inline.com) and [reference](https://reference.com).\n"
    );
}

// ---------------------------------------------------------------------------
// Round trips
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_simple() {
    let original = "Here is a [link](https://google.com) to Google.";
    let reverted = inline(&refs(original));
    assert_eq!(reverted.trim(), original);
}

#[test]
fn roundtrip_complex() {
    let original = "# Introduction\n\nThis is a [test](https://test.com) document.\n\n## Section 1\n\nHere's [Google](https://google.com) again.";
    let reverted = inline(&refs(original));
    assert_eq!(reverted.trim(), original);
}

// ---------------------------------------------------------------------------
// Range inlining ("copy as inlined Markdown")
// ---------------------------------------------------------------------------

#[test]
fn inline_range_expands_only_the_selection() {
    // References live at the bottom, outside the selection, but must still be
    // resolved so the copied chunk is self-contained.
    let doc = "Intro [one][1] and [two][2].\n\nTail [three][1].\n\n# References\n\n[1]: https://one.com\n[2]: https://two.com\n";
    let start = doc.find("Intro").unwrap();
    let end = doc.find(".\n\n").unwrap() + 1; // through the first period
    let out = inline_links_in_range(doc, true, "References", start, end);
    assert_eq!(
        out,
        "Intro [one](https://one.com) and [two](https://two.com)."
    );
}

#[test]
fn inline_range_handles_images() {
    let doc = "![alt][img] here.\n\n[img]: ./p.png\n";
    let end = doc.find(".\n").unwrap() + 1;
    let out = inline_links_in_range(doc, true, "References", 0, end);
    assert_eq!(out, "![alt](./p.png) here.");
}

#[test]
fn inline_range_leaves_plain_selection_untouched() {
    let doc = "No links here.\n\n[1]: https://x.com\n";
    let end = doc.find(".\n").unwrap() + 1;
    assert_eq!(
        inline_links_in_range(doc, true, "References", 0, end),
        "No links here."
    );
}

#[test]
fn inline_range_over_whole_doc_drops_definitions() {
    let doc = "See [x][1].\n\n# References\n\n[1]: https://x.com\n";
    let out = inline_links_in_range(doc, true, "References", 0, doc.len());
    assert!(out.contains("[x](https://x.com)"), "got: {out:?}");
    assert!(!out.contains("[1]:"), "definition should be gone: {out:?}");
    assert!(
        !out.contains("# References"),
        "heading should be gone: {out:?}"
    );
}
