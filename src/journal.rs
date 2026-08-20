//! Daily-note ("journal") creation.
//!
//! Resolves the note path for a given date under the workspace root and creates
//! it on demand — copying a configured template when present, otherwise seeding
//! a minimal note with a date heading. Opening an existing note never rewrites
//! it. The IO here is deliberately small and synchronous so it stays easy to
//! unit-test with a temp directory; [`crate::server`] handles the async
//! plumbing and asks the editor to open the resulting file.

use std::path::{Path, PathBuf};

use chrono::format::{Item, StrftimeItems};
use chrono::NaiveDate;

use crate::config::JournalConfig;

/// The outcome of [`ensure`]: where the note lives and whether it was just
/// created (vs. already existing).
pub struct DailyNote {
    pub path: PathBuf,
    pub created: bool,
}

/// Absolute path of the note for `date`, given the journal `config` and the
/// workspace `root`.
pub fn note_path(root: &Path, config: &JournalConfig, date: NaiveDate) -> PathBuf {
    let filename = format_date(&config.filename_format, date, "%Y-%m-%d.md");
    let dir = Path::new(&config.directory);
    let dir = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    };
    dir.join(filename)
}

/// Ensure the note for `date` exists, creating it (and any missing parent
/// directories) from the template — or a minimal default — when absent.
pub fn ensure(root: &Path, config: &JournalConfig, date: NaiveDate) -> std::io::Result<DailyNote> {
    let path = note_path(root, config, date);
    if path.exists() {
        return Ok(DailyNote {
            path,
            created: false,
        });
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match template_path(root, config) {
        Some(template) if template.exists() => {
            std::fs::copy(&template, &path)?;
        }
        _ => {
            std::fs::write(&path, default_contents(date))?;
        }
    }
    Ok(DailyNote {
        path,
        created: true,
    })
}

/// Resolve the configured template path (relative to `root`), if any.
fn template_path(root: &Path, config: &JournalConfig) -> Option<PathBuf> {
    config.template.as_ref().map(|t| {
        let p = Path::new(t);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    })
}

/// Minimal note body used when no template is configured/available.
fn default_contents(date: NaiveDate) -> String {
    format!("# {}\n\n", date.format("%Y-%m-%d"))
}

/// Format `date` with `fmt`, falling back to `fallback` on an invalid
/// `strftime` string so a bad config never panics.
fn format_date(fmt: &str, date: NaiveDate, fallback: &str) -> String {
    if StrftimeItems::new(fmt).any(|it| matches!(it, Item::Error)) {
        return date.format(fallback).to_string();
    }
    date.format(fmt).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 7).unwrap()
    }

    #[test]
    fn note_path_uses_directory_and_filename_format() {
        let cfg = JournalConfig {
            directory: "journal".to_string(),
            filename_format: "%Y-%m-%d.md".to_string(),
            ..JournalConfig::default()
        };
        let p = note_path(Path::new("/ws"), &cfg, date());
        assert!(p.ends_with("journal/2026-07-07.md"), "got {p:?}");
    }

    #[test]
    fn ensure_copies_template_when_present() {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("journal")).unwrap();
        fs::write(
            dir.path().join("journal/template.md"),
            "# Daily\n\n- [ ] task\n",
        )
        .unwrap();
        let cfg = JournalConfig {
            directory: "journal".to_string(),
            template: Some("journal/template.md".to_string()),
            filename_format: "%Y-%m-%d.md".to_string(),
        };

        let note = ensure(dir.path(), &cfg, date()).unwrap();
        assert!(note.created);
        assert_eq!(
            fs::read_to_string(&note.path).unwrap(),
            "# Daily\n\n- [ ] task\n"
        );

        // Opening again must not recreate/overwrite it.
        fs::write(&note.path, "edited\n").unwrap();
        let again = ensure(dir.path(), &cfg, date()).unwrap();
        assert!(!again.created);
        assert_eq!(fs::read_to_string(&again.path).unwrap(), "edited\n");
    }

    #[test]
    fn ensure_without_template_writes_default_heading() {
        let dir = tempdir().unwrap();
        let cfg = JournalConfig::default();
        let note = ensure(dir.path(), &cfg, date()).unwrap();
        assert!(note.created);
        assert!(note.path.ends_with("journal/2026-07-07.md"));
        assert_eq!(fs::read_to_string(&note.path).unwrap(), "# 2026-07-07\n\n");
    }

    #[test]
    fn invalid_filename_format_falls_back() {
        let cfg = JournalConfig {
            filename_format: "%Q.md".to_string(),
            ..JournalConfig::default()
        };
        let p = note_path(Path::new("/ws"), &cfg, date());
        assert!(p.ends_with("journal/2026-07-07.md"), "got {p:?}");
    }
}
