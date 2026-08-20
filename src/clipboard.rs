//! Best-effort system-clipboard writes.
//!
//! A language server can't reach the editor's clipboard over LSP, but it *is* a
//! local process, so it can copy by shelling out to whatever clipboard tool the
//! platform provides. We try, in order: `wl-copy` (Wayland), `xclip` / `xsel`
//! (X11), and `pbcopy` (macOS). The Nix package wraps the binary so at least one
//! of these is always on `PATH`.

use std::io::Write;
use std::process::{Command, Stdio};

/// Clipboard tools to try, in priority order, as `(program, args)`.
const TOOLS: &[(&str, &[&str])] = &[
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
    ("xsel", &["--clipboard", "--input"]),
    ("pbcopy", &[]),
];

/// Copy `text` to the system clipboard, returning the tool that succeeded.
///
/// Best-effort and synchronous (run it off the async runtime, e.g. via
/// `spawn_blocking`). Returns `Err` with a human-readable reason when no tool is
/// available or every candidate failed.
pub fn copy(text: &str) -> Result<&'static str, String> {
    let mut last_err =
        "no clipboard tool found (install wl-clipboard, xclip, xsel, or pbcopy)".to_string();
    for (prog, args) in TOOLS {
        match try_copy(prog, args, text) {
            Ok(()) => return Ok(prog),
            Err(e) => last_err = format!("{prog}: {e}"),
        }
    }
    Err(last_err)
}

/// Pipe `text` into `prog args` via stdin and wait for it to finish.
fn try_copy(prog: &str, args: &[&str], text: &str) -> Result<(), String> {
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "no stdin handle".to_string())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        // `stdin` is dropped here, signalling EOF so the tool can finish.
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_is_an_error_not_a_panic() {
        // A tool that certainly doesn't exist must surface as `Err`.
        assert!(try_copy("sanemark-no-such-clipboard-tool", &[], "hello").is_err());
    }
}
