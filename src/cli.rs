//! Command-line interface: run the formatter and the broken-link linter without
//! an editor.
//!
//! ```text
//! sanemark format [--write|--check] [--move-references] [--no-tables] [--no-lists] [PATHS...]
//! sanemark inline [--references-heading NAME] [--stdin] [PATHS...]
//! sanemark lint   [--root DIR] [--no-images] PATHS...
//! ```
//!
//! `PATHS` may be files *or* directories: directories are searched recursively
//! for Markdown files (`*.md` / `*.markdown`), so `sanemark format .`
//! formats every Markdown file under the current directory.
//!
//! With no subcommand the binary speaks LSP over stdio (see `main.rs`).

use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::analysis::analyze;
use crate::config::{DiagnosticsConfig, FormattingConfig};
use crate::encoding::PositionEncoding;
use crate::features::{diagnostics, formatting};

/// Dispatch a CLI subcommand. `args[0]` is the subcommand name. Returns a
/// process exit code.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("format") => cmd_format(&args[1..]),
        Some("inline") => cmd_inline(&args[1..]),
        Some("lint") | Some("check") => cmd_lint(&args[1..]),
        Some("config") => cmd_config(),
        Some("readme") => cmd_readme(),
        _ => {
            print_help();
            2
        }
    }
}

/// Print an example configuration (every option at its default value) as JSONC.
/// Handy for seeding an editor's settings — see the README for where it goes.
fn cmd_config() -> i32 {
    match crate::config::example_jsonc() {
        Ok(jsonc) => {
            println!("{jsonc}");
            0
        }
        Err(e) => {
            eprintln!("failed to serialize config: {e}");
            1
        }
    }
}

/// Print this crate's README (embedded at build time).
fn cmd_readme() -> i32 {
    print!("{}", include_str!("../README.md"));
    0
}

/// Top-level `--help` text.
pub fn print_help() {
    eprintln!(
        "sanemark {}\n\
\n\
USAGE:\n\
    sanemark                       Run the language server over stdio (default)\n\
    sanemark format [OPTIONS] [PATHS...]\n\
    sanemark inline [OPTIONS] [PATHS...]\n\
    sanemark lint   [OPTIONS] PATHS...\n\
    sanemark config                Print a commented example config (all defaults) as JSONC\n\
    sanemark readme                Print the README to stdout\n\
\n\
PATHS may be files or directories; directories are searched recursively for\n\
Markdown files (*.md / *.markdown). E.g. `sanemark format .`\n\
By default the walk respects .gitignore/.ignore and skips hidden files.\n\
\n\
DIRECTORY WALK OPTIONS (format / inline / lint):\n\
        --no-ignore         Do not respect .gitignore / .ignore files\n\
        --hidden            Include hidden (dot) files and directories\n\
\n\
FORMAT OPTIONS:\n\
    -w, --write             Rewrite files in place (default: print to stdout)\n\
        --check             Exit non-zero if any file is not already formatted\n\
    -r, --move-references   Consolidate reference links under a heading at the bottom\n\
        --no-tables         Do not reformat GFM tables\n\
        --no-lists          Do not normalise list markers\n\
        --references-heading <NAME>   Heading for the references section (default: References)\n\
        --stdin             Read from stdin, write to stdout\n\
\n\
INLINE OPTIONS (expand reference links to inline, dropping the definitions —\n\
handy for copying self-contained Markdown, e.g. `sanemark inline notes.md | wl-copy`):\n\
        --references-heading <NAME>   Heading of the references section to drop (default: References)\n\
        --stdin             Read from stdin, write to stdout\n\
\n\
LINT OPTIONS:\n\
        --root <DIR>        Workspace root for absolute-path links (default: cwd)\n\
        --no-images         Skip image (`![]()`) destinations\n\
\n\
OTHER:\n\
    -h, --help              Show this help\n\
    -V, --version           Show version",
        env!("CARGO_PKG_VERSION")
    );
}

