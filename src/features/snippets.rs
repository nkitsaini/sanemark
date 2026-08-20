//! Quick "slash / at" command completions.
//!
//! Typing a trigger character (`/` or `@` by default) at a word boundary —
//! start of line or right after whitespace — opens a small menu of insertable
//! snippets, in the spirit of the `/`-commands in Notion / Slack:
//!
//! * **date & time** — `now` (`HH:mm`), `today` / `tomorrow` / `yesterday`
//!   (`YYYY-MM-DD`) and `datetime`; formats are configurable via
//!   [`SnippetsConfig`];
//! * **file links** — every workspace file, inserted as an inline link
//!   `[stem](path)` (the same deep, fuzzy, workspace-wide search that powers
//!   path completion, so `/rea` surfaces `[readme](./docs/readme.md)`).
//!
//! Accepting an item replaces the trigger and everything typed after it, so
//! `/now` becomes `14:30` and `/read` becomes the chosen link.
//!
//! Like [`super::completion`], detection is driven by the text on the current
//! line rather than the AST (the trigger + query aren't valid Markdown yet), so
//! this stays a pure function of `(rope, position, config, now)` and is fully
//! unit-testable without a clock or an LSP connection.

use std::path::Path;

use chrono::format::{Item, StrftimeItems};
use chrono::{Duration, NaiveDateTime};
use ropey::Rope;
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Position, Range, TextEdit,
};

use crate::config::{Config, SnippetsConfig};
use crate::encoding::{char_to_position, position_to_char, PositionEncoding};
use crate::features::completion;

/// The parsed "we are typing a trigger command" context.
struct CommandCtx {
    /// The trigger character that opened the menu (`/` or `@`).
    trigger: char,
    /// The word typed after the trigger (may be empty).
    query: String,
    /// Number of characters to replace: the trigger plus [`Self::query`].
    replace_len: usize,
}

/// A ready-to-insert date/time snippet.
struct DateSnippet {
    /// Menu label (also the fuzzy-match target), e.g. `now`.
    label: &'static str,
    /// One-line description shown next to the label.
    detail: String,
    /// The text inserted when accepted.
    insert: String,
}

/// Produce the `@`/`/` quick-command items for the cursor position, given the
/// current local wall-clock time `now`.
pub fn complete(
    rope: &Rope,
    position: Position,
    enc: PositionEncoding,
    config: &Config,
    now: NaiveDateTime,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Vec<CompletionItem> {
    let snippets = &config.snippets;
    if !snippets.enabled {
        return Vec::new();
    }

    let cursor_char = position_to_char(rope, position, enc);
    let line_idx = rope.char_to_line(cursor_char);
    let line_start_char = rope.line_to_char(line_idx);
    let before: String = rope.slice(line_start_char..cursor_char).chars().collect();

    let ctx = match detect_command(&before, snippets) {
        Some(ctx) => ctx,
        None => return Vec::new(),
    };

    let replace_range = Range::new(
        char_to_position(rope, cursor_char.saturating_sub(ctx.replace_len), enc),
        position,
    );

    // A running rank keeps date snippets before file links while preserving the
    // best-first order each group is already in.
    let mut rank = 0usize;
    let mut items: Vec<CompletionItem> = Vec::new();

    for snip in date_snippets(&now, snippets) {
        if matches_query(&ctx.query, snip.label) {
            items.push(date_item(snip, ctx.trigger, &replace_range, rank));
            rank += 1;
        }
    }

    if snippets.file_links {
        for hit in
            completion::workspace_files(&ctx.query, &config.completion, doc_path, workspace_root)
        {
            items.push(file_item(hit, ctx.trigger, &replace_range, rank));
            rank += 1;
        }
    }

    items
}

/// Detect a trigger command at the end of `before` (the line up to the cursor).
///
/// A command is `<trigger><query>` where `<trigger>` is one of the configured
/// characters sitting at a word boundary (start of line or after whitespace)
/// and `<query>` is a run of "word" characters. This deliberately ignores
/// triggers embedded in words (`and/or`, `a@b.com`) and inside link
/// destinations (`](./`), which path completion already handles.
fn detect_command(before: &str, cfg: &SnippetsConfig) -> Option<CommandCtx> {
    let chars: Vec<char> = before.chars().collect();

    // Peel off the trailing query characters.
    let mut i = chars.len();
    while i > 0 && is_query_char(chars[i - 1]) {
        i -= 1;
    }
    if i == 0 {
        return None;
    }

    let trigger = chars[i - 1];
    if !cfg.triggers.iter().any(|t| t.starts_with(trigger)) {
        return None;
    }

    // The trigger must start a token: at line start or after whitespace.
    let boundary = i < 2 || chars[i - 2].is_whitespace();
    if !boundary {
        return None;
    }

    Some(CommandCtx {
        trigger,
        query: chars[i..].iter().collect(),
        replace_len: chars.len() - (i - 1),
    })
}

fn is_query_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Build the date/time snippet list for `now`.
fn date_snippets(now: &NaiveDateTime, cfg: &SnippetsConfig) -> Vec<DateSnippet> {
    let time = fmt_or(now, &cfg.time_format, "%H:%M");
    let today = fmt_or(now, &cfg.date_format, "%Y-%m-%d");
    let datetime = fmt_or(now, &cfg.date_time_format, "%Y-%m-%d %H:%M");
    let tomorrow = fmt_or(&(*now + Duration::days(1)), &cfg.date_format, "%Y-%m-%d");
    let yesterday = fmt_or(&(*now - Duration::days(1)), &cfg.date_format, "%Y-%m-%d");

    vec![
        DateSnippet {
            label: "now",
            detail: format!("Current time — {time}"),
            insert: time,
        },
        DateSnippet {
            label: "today",
            detail: format!("Today's date — {today}"),
            insert: today,
        },
        DateSnippet {
            label: "datetime",
            detail: format!("Date & time — {datetime}"),
            insert: datetime,
        },
        DateSnippet {
            label: "tomorrow",
            detail: format!("Tomorrow's date — {tomorrow}"),
            insert: tomorrow,
        },
        DateSnippet {
            label: "yesterday",
            detail: format!("Yesterday's date — {yesterday}"),
            insert: yesterday,
        },
    ]
}

/// Format `dt` with `fmt`, falling back to `fallback` when `fmt` contains an
/// invalid `strftime` specifier (so a bad config never panics the request).
fn fmt_or(dt: &NaiveDateTime, fmt: &str, fallback: &str) -> String {
    if StrftimeItems::new(fmt).any(|it| matches!(it, Item::Error)) {
        return dt.format(fallback).to_string();
    }
    dt.format(fmt).to_string()
}

/// Case-insensitive subsequence match (fzf-style) of `query` against `label`.
fn matches_query(query: &str, label: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let label_lower = label.to_lowercase();
    let mut hay = label_lower.chars();
    'next: for qc in query.to_lowercase().chars() {
        for hc in hay.by_ref() {
            if hc == qc {
                continue 'next;
            }
        }
        return false;
    }
    true
}

