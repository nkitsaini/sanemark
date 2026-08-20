//! Hover: show a link's resolved target and whether it exists on disk.

use std::path::Path;

use ropey::Rope;
use tower_lsp_server::ls_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position, Range,
};

use crate::analysis::Analysis;
use crate::encoding::{position_to_char, range_from_bytes, PositionEncoding};
use crate::links;
use crate::uri;

/// Produce hover information when the cursor is over a link/image destination.
pub fn hover(
    analysis: &Analysis,
    rope: &Rope,
    position: Position,
    enc: PositionEncoding,
    doc_path: Option<&Path>,
    workspace_root: Option<&Path>,
) -> Option<Hover> {
    let char_idx = position_to_char(rope, position, enc);
    let byte = rope.char_to_byte(char_idx);

    let target = analysis
        .link_targets
        .iter()
        .find(|t| byte >= t.start_byte && byte < t.end_byte)?;

    let value = match links::local_target(&target.url) {
        Some(local) => {
            let base = if local.starts_with('/') {
                workspace_root
            } else {
                doc_path.and_then(Path::parent)
            };
            match base {
                Some(base) => {
                    let resolved = uri::normalize(&base.join(local.trim_start_matches('/')));
                    let status = if resolved.exists() {
                        "exists"
                    } else {
                        "**missing**"
                    };
                    format!("`{}`\n\n{} — {}", target.url, resolved.display(), status)
                }
                None => format!("`{}`", target.url),
            }
        }
        None => format!("[{}]({})", target.url, target.url),
    };

    let range: Range = range_from_bytes(rope, target.start_byte, target.end_byte, enc);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(range),
    })
}