// ---------------------------------------------------------------------------
// format
// ---------------------------------------------------------------------------

fn cmd_format(args: &[String]) -> i32 {
    let mut write = false;
    let mut check = false;
    let mut use_stdin = false;
    let mut walk = WalkOpts::default();
    let mut cfg = FormattingConfig {
        move_references_to_bottom: false,
        format_tables: true,
        format_lists: true,
        ..FormattingConfig::default()
    };
    let mut files: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-w" | "--write" => write = true,
            "--check" => check = true,
            "--stdin" => use_stdin = true,
            "--no-ignore" => walk.gitignore = false,
            "--hidden" => walk.hidden = true,
            "-r" | "--move-references" => cfg.move_references_to_bottom = true,
            "--no-tables" => cfg.format_tables = false,
            "--no-lists" => cfg.format_lists = false,
            "--references-heading" => match it.next() {
                Some(v) => cfg.references_heading = v.clone(),
                None => {
                    eprintln!("--references-heading requires a value");
                    return 2;
                }
            },
            "-h" | "--help" => {
                print_help();
                return 0;
            }
            other if other.starts_with('-') && other != "-" => {
                eprintln!("format: unknown option '{other}'");
                return 2;
            }
            other => files.push(other.to_string()),
        }
    }

    if use_stdin || files.is_empty() {
        let source = read_stdin();
        print!("{}", formatting::format_document(&source, true, &cfg));
        return 0;
    }

    let files = expand_inputs(&files, walk);
    let mut changed = false;
    let mut had_error = false;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file}: {e}");
                had_error = true;
                continue;
            }
        };
        let formatted = formatting::format_document(&source, true, &cfg);
        let file_changed = formatted != source;
        changed |= file_changed;

        if check {
            if file_changed {
                println!("would reformat {file}");
            }
        } else if write {
            if file_changed {
                if let Err(e) = std::fs::write(file, &formatted) {
                    eprintln!("{file}: {e}");
                    had_error = true;
                } else {
                    println!("formatted {file}");
                }
            }
        } else {
            print!("{formatted}");
        }
    }

    if had_error || (check && changed) {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// inline
// ---------------------------------------------------------------------------

/// Expand reference links to inline links (dropping the definitions and the
/// references heading) and print the result to stdout. Non-destructive by
/// design: it never rewrites files, so it composes with a clipboard tool, e.g.
/// `sanemark inline notes.md | pbcopy`.
fn cmd_inline(args: &[String]) -> i32 {
    let mut use_stdin = false;
    let mut walk = WalkOpts::default();
    let mut references_heading = "References".to_string();
    let mut files: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--stdin" => use_stdin = true,
            "--no-ignore" => walk.gitignore = false,
            "--hidden" => walk.hidden = true,
            "--references-heading" => match it.next() {
                Some(v) => references_heading = v.clone(),
                None => {
                    eprintln!("--references-heading requires a value");
                    return 2;
                }
            },
            "-h" | "--help" => {
                print_help();
                return 0;
            }
            other if other.starts_with('-') && other != "-" => {
                eprintln!("inline: unknown option '{other}'");
                return 2;
            }
            other => files.push(other.to_string()),
        }
    }

    if use_stdin || files.is_empty() {
        let source = read_stdin();
        print!(
            "{}",
            formatting::to_inline_links(&source, true, &references_heading)
        );
        return 0;
    }

    let files = expand_inputs(&files, walk);
    let mut had_error = false;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file}: {e}");
                had_error = true;
                continue;
            }
        };
        print!(
            "{}",
            formatting::to_inline_links(&source, true, &references_heading)
        );
    }

    if had_error {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// lint
// ---------------------------------------------------------------------------

/// A single lint finding, in editor-friendly 1-based coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintIssue {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

fn cmd_lint(args: &[String]) -> i32 {
    let mut root: Option<PathBuf> = None;
    let mut check_images = true;
    let mut walk = WalkOpts::default();
    let mut files: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => match it.next() {
                Some(v) => root = Some(PathBuf::from(v)),
                None => {
                    eprintln!("--root requires a value");
                    return 2;
                }
            },
            "--no-images" => check_images = false,
            "--no-ignore" => walk.gitignore = false,
            "--hidden" => walk.hidden = true,
            "-h" | "--help" => {
                print_help();
                return 0;
            }
            other if other.starts_with('-') && other != "-" => {
                eprintln!("lint: unknown option '{other}'");
                return 2;
            }
            other => files.push(other.to_string()),
        }
    }

    if files.is_empty() {
        eprintln!("lint: no files given");
        return 2;
    }

    let files = expand_inputs(&files, walk);
    let root = root
        .or_else(|| std::env::current_dir().ok())
        .map(|p| crate::uri::normalize(&p));
    let cfg = DiagnosticsConfig {
        broken_links: true,
        check_images,
        ..DiagnosticsConfig::default()
    };

    let mut total = 0usize;
    let mut had_error = false;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{file}: {e}");
                had_error = true;
                continue;
            }
        };
        let doc_path = absolutize(Path::new(file));
        let issues = lint_source(&source, Some(&doc_path), root.as_deref(), &cfg);
        for issue in &issues {
            println!("{file}:{}:{}: {}", issue.line, issue.column, issue.message);
            total += 1;
        }
    }

    if had_error || total > 0 {
        1
    } else {
        0
    }
}

