//! Path completion inside link / image destinations.
//!
//! Completion is driven by the *text around the cursor* rather than the AST:
//! while typing `[x](./pa` the destination isn't a valid link yet, so there is
//! no reliable node to attach to. We scan the current line back to the opening
//! `](` and, if the partial destination looks like a path, offer matching files
//! from the workspace, fuzzy-matched fzf-style (via [`crate::fuzzy`], backed by
//! nucleo).
//!
//! ## Scope & presentation
//!
//! By default we search the **whole workspace tree** (from the workspace root),
//! so a nearby file can be completed without first typing `../`. How each hit is
//! *presented* depends on the prefix the user has typed:
//!
//! * bare (no prefix) — presentation follows `completion.pathStyle`; its
//!   `auto` default uses `hybrid` in a Git work tree (relative for siblings and
//!   children, workspace-root absolute for other files) and `relative`
//!   elsewhere;
//! * explicit `./` or `../` — **everything** is offered relative (walking up
//!   with `../` as needed);
//! * explicit `/` — **everything** is offered absolute (workspace-root relative).
//!
//! The walk is breadth-first and bounded (depth, a scan budget and a result
//! cap) so it stays cheap even in large trees.

use std::fs;
use std::path::{Component, Path};

use ignore::WalkBuilder;
use ropey::Rope;
use tower_lsp_server::ls_types::{
    Command, CompletionItem, CompletionItemKind, CompletionTextEdit, Position, Range, TextEdit,
};

use crate::config::{CompletionConfig, PathStyle};
use crate::encoding::{char_to_position, position_to_char, PositionEncoding};
use crate::fuzzy::PathMatcher;
use crate::links::has_scheme;

/// Hard cap on directory entries examined during a single completion request,
/// regardless of configuration. Protects against pathological trees.
const WALK_BUDGET: usize = 8192;

/// How the user anchored the destination they're typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// No `./` or `/` prefix: relative for down/parallel, absolute for up.
    Bare,
    /// Explicit `./` or `../`: everything presented relative.
    Relative,
    /// Explicit `/`: everything presented absolute (workspace-root relative).
    Absolute,
}

/// The parsed "we are inside a link destination" context.
struct PathContext {
    mode: Mode,
    /// The full destination text typed so far (also used as the fuzzy query).
    query: String,
}

/// A scored completion candidate before it becomes a [`CompletionItem`].
struct Candidate {
    /// The complete path to insert, e.g. `./test/c.md`, `../a.md` or `/x.md`.
    display: String,
    is_dir: bool,
    /// Whether the target is at/under the current document's directory.
    is_down: bool,
    depth: usize,
    /// Fuzzy score (higher is better); `0` for an empty query.
    score: u32,
    name: String,
}

