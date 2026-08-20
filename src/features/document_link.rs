//! Document links (`textDocument/documentLink`): make link/image destinations
//! clickable.
//!
//! Every link, image and reference definition destination becomes a clickable
//! [`DocumentLink`]:
//!
//! * external URLs (`https:`, `http:`, `mailto:`, …) open as-is;
//! * local file paths resolve (relative to the document, or the workspace root
//!   for `/absolute` paths) and open the file when it exists.
//!
//! Pure `#anchor` destinations are skipped (there's nothing to open); use
//! go-to-definition for those.

use std::path::{Path, PathBuf};

use ropey::Rope;
use tower_lsp_server::ls_types::{DocumentLink, Uri};

use crate::analysis::Analysis;
use crate::encoding::{range_from_bytes, PositionEncoding};
use crate::links;
use crate::uri;

/// Build the clickable document links for a document.
pub fn document_links(
    analysis: &Analysis,
    rope: &Rope,
    enc: PositionEncoding,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Vec<DocumentLink> {
    let doc_dir = doc_path.and_then(Path::parent);
    let mut out = Vec::new();

    for target in &analysis.link_targets {
        let url = target.url.trim();
        if url.is_empty() || url.starts_with('#') {
            continue;
        }

        let (uri, tooltip) = if links::is_external_or_anchor(url) {
            // External scheme (http/https/mailto/…) — open verbatim.
            match url.parse::<Uri>() {
                Ok(u) => (u, format!("Open {url}")),
                Err(_) => continue,
            }
        } else {
            // Local file — resolve and only link when it exists.
            let Some(resolved) = resolve(url, doc_dir, workspace_root) else {
                continue;
            };
            if !resolved.exists() {
                continue;
            }
            match uri::from_path(&resolved) {
                Some(u) => (u, format!("Open {}", resolved.display())),
                None => continue,
            }
        };

        out.push(DocumentLink {
            range: range_from_bytes(rope, target.start_byte, target.end_byte, enc),
            target: Some(uri),
            tooltip: Some(tooltip),
            data: None,
        });
    }

    out
}

fn resolve(url: &str, doc_dir: Option<&Path>, workspace_root: Option<&Path>) -> Option<PathBuf> {
    let local = links::local_target(url)?;
    let base = if local.starts_with('/') {
        workspace_root
    } else {
        doc_dir
    }?;
    Some(uri::normalize(&base.join(local.trim_start_matches('/'))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use std::fs;
    use tempfile::tempdir;

    fn links_for(text: &str, doc: &Path, root: &Path) -> Vec<DocumentLink> {
        let analysis = analyze(text, 1);
        let rope = Rope::from_str(text);
        document_links(
            &analysis,
            &rope,
            PositionEncoding::Utf16,
            Some(doc),
            Some(root),
        )
    }

    #[test]
    fn external_url_is_linked() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("index.md");
        let links = links_for("see [g](https://google.com)", &doc, dir.path());
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "https://google.com"
        );
    }

    #[test]
    fn existing_local_file_is_linked_missing_is_not() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a.md"), "").unwrap();
        let doc = dir.path().join("index.md");
        let links = links_for("[a](./a.md) and [b](./missing.md)", &doc, dir.path());
        assert_eq!(links.len(), 1);
        assert!(links[0].target.as_ref().unwrap().as_str().ends_with("a.md"));
    }

    #[test]
    fn pure_anchor_is_skipped() {
        let dir = tempdir().unwrap();
        let doc = dir.path().join("index.md");
        assert!(links_for("[x](#section)", &doc, dir.path()).is_empty());
    }
}