/// Pure linting helper: broken-link findings for `text`. Byte columns are
/// reported (1-based).
pub fn lint_source(
    text: &str,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
    cfg: &DiagnosticsConfig,
) -> Vec<LintIssue> {
    let analysis = analyze(text, 0);
    let rope = Rope::from_str(text);
    // UTF-8 encoding => `character` is a byte offset within the line.
    let diags = diagnostics::diagnostics(
        &analysis,
        &rope,
        cfg,
        PositionEncoding::Utf8,
        doc_path,
        workspace_root,
    );
    diags
        .into_iter()
        .map(|d| LintIssue {
            line: d.range.start.line as usize + 1,
            column: d.range.start.character as usize + 1,
            message: d.message,
        })
        .collect()
}

fn absolutize(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    crate::uri::normalize(&joined)
}

fn read_stdin() -> String {
    use std::io::Read;
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    buf
}

// ---------------------------------------------------------------------------
// Path expansion (files + directories)
// ---------------------------------------------------------------------------

/// Markdown file extensions recognised when walking directories.
const MARKDOWN_EXTS: &[&str] = &["md", "markdown"];

/// How a directory walk should treat ignored / hidden files.
#[derive(Debug, Clone, Copy)]
struct WalkOpts {
    /// Respect `.gitignore` / `.ignore` / git excludes (default true).
    gitignore: bool,
    /// Include hidden (dot) files and directories (default false).
    hidden: bool,
}

impl Default for WalkOpts {
    fn default() -> Self {
        Self {
            gitignore: true,
            hidden: false,
        }
    }
}

/// Expand `inputs` into a list of files: directory arguments are searched
/// recursively for Markdown files (via ripgrep's [`ignore`] walker, so
/// `.gitignore` and hidden files are honoured per `opts`); everything else is
/// passed through untouched so non-existent files still surface a per-file
/// error later.
fn expand_inputs(inputs: &[String], opts: WalkOpts) -> Vec<String> {
    let mut out = Vec::new();
    for input in inputs {
        let path = Path::new(input);
        if path.is_dir() {
            collect_markdown(path, opts, &mut out);
        } else {
            out.push(input.clone());
        }
    }
    out
}