/// Produce path completions for the given cursor position.
pub fn complete(
    rope: &Rope,
    position: Position,
    enc: PositionEncoding,
    config: &CompletionConfig,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Vec<CompletionItem> {
    if !config.paths {
        return Vec::new();
    }

    let cursor_char = position_to_char(rope, position, enc);
    let line_idx = rope.char_to_line(cursor_char);
    let line_start_char = rope.line_to_char(line_idx);
    let before: String = rope.slice(line_start_char..cursor_char).chars().collect();

    let mut ctx = match detect_context(&before) {
        Some(ctx) => ctx,
        None => return Vec::new(),
    };

    let doc_dir = doc_path.and_then(Path::parent);
    // Absolute mode always resolves from the workspace root; the others fall
    // back to it too (so bare queries can reach files above the document).
    let root = match workspace_root.or(doc_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    if ctx.mode == Mode::Bare {
        ctx.mode = configured_mode(config.path_style, root);
    }

    // When deep search is off, restrict to the document's own directory (for
    // absolute mode, the root) and don't recurse.
    let (scope, max_depth): (&Path, usize) = if config.deep_paths {
        (root, config.deep_paths_max_depth.max(1))
    } else {
        let base = match ctx.mode {
            Mode::Absolute => root,
            _ => doc_dir.unwrap_or(root),
        };
        (base, 1)
    };

    let candidates = collect(scope, doc_dir, root, &ctx, config, max_depth);

    // The edit replaces the entire destination typed so far, since each item is
    // a complete path (relative or absolute).
    let replace_len = ctx.query.chars().count();
    let replace_range = Range::new(
        char_to_position(rope, cursor_char.saturating_sub(replace_len), enc),
        position,
    );

    candidates
        .into_iter()
        .enumerate()
        .map(|(rank, c)| make_item(c, rank, &replace_range))
        .collect()
}

/// A workspace file surfaced for the snippet (`@`/`/`) menu, already presented
/// as a link destination.
pub(crate) struct FileHit {
    /// Presented destination, e.g. `./notes.md` or `/shared/x.md`.
    pub display: String,
    /// The file's name (with extension).
    pub name: String,
}

/// Walk the workspace (configured bare mode, deep + fuzzy per `config`) and return the
/// matching **files** (never directories), best-first. Shared with the snippet
/// menu, which turns each hit into a `[stem](display)` inline link.
pub(crate) fn workspace_files(
    query: &str,
    config: &CompletionConfig,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Vec<FileHit> {
    let doc_dir = doc_path.and_then(Path::parent);
    let root = match workspace_root.or(doc_dir) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let ctx = PathContext {
        mode: configured_mode(config.path_style, root),
        query: query.to_string(),
    };
    let (scope, max_depth): (&Path, usize) = if config.deep_paths {
        (root, config.deep_paths_max_depth.max(1))
    } else {
        (doc_dir.unwrap_or(root), 1)
    };
    collect(scope, doc_dir, root, &ctx, config, max_depth)
        .into_iter()
        .filter(|c| !c.is_dir)
        .map(|c| FileHit {
            display: c.display,
            name: c.name,
        })
        .collect()
}

/// Walk `scope` (bounded), presenting and scoring each entry, then sort
/// best-first and cap to `config.max_items`.
///
/// Uses ripgrep's [`ignore`] walker so `.gitignore` / `.ignore` / git excludes
/// and hidden files are honoured per `config` — which also prunes the usual
/// heavy directories (`node_modules`, `target`, `.git`) for free.
fn collect(
    scope: &Path,
    doc_dir: Option<&Path>,
    root: &Path,
    ctx: &PathContext,
    config: &CompletionConfig,
    max_depth: usize,
) -> Vec<Candidate> {
    let mut matcher = PathMatcher::new(&ctx.query);
    // Reveal hidden entries when the query itself targets a dotfile.
    let query_is_hidden = ctx
        .query
        .rsplit('/')
        .next()
        .is_some_and(|s| s.starts_with('.'));
    let show_hidden = config.show_hidden_files || query_is_hidden;

    let mut out: Vec<Candidate> = Vec::new();

    for entry in build_walker(scope, max_depth, show_hidden, config.gitignore)
        .build()
        .take(WALK_BUDGET)
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        // Skip the scope root itself.
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let path = entry.path();

        if let Some((display, is_down)) = present(path, is_dir, doc_dir, root, ctx.mode) {
            if let Some(score) = matcher.score(&display) {
                out.push(Candidate {
                    display,
                    is_dir,
                    is_down,
                    depth: entry.depth(),
                    score,
                    name: entry.file_name().to_string_lossy().into_owned(),
                });
            }
        }
    }

    sort_candidates(&mut out, config);
    out.truncate(config.max_items);
    out
}

/// A configured [`ignore`] walker: depth-bounded, honouring hidden-file and
/// gitignore preferences. `.gitignore` is applied even outside a git repo
/// (`require_git(false)`) so Markdown workspaces that aren't git repos still get
/// the expected behaviour.
pub(crate) fn build_walker(
    scope: &Path,
    max_depth: usize,
    show_hidden: bool,
    gitignore: bool,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(scope);
    builder
        .max_depth(Some(max_depth))
        .hidden(!show_hidden)
        .parents(gitignore)
        .git_ignore(gitignore)
        .git_global(gitignore)
        .git_exclude(gitignore)
        .ignore(gitignore)
        .require_git(false);
    builder
}

/// Compute the presented path for `path` under the active [`Mode`], plus whether
/// it is at/under the document's directory. Returns `None` when it can't be
/// expressed (e.g. the document's own directory, or a divergent drive/root).
fn present(
    path: &Path,
    is_dir: bool,
    doc_dir: Option<&Path>,
    root: &Path,
    mode: Mode,
) -> Option<(String, bool)> {
    let doc_rel = doc_dir.and_then(|d| rel_path(d, path));
    let is_down = doc_rel.as_deref().is_some_and(|r| !r.starts_with(".."));

    let display = match mode {
        Mode::Absolute => absolute_display(root, path, is_dir)?,
        Mode::Relative => match &doc_rel {
            Some(r) => relative_display(r, is_dir)?,
            None => absolute_display(root, path, is_dir)?,
        },
        Mode::Bare => {
            if is_down {
                relative_display(doc_rel.as_deref()?, is_dir)?
            } else {
                absolute_display(root, path, is_dir)?
            }
        }
    };
    Some((display, is_down))
}

/// Resolve the configured style for a bare completion query.
fn configured_mode(style: PathStyle, root: &Path) -> Mode {
    match style {
        PathStyle::Auto if is_git_work_tree(root) => Mode::Bare,
        PathStyle::Auto | PathStyle::Relative => Mode::Relative,
        PathStyle::Absolute => Mode::Absolute,
        PathStyle::Hybrid => Mode::Bare,
    }
}

/// Detect a Git work tree without spawning `git`. Checking ancestors handles
/// both a repository root (`.git` directory), linked worktrees (`.git` file),
/// and an editor workspace opened at a subdirectory of either.
fn is_git_work_tree(path: &Path) -> bool {
    for dir in path.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_file() || dot_git.join("HEAD").exists() || dot_git.join("config").exists() {
            return true;
        }
        #[cfg(unix)]
        if is_mount_point(dir) {
            break;
        }
    }
    false
}

#[cfg(unix)]
fn is_mount_point(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = fs::metadata(dir) else {
        return false;
    };
    let Some(parent) = dir.parent() else {
        return true;
    };
    let Ok(parent_meta) = fs::metadata(parent) else {
        return false;
    };
    meta.dev() != parent_meta.dev()
}

fn relative_display(rel: &str, is_dir: bool) -> Option<String> {
    if rel == "." {
        return None; // the document's own directory
    }
    let base = if rel == ".." || rel.starts_with("../") {
        rel.to_string()
    } else {
        format!("./{rel}")
    };
    Some(if is_dir { format!("{base}/") } else { base })
}

fn absolute_display(root: &Path, path: &Path, is_dir: bool) -> Option<String> {
    let rel = rel_path(root, path)?;
    if rel == "." || rel.starts_with("..") {
        return None; // outside the workspace root
    }
    let base = format!("/{rel}");
    Some(if is_dir { format!("{base}/") } else { base })
}

/// Lexical relative path from `from` to `to` using `/` separators and `..` as
/// needed. `None` if they don't share a common prefix (e.g. different drives).
pub(crate) fn rel_path(from: &Path, to: &Path) -> Option<String> {
    let f: Vec<Component> = from.components().collect();
    let t: Vec<Component> = to.components().collect();

    let mut i = 0;
    while i < f.len() && i < t.len() && f[i] == t[i] {
        i += 1;
    }

    let mut parts: Vec<String> = Vec::new();
    for c in &f[i..] {
        match c {
            Component::Normal(_) => parts.push("..".to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    for c in &t[i..] {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }

    if parts.is_empty() {
        Some(".".to_string())
    } else {
        Some(parts.join("/"))
    }
}

/// Order candidates best-first: higher fuzzy score, then nearer (down before
/// up), shallower, prioritized extension, and finally alphabetically.
fn sort_candidates(cands: &mut [Candidate], config: &CompletionConfig) {
    cands.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(b.is_down.cmp(&a.is_down))
            .then(a.depth.cmp(&b.depth))
            .then(ext_bucket(&a.name, a.is_dir, config).cmp(&ext_bucket(&b.name, b.is_dir, config)))
            .then(a.display.len().cmp(&b.display.len()))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Detect whether the cursor sits inside an (unclosed) link/image destination
/// and, if so, classify the destination's prefix.
fn detect_context(before: &str) -> Option<PathContext> {
    let open = before.rfind("](")?;
    let mut partial = &before[open + 2..];

    // A closed destination or a title / whitespace means we're no longer in the
    // path segment.
    if partial.contains(')') || partial.contains(char::is_whitespace) {
        return None;
    }
    // Angle-bracketed destinations: `](<path`.
    if let Some(stripped) = partial.strip_prefix('<') {
        partial = stripped;
    }
    // Never complete over an external URL or a bare anchor.
    if partial.starts_with('#') || partial.contains("://") || has_scheme(partial) {
        return None;
    }

    let mode = if partial.starts_with('/') {
        Mode::Absolute
    } else if partial.starts_with("./")
        || partial.starts_with("../")
        || partial == "."
        || partial == ".."
    {
        Mode::Relative
    } else {
        Mode::Bare
    };

    Some(PathContext {
        mode,
        query: partial.to_string(),
    })
}

fn make_item(cand: Candidate, rank: usize, replace_range: &Range) -> CompletionItem {
    // Re-trigger completion after entering a directory so the user can keep
    // narrowing without pressing the trigger key again.
    let command = if cand.is_dir {
        Some(Command {
            title: "Suggest".to_string(),
            command: "editor.action.triggerSuggest".to_string(),
            arguments: None,
        })
    } else {
        None
    };

    let kind = if cand.is_dir {
        CompletionItemKind::FOLDER
    } else {
        CompletionItemKind::FILE
    };

    // Preserve our ranking on clients that honour `sortText`, while `filterText`
    // (the full path) lets clients that fuzzy-filter keep every entry.
    let sort_text = format!("{rank:06}");

    CompletionItem {
        label: cand.display.clone(),
        kind: Some(kind),
        sort_text: Some(sort_text),
        filter_text: Some(cand.display.clone()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: *replace_range,
            new_text: cand.display,
        })),
        command,
        ..Default::default()
    }
}

/// Extension-priority bucket: prioritized extensions first, then other files,
/// then directories.
fn ext_bucket(name: &str, is_dir: bool, config: &CompletionConfig) -> u8 {
    if is_dir {
        2
    } else if config
        .prioritize_extensions
        .iter()
        .any(|ext| name.to_lowercase().ends_with(&ext.to_lowercase()))
    {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn labels(items: &[CompletionItem]) -> Vec<String> {
        let mut l: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        l.sort();
        l
    }

    fn complete_at(
        text: &str,
        cursor: u32,
        cfg: &CompletionConfig,
        doc: &Path,
        root: &Path,
    ) -> Vec<CompletionItem> {
        let rope = Rope::from_str(text);
        complete(
            &rope,
            Position::new(0, cursor),
            PositionEncoding::Utf16,
            cfg,
            Some(doc),
            Some(root),
        )
    }

    #[test]
    fn relative_prefix_lists_relative_paths() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("notes.md"), "").unwrap();
        fs::write(dir.path().join("other.txt"), "").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        let doc = dir.path().join("index.md");
        fs::write(&doc, "").unwrap();

        let items = complete_at(
            "see [x](./",
            10,
            &CompletionConfig::default(),
            &doc,
            dir.path(),
        );
        assert_eq!(
            labels(&items),
            vec!["./index.md", "./notes.md", "./other.txt", "./sub/"]
        );
    }

    #[test]
    fn filters_by_fuzzy_query() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("alpha.md"), "").unwrap();
        fs::write(dir.path().join("beta.md"), "").unwrap();
        let doc = dir.path().join("index.md");

        let items = complete_at(
            "see [x](./al",
            12,
            &CompletionConfig::default(),
            &doc,
            dir.path(),
        );
        assert_eq!(labels(&items), vec!["./alpha.md"]);
    }

    #[test]
    fn respects_gitignore_by_default() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.md\n").unwrap();
        fs::write(dir.path().join("kept.md"), "").unwrap();
        fs::write(dir.path().join("ignored.md"), "").unwrap();
        let doc = dir.path().join("index.md");

        let items = complete_at(
            "see [x](./",
            10,
            &CompletionConfig::default(),
            &doc,
            dir.path(),
        );
        let ls = labels(&items);
        assert!(ls.contains(&"./kept.md".to_string()), "got {ls:?}");
        assert!(
            !ls.contains(&"./ignored.md".to_string()),
            "gitignored; got {ls:?}"
        );

        // Disabling the gitignore option surfaces the ignored file again.
        let cfg = CompletionConfig {
            gitignore: false,
            ..CompletionConfig::default()
        };
        let items = complete_at("see [x](./", 10, &cfg, &doc, dir.path());
        assert!(labels(&items).contains(&"./ignored.md".to_string()));
    }

    #[test]
    fn deep_completion_finds_nested_file() {
        // a.md, b.md, test/k.md — from a.md, `[k](./` surfaces ./test/k.md.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::write(dir.path().join("b.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("a.md");

        let items = complete_at("[k](./", 6, &CompletionConfig::default(), &doc, dir.path());
        assert!(
            labels(&items).contains(&"./test/k.md".to_string()),
            "got {:?}",
            labels(&items)
        );

        let items = complete_at("[k](./k", 7, &CompletionConfig::default(), &doc, dir.path());
        assert!(
            labels(&items).contains(&"./test/k.md".to_string()),
            "got {:?}",
            labels(&items)
        );
    }

    #[test]
    fn auto_style_reaches_parent_as_absolute_in_git_work_tree() {
        // From test/k.md, a bare `a` should offer the root's a.md as ABSOLUTE
        // in a Git work tree (it requires walking up).
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        fs::write(dir.path().join("test/sibling.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");

        let items = complete_at("[a](a", 5, &CompletionConfig::default(), &doc, dir.path());
        let ls = labels(&items);
        assert!(
            ls.contains(&"/a.md".to_string()),
            "up file absolute; got {ls:?}"
        );
    }

    #[test]
    fn auto_style_is_relative_outside_git_work_tree() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");

        let items = complete_at("[a](a", 5, &CompletionConfig::default(), &doc, dir.path());
        let ls = labels(&items);
        assert!(
            ls.contains(&"../a.md".to_string()),
            "up file relative; got {ls:?}"
        );
        assert!(!ls.contains(&"/a.md".to_string()), "got {ls:?}");
    }

    #[test]
    fn configured_absolute_style_applies_to_bare_queries() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        fs::write(dir.path().join("test/sibling.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");
        let cfg = CompletionConfig {
            path_style: PathStyle::Absolute,
            ..CompletionConfig::default()
        };

        let items = complete_at("[s](sib", 7, &cfg, &doc, dir.path());
        assert!(
            labels(&items).contains(&"/test/sibling.md".to_string()),
            "got {:?}",
            labels(&items)
        );
    }

    #[test]
    fn configured_relative_style_overrides_git_default() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");
        let cfg = CompletionConfig {
            path_style: PathStyle::Relative,
            ..CompletionConfig::default()
        };

        let items = complete_at("[a](a", 5, &cfg, &doc, dir.path());
        assert!(labels(&items).contains(&"../a.md".to_string()));
    }

    #[test]
    fn configured_hybrid_style_does_not_require_git() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");
        let cfg = CompletionConfig {
            path_style: PathStyle::Hybrid,
            ..CompletionConfig::default()
        };

        let items = complete_at("[a](a", 5, &cfg, &doc, dir.path());
        assert!(labels(&items).contains(&"/a.md".to_string()));
    }

    #[test]
    fn git_detection_handles_workspace_subdirectories() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::create_dir_all(dir.path().join("notes/nested")).unwrap();
        fs::write(dir.path().join("notes/a.md"), "").unwrap();
        fs::write(dir.path().join("notes/nested/k.md"), "").unwrap();
        let root = dir.path().join("notes");
        let doc = root.join("nested/k.md");

        let items = complete_at("[a](a", 5, &CompletionConfig::default(), &doc, &root);
        assert!(labels(&items).contains(&"/a.md".to_string()));
    }

    #[test]
    fn bare_query_prefers_relative_for_siblings() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        fs::write(dir.path().join("test/sibling.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");

        let items = complete_at("[s](sib", 7, &CompletionConfig::default(), &doc, dir.path());
        let ls = labels(&items);
        assert!(
            ls.contains(&"./sibling.md".to_string()),
            "sibling relative; got {ls:?}"
        );
    }

    #[test]
    fn explicit_relative_walks_up() {
        // `../` from test/k.md offers the root's a.md as a relative path.
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");

        let items = complete_at(
            "[a](../a",
            8,
            &CompletionConfig::default(),
            &doc,
            dir.path(),
        );
        assert!(
            labels(&items).contains(&"../a.md".to_string()),
            "got {:?}",
            labels(&items)
        );
    }

    #[test]
    fn explicit_relative_prefix_overrides_absolute_style() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");
        let cfg = CompletionConfig {
            path_style: PathStyle::Absolute,
            ..CompletionConfig::default()
        };

        let items = complete_at("[a](../a", 8, &cfg, &doc, dir.path());
        assert!(labels(&items).contains(&"../a.md".to_string()));
    }

    #[test]
    fn explicit_absolute_lists_from_root() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");

        // Absolute mode from a nested doc still lists the whole root.
        let items = complete_at("[a](/", 5, &CompletionConfig::default(), &doc, dir.path());
        let ls = labels(&items);
        assert!(ls.contains(&"/a.md".to_string()), "got {ls:?}");
        assert!(ls.contains(&"/test/k.md".to_string()), "got {ls:?}");
    }

    #[test]
    fn explicit_absolute_prefix_overrides_relative_style() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("test/k.md");
        let cfg = CompletionConfig {
            path_style: PathStyle::Relative,
            ..CompletionConfig::default()
        };

        let items = complete_at("[a](/a", 6, &cfg, &doc, dir.path());
        assert!(labels(&items).contains(&"/a.md".to_string()));
    }

    #[test]
    fn deep_completion_can_be_disabled() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("test")).unwrap();
        fs::write(dir.path().join("test/k.md"), "").unwrap();
        let doc = dir.path().join("a.md");

        let cfg = CompletionConfig {
            deep_paths: false,
            ..CompletionConfig::default()
        };
        let items = complete_at("[k](./", 6, &cfg, &doc, dir.path());
        // Only the immediate `./test/` directory, not the nested file.
        assert_eq!(labels(&items), vec!["./test/"]);
    }

    #[test]
    fn no_completion_for_urls() {
        let rope = Rope::from_str("see [x](https://");
        let items = complete(
            &rope,
            Position::new(0, 16),
            PositionEncoding::Utf16,
            &CompletionConfig::default(),
            None,
            None,
        );
        assert!(items.is_empty());
    }

    #[test]
    fn no_completion_outside_link() {
        let rope = Rope::from_str("just some text ");
        let items = complete(
            &rope,
            Position::new(0, 15),
            PositionEncoding::Utf16,
            &CompletionConfig::default(),
            None,
            None,
        );
        assert!(items.is_empty());
    }
}
