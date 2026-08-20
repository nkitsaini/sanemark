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
