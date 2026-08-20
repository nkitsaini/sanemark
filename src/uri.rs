//! Helpers for converting between LSP [`Uri`]s and filesystem paths.

use std::path::{Path, PathBuf};

use tower_lsp_server::ls_types::Uri;

/// A stable string key for a document URI, used in the document store.
pub fn key(uri: &Uri) -> String {
    uri.as_str().to_string()
}

/// Convert a `file:` URI into a filesystem path, if possible.
pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    // `to_file_path` is an inherent method on the ls-types `Uri`.
    uri.to_file_path().map(|p| p.into_owned())
}

/// Convert a filesystem path into a `file:` URI, if possible.
pub fn from_path(path: &Path) -> Option<Uri> {
    Uri::from_file_path(path)
}

/// Lexically normalize a path: resolve `.` and `..` components without touching
/// the filesystem. This keeps link resolution predictable even when the target
/// does not (yet) exist.
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(
            normalize(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
    }
}
