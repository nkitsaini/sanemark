//! Broken-link diagnostics: warn when a link/image points at a local file that
//! does not exist on disk.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ropey::Rope;
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

use crate::analysis::Analysis;
use crate::config::{DiagnosticsConfig, Severity};
use crate::encoding::{range_from_bytes, PositionEncoding};
use crate::links;
use crate::uri;

/// Compute broken-link diagnostics for a document.
pub fn diagnostics(
    analysis: &Analysis,
    rope: &Rope,
    config: &DiagnosticsConfig,
    enc: PositionEncoding,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Vec<Diagnostic> {
    if !config.broken_links {
        return Vec::new();
    }

    let ignore = build_globset(&config.ignore);
    let doc_dir = doc_path.and_then(Path::parent);
    let mut diagnostics = Vec::new();

    for target in &analysis.link_targets {
        if target.is_image && !config.check_images {
            continue;
        }
        let Some(local) = links::local_target(&target.url) else {
            continue;
        };
        if ignore.is_match(&local) {
            continue;
        }

        // Absolute paths resolve against the workspace root; everything else
        // against the document's directory.
        let base = if local.starts_with('/') {
            workspace_root
        } else {
            doc_dir
        };
        let Some(base) = base else { continue };
        let resolved = resolve(base, &local);

        if !resolved.exists() {
            diagnostics.push(Diagnostic {
                range: range_from_bytes(rope, target.start_byte, target.end_byte, enc),
                severity: Some(severity(config.severity)),
                code: Some(NumberOrString::String("broken-link".to_string())),
                source: Some("sanemark".to_string()),
                message: format!("File does not exist: {local}"),
                ..Default::default()
            });
        }
    }

    diagnostics
}

/// Suggest bounded, ranked replacements for current broken-link diagnostics.
/// Reusing freshly computed diagnostics honours ignore/image settings and avoids
/// offering edits for stale client diagnostics.
pub fn fixes(
    analysis: &Analysis,
    rope: &Rope,
    diagnostics: &[Diagnostic],
    params: &tower_lsp_server::ls_types::CodeActionParams,
    enc: PositionEncoding,
    root: Option<&Path>,
    config: &crate::config::CompletionConfig,
) -> Vec<tower_lsp_server::ls_types::CodeActionOrCommand> {
    use tower_lsp_server::ls_types::{CodeAction, CodeActionKind, TextEdit, WorkspaceEdit};
    let doc_path = uri::to_path(&params.text_document.uri);
    let Some(doc_dir) = doc_path.as_deref().and_then(Path::parent) else {
        return Vec::new();
    };
    let root = root.unwrap_or(doc_dir);
    let relevant: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.range.start <= params.range.end && params.range.start <= d.range.end)
        .collect();
    if relevant.is_empty() {
        return Vec::new();
    }
    let files: Vec<_> = super::completion::build_walker(
        root,
        config.deep_paths_max_depth.max(1),
        config.show_hidden_files,
        config.gitignore,
    )
    .build()
    .take(20_000)
    .filter_map(Result::ok)
    .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
    .map(|e| e.into_path())
    .collect();
    let mut actions = Vec::new();
    for diagnostic in relevant {
        let Some(target) = analysis
            .link_targets
            .iter()
            .find(|t| range_from_bytes(rope, t.start_byte, t.end_byte, enc) == diagnostic.range)
        else {
            continue;
        };
        let Some(local) = links::local_target(&target.url) else {
            continue;
        };
        let name = Path::new(&local)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if name.is_empty() {
            continue;
        }
        let mut matcher = crate::fuzzy::PathMatcher::new(&name);
        let mut candidates = Vec::new();
        for path in &files {
            let candidate = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            let distance = edit_distance(&name, &candidate);
            let score = matcher.score(&candidate);
            // Edit distance catches substitutions and transpositions, which a
            // subsequence matcher alone cannot recover.
            let threshold = (name.chars().count() / 3).clamp(1, 3);
            if distance > threshold && score.is_none() {
                continue;
            }
            let base = if local.starts_with('/') {
                root
            } else {
                doc_dir
            };
            let Some(relative) = super::completion::rel_path(base, path) else {
                continue;
            };
            let display = if local.starts_with('/') {
                format!("/{relative}")
            } else if local.starts_with("./") && !relative.starts_with("../") {
                format!("./{relative}")
            } else {
                relative
            };
            candidates.push((distance, std::cmp::Reverse(score.unwrap_or(0)), display));
        }
        candidates.sort();
        candidates.truncate(5);
        // Encode URL delimiters and Markdown destination syntax, preserving the
        // original fragment/query verbatim.
        const ESCAPE: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
            .add(b' ')
            .add(b'%')
            .add(b'#')
            .add(b'?')
            .add(b'(')
            .add(b')')
            .add(b'<')
            .add(b'>')
            .add(b'\\')
            .add(b'"')
            .add(b'\'')
            .add(b'`');
        let suffix = target
            .url
            .find(['#', '?'])
            .map(|i| &target.url[i..])
            .unwrap_or("");
        for (_, _, display) in candidates {
            let replacement = format!(
                "{}{suffix}",
                percent_encoding::utf8_percent_encode(&display, ESCAPE)
            );
            actions.push(
                CodeAction {
                    title: format!("Replace with {display}"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(WorkspaceEdit {
                        changes: Some(
                            [(
                                params.text_document.uri.clone(),
                                vec![TextEdit {
                                    range: diagnostic.range,
                                    new_text: replacement,
                                }],
                            )]
                            .into_iter()
                            .collect(),
                        ),
                        ..Default::default()
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
    }
    actions
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    // Keep adversarial or unusually long destinations cheap to score.
    if a.len() > 256 || b.len() > 256 {
        return usize::MAX;
    }
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(old + 1)
                .min(diagonal + usize::from(x != y));
            diagonal = old;
        }
    }
    row[b.len()]
}

fn resolve(base: &Path, local: &str) -> PathBuf {
    let joined = base.join(local.trim_start_matches('/'));
    uri::normalize(&joined)
}

fn build_globset(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            builder.add(g);
        }
    }
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

fn severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Information => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use std::fs;
    use tempfile::tempdir;

    fn run(
        text: &str,
        doc_path: &Path,
        root: &Path,
        config: &DiagnosticsConfig,
    ) -> Vec<Diagnostic> {
        let a = analyze(text, 1);
        let rope = Rope::from_str(text);
        diagnostics(
            &a,
            &rope,
            config,
            PositionEncoding::Utf16,
            Some(doc_path),
            Some(root),
        )
    }

    #[test]
    fn flags_missing_file_only() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("exists.md"), "").unwrap();
        let doc = dir.path().join("index.md");

        let text = "[a](./exists.md) and [b](./missing.md)";
        let diags = run(text, &doc, dir.path(), &DiagnosticsConfig::default());
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("missing.md"));
    }

    #[test]
    fn ignores_external_and_anchor() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("index.md");
        let text = "[a](https://x.com) [b](#frag) [c](mailto:x@y.z)";
        assert!(run(text, &doc, dir.path(), &DiagnosticsConfig::default()).is_empty());
    }

    #[test]
    fn respects_ignore_globs() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("index.md");
        let text = "[a](./missing.md)";
        let cfg = DiagnosticsConfig {
            ignore: vec!["**/missing.md".to_string()],
            ..DiagnosticsConfig::default()
        };
        assert!(run(text, &doc, dir.path(), &cfg).is_empty());
    }

    #[test]
    fn image_check_toggle() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("index.md");
        let text = "![alt](./missing.png)";
        let off = DiagnosticsConfig {
            check_images: false,
            ..DiagnosticsConfig::default()
        };
        assert!(run(text, &doc, dir.path(), &off).is_empty());
        assert_eq!(
            run(text, &doc, dir.path(), &DiagnosticsConfig::default()).len(),
            1
        );
    }
}
