//! URL classification shared by diagnostics and completion.

use percent_encoding::percent_decode_str;

/// Returns `true` if `url` has an explicit URI scheme such as `https:`,
/// `mailto:`, or `tel:`. Windows drive letters (`C:\...`) are intentionally not
/// special-cased; Markdown links overwhelmingly use forward-slash relative
/// paths.
pub fn has_scheme(url: &str) -> bool {
    let bytes = url.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b':' => return i > 0,
            b'/' | b'?' | b'#' | b' ' => return false,
            c if c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.' => continue,
            _ => return false,
        }
    }
    false
}

/// Returns `true` for URLs that point somewhere other than the local
/// filesystem (external schemes, protocol-relative URLs, or pure anchors).
pub fn is_external_or_anchor(url: &str) -> bool {
    let url = url.trim();
    url.is_empty() || url.starts_with('#') || url.starts_with("//") || has_scheme(url)
}

/// Extract the local, filesystem-checkable path portion of a link destination.
///
/// Returns [`None`] for external URLs, anchors, and empty destinations. The
/// fragment (`#...`) and query (`?...`) are stripped and the remainder is
/// percent-decoded.
pub fn local_target(url: &str) -> Option<String> {
    let url = url.trim();
    if is_external_or_anchor(url) {
        return None;
    }
    let path = url.split(['#', '?']).next().unwrap_or("");
    if path.is_empty() {
        return None;
    }
    let decoded = percent_decode_str(path).decode_utf8_lossy().into_owned();
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes() {
        assert!(has_scheme("https://example.com"));
        assert!(has_scheme("mailto:a@b.com"));
        assert!(!has_scheme("./relative.md"));
        assert!(!has_scheme("/abs/path.md"));
        assert!(!has_scheme("no-scheme/here"));
    }

    #[test]
    fn local_targets() {
        assert_eq!(local_target("./a.md"), Some("./a.md".to_string()));
        assert_eq!(
            local_target("../b/c.md#frag"),
            Some("../b/c.md".to_string())
        );
        assert_eq!(local_target("my%20file.md"), Some("my file.md".to_string()));
        assert_eq!(local_target("https://x.com"), None);
        assert_eq!(local_target("#section"), None);
        assert_eq!(local_target(""), None);
        assert_eq!(local_target("//cdn/x"), None);
    }
}