fn date_item(snip: DateSnippet, trigger: char, range: &Range, rank: usize) -> CompletionItem {
    CompletionItem {
        label: snip.label.to_string(),
        kind: Some(CompletionItemKind::SNIPPET),
        detail: Some(snip.detail),
        sort_text: Some(format!("{rank:04}")),
        // Include the trigger so clients that re-filter against the buffer text
        // (which still contains the `/`) keep the item as the user types.
        filter_text: Some(format!("{trigger}{}", snip.label)),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: *range,
            new_text: snip.insert,
        })),
        ..Default::default()
    }
}

fn file_item(
    hit: completion::FileHit,
    trigger: char,
    range: &Range,
    rank: usize,
) -> CompletionItem {
    let stem = link_text(&hit.name);
    let insert = format!("[{stem}]({})", hit.display);
    CompletionItem {
        label: hit.display.clone(),
        kind: Some(CompletionItemKind::FILE),
        detail: Some(format!("Link — {insert}")),
        sort_text: Some(format!("{rank:04}")),
        filter_text: Some(format!("{trigger}{}", hit.display)),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: *range,
            new_text: insert,
        })),
        ..Default::default()
    }
}

/// Link text for a file: its name without the final extension
/// (`notes.md` → `notes`), falling back to the whole name.
fn link_text(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn dt(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mm, 0)
            .unwrap()
    }

    fn items_at(
        text: &str,
        cursor: u32,
        now: NaiveDateTime,
        doc: Option<&Path>,
        root: Option<&Path>,
    ) -> Vec<CompletionItem> {
        items_with(text, cursor, now, doc, root, Config::default())
    }

    fn items_with(
        text: &str,
        cursor: u32,
        now: NaiveDateTime,
        doc: Option<&Path>,
        root: Option<&Path>,
        config: Config,
    ) -> Vec<CompletionItem> {
        let rope = Rope::from_str(text);
        complete(
            &rope,
            Position::new(0, cursor),
            PositionEncoding::Utf16,
            &config,
            now,
            doc,
            root,
        )
    }

    fn inserted<'a>(items: &'a [CompletionItem], label: &str) -> &'a str {
        let item = items
            .iter()
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no item labelled {label:?}"));
        match item.text_edit.as_ref().unwrap() {
            CompletionTextEdit::Edit(e) => &e.new_text,
            _ => panic!("expected a plain edit"),
        }
    }

    #[test]
    fn slash_offers_date_and_time_snippets() {
        let items = items_at("/", 1, dt(2026, 7, 7, 14, 30), None, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"now"));
        assert!(labels.contains(&"today"));
        assert!(labels.contains(&"datetime"));
        assert_eq!(inserted(&items, "now"), "14:30");
        assert_eq!(inserted(&items, "today"), "2026-07-07");
        assert_eq!(inserted(&items, "datetime"), "2026-07-07 14:30");
    }

    #[test]
    fn at_sign_also_triggers() {
        let items = items_at("@", 1, dt(2026, 7, 7, 9, 5), None, None);
        assert!(items.iter().any(|i| i.label == "now"));
        assert_eq!(inserted(&items, "now"), "09:05");
    }

    #[test]
    fn tomorrow_and_yesterday_shift_the_date() {
        let items = items_at("/", 1, dt(2026, 7, 7, 0, 0), None, None);
        assert_eq!(inserted(&items, "tomorrow"), "2026-07-08");
        assert_eq!(inserted(&items, "yesterday"), "2026-07-06");
    }

    #[test]
    fn query_filters_snippets() {
        // "/tod" should keep "today" but drop "now".
        let items = items_at("/tod", 4, dt(2026, 7, 7, 14, 30), None, None);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"today"));
        assert!(!labels.contains(&"now"));
    }

    #[test]
    fn replace_range_covers_trigger_and_query() {
        // Accepting must replace "/now" (4 chars), not just append.
        let items = items_at("/now", 4, dt(2026, 7, 7, 14, 30), None, None);
        let now = items.iter().find(|i| i.label == "now").unwrap();
        let CompletionTextEdit::Edit(edit) = now.text_edit.as_ref().unwrap() else {
            panic!();
        };
        assert_eq!(edit.range.start, Position::new(0, 0));
        assert_eq!(edit.range.end, Position::new(0, 4));
    }

    #[test]
    fn no_trigger_inside_a_word() {
        // "and/or" — the slash is mid-word, so no menu.
        assert!(items_at("and/or", 6, dt(2026, 7, 7, 14, 30), None, None).is_empty());
        // Email-like "a@b" — the "@" is mid-word.
        assert!(items_at("a@b", 3, dt(2026, 7, 7, 14, 30), None, None).is_empty());
    }

    #[test]
    fn trigger_after_whitespace_is_ok() {
        let items = items_at("- /", 3, dt(2026, 7, 7, 14, 30), None, None);
        assert!(items.iter().any(|i| i.label == "now"));
    }

    #[test]
    fn file_links_are_offered_as_inline_links() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("readme.md"), "").unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs/guide.md"), "").unwrap();
        let doc = dir.path().join("index.md");

        let items = items_at(
            "/guide",
            6,
            dt(2026, 7, 7, 14, 30),
            Some(&doc),
            Some(dir.path()),
        );
        let inserts: Vec<&str> = items
            .iter()
            .filter_map(|i| match i.text_edit.as_ref()? {
                CompletionTextEdit::Edit(e) => Some(e.new_text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            inserts.contains(&"[guide](./docs/guide.md)"),
            "got {inserts:?}"
        );
    }

    #[test]
    fn file_links_follow_completion_path_style() {
        use crate::config::{CompletionConfig, PathStyle};

        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("docs/guide.md"), "").unwrap();
        let doc = dir.path().join("index.md");
        let config = Config {
            completion: CompletionConfig {
                path_style: PathStyle::Absolute,
                ..CompletionConfig::default()
            },
            ..Config::default()
        };

        let items = items_with(
            "/guide",
            6,
            dt(2026, 7, 7, 14, 30),
            Some(&doc),
            Some(dir.path()),
            config,
        );
        assert_eq!(
            inserted(&items, "/docs/guide.md"),
            "[guide](/docs/guide.md)"
        );
    }

    #[test]
    fn disabled_snippets_return_nothing() {
        let config = Config {
            snippets: SnippetsConfig {
                enabled: false,
                ..SnippetsConfig::default()
            },
            ..Config::default()
        };
        let items = items_with("/", 1, dt(2026, 7, 7, 14, 30), None, None, config);
        assert!(items.is_empty());
    }

    #[test]
    fn invalid_time_format_falls_back() {
        let config = Config {
            snippets: SnippetsConfig {
                time_format: "%Q:%".to_string(),
                ..SnippetsConfig::default()
            },
            ..Config::default()
        };
        let items = items_with("/", 1, dt(2026, 7, 7, 14, 30), None, None, config);
        // Falls back to the default "%H:%M" instead of panicking.
        assert_eq!(inserted(&items, "now"), "14:30");
    }
}