/// Recursively collect Markdown files under `dir` into `out`, sorted for
/// deterministic output.
fn collect_markdown(dir: &Path, opts: WalkOpts, out: &mut Vec<String>) {
    let mut builder = ignore::WalkBuilder::new(dir);
    builder
        .hidden(!opts.hidden)
        .parents(opts.gitignore)
        .git_ignore(opts.gitignore)
        .git_global(opts.gitignore)
        .git_exclude(opts.gitignore)
        .ignore(opts.gitignore)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b));

    for entry in builder.build().flatten() {
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if is_file && is_markdown(entry.path()) {
            out.push(entry.path().to_string_lossy().into_owned());
        }
    }
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| MARKDOWN_EXTS.iter().any(|m| ext.eq_ignore_ascii_case(m)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn lint_reports_missing_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("exists.md"), "").unwrap();
        let doc = dir.path().join("index.md");
        let text = "[a](./exists.md)\n\n[b](./missing.md)\n";
        let issues = lint_source(
            text,
            Some(&doc),
            Some(dir.path()),
            &DiagnosticsConfig::default(),
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].line, 3);
        assert!(issues[0].message.contains("missing.md"));
    }

    #[test]
    fn format_via_cli_helper_moves_references() {
        let cfg = FormattingConfig {
            move_references_to_bottom: true,
            format_tables: true,
            ..FormattingConfig::default()
        };
        let out = formatting::format_document("[x](https://a.com)\n", true, &cfg);
        assert!(out.contains("[x][1]"));
        assert!(out.contains("[1]: https://a.com"));
    }

    #[test]
    fn format_via_cli_helper_formats_tables() {
        let cfg = FormattingConfig::default();
        let out = formatting::format_document("a | b | c\n", true, &cfg);
        assert!(out.contains("| a"));
        assert!(out.contains("| ---"));
    }

    fn names_of(paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn expand_inputs_walks_directories_for_markdown() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        fs::write(dir.path().join("b.markdown"), "").unwrap();
        fs::write(dir.path().join("c.txt"), "").unwrap(); // non-md
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/d.md"), "").unwrap();

        let names = names_of(&expand_inputs(
            &[dir.path().to_string_lossy().into_owned()],
            WalkOpts::default(),
        ));
        assert!(names.contains(&"a.md".to_string()), "got {names:?}");
        assert!(names.contains(&"b.markdown".to_string()), "got {names:?}");
        assert!(names.contains(&"d.md".to_string()), "nested; got {names:?}");
        assert!(!names.contains(&"c.txt".to_string()), "non-md excluded");
    }

    #[test]
    fn expand_inputs_respects_gitignore_and_hidden_by_default() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored/\nsecret.md\n").unwrap();
        fs::write(dir.path().join("keep.md"), "").unwrap();
        fs::write(dir.path().join("secret.md"), "").unwrap(); // gitignored
        fs::write(dir.path().join(".hidden.md"), "").unwrap(); // hidden
        fs::create_dir(dir.path().join("ignored")).unwrap();
        fs::write(dir.path().join("ignored/x.md"), "").unwrap(); // gitignored dir
        let arg = dir.path().to_string_lossy().into_owned();

        // Defaults: gitignore + hidden are honoured.
        let names = names_of(&expand_inputs(
            std::slice::from_ref(&arg),
            WalkOpts::default(),
        ));
        assert_eq!(
            names,
            vec!["keep.md".to_string()],
            "default walk; got {names:?}"
        );

        // Overrides bring the ignored/hidden files back.
        let all = names_of(&expand_inputs(
            std::slice::from_ref(&arg),
            WalkOpts {
                gitignore: false,
                hidden: true,
            },
        ));
        assert!(
            all.contains(&"secret.md".to_string()),
            "no-ignore; got {all:?}"
        );
        assert!(
            all.contains(&".hidden.md".to_string()),
            "hidden; got {all:?}"
        );
        assert!(
            all.contains(&"x.md".to_string()),
            "ignored dir; got {all:?}"
        );
    }

    #[test]
    fn expand_inputs_passes_through_files() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("note.md");
        fs::write(&f, "").unwrap();
        let arg = f.to_string_lossy().into_owned();
        assert_eq!(
            expand_inputs(std::slice::from_ref(&arg), WalkOpts::default()),
            vec![arg]
        );
    }
}
