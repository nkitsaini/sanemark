//! End-to-end tests for the `format` and `lint` CLI subcommands, driving the
//! actual built binary.

use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::tempdir;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sanemark"))
}

/// Run the binary with `args`, feeding `stdin`, returning (exit_code, stdout).
fn run(args: &[&str], stdin: &str) -> (i32, String) {
    let mut child = bin()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sanemark");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn format_stdin_builds_table() {
    let (code, stdout) = run(&["format", "--stdin"], "a | b | c\n");
    assert_eq!(code, 0);
    assert!(stdout.contains("| a"), "got: {stdout}");
    assert!(stdout.contains("| ---"), "got: {stdout}");
}

#[test]
fn format_stdin_moves_references() {
    let (code, stdout) = run(
        &["format", "--stdin", "--move-references"],
        "[x](https://a.com)\n",
    );
    assert_eq!(code, 0);
    assert!(stdout.contains("[x][1]"), "got: {stdout}");
    assert!(stdout.contains("[1]: https://a.com"), "got: {stdout}");
}

#[test]
fn config_prints_documented_default_jsonc() {
    let (code, stdout) = run(&["config"], "");
    assert_eq!(code, 0);
    assert!(
        stdout.contains("// auto:"),
        "expected explanatory comments: {stdout}"
    );
    let value =
        sanemark::config::parse_jsonc_value(&stdout).expect("config output must be valid JSONC");
    // A few representative keys across the config tree, in camelCase.
    assert!(value.get("folding").is_some(), "got: {stdout}");
    assert!(value.get("snippets").is_some(), "got: {stdout}");
    assert_eq!(value["completion"]["pathStyle"], "auto");
    assert_eq!(value["snippets"]["timeFormat"], "%H:%M");
    assert_eq!(value["formatting"]["referencesHeading"], "References");
}

#[test]
fn readme_prints_documentation() {
    let (code, stdout) = run(&["readme"], "");
    assert_eq!(code, 0);
    assert!(
        stdout.contains("# sanemark"),
        "got start: {:?}",
        &stdout[..stdout.len().min(80)]
    );
    assert!(stdout.contains("Configuration"));
}

#[test]
fn inline_stdin_expands_references() {
    let input =
        "See [one][1] and [two][2].\n\n# References\n\n[1]: https://one.com\n[2]: https://two.com\n";
    let (code, stdout) = run(&["inline", "--stdin"], input);
    assert_eq!(code, 0);
    assert!(stdout.contains("[one](https://one.com)"), "got: {stdout}");
    assert!(stdout.contains("[two](https://two.com)"), "got: {stdout}");
    assert!(
        !stdout.contains("[1]:"),
        "definitions should be gone: {stdout}"
    );
    assert!(
        !stdout.contains("# References"),
        "heading should be gone: {stdout}"
    );
}

#[test]
fn format_write_and_check() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("t.md");
    std::fs::write(&file, "a | b | c\n").unwrap();
    let path = file.to_str().unwrap();

    // --check reports it would change and exits non-zero.
    let (code, _) = run(&["format", "--check", path], "");
    assert_eq!(code, 1);

    // --write rewrites in place; a second --check then passes.
    let (code, _) = run(&["format", "--write", path], "");
    assert_eq!(code, 0);
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(written.contains("| ---"), "not formatted: {written}");

    let (code, _) = run(&["format", "--check", path], "");
    assert_eq!(code, 0);
}

#[test]
fn lint_reports_missing_files() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("exists.md"), "").unwrap();
    let doc = dir.path().join("index.md");
    std::fs::write(&doc, "[a](./exists.md)\n\n[b](./missing.md)\n").unwrap();

    let (code, stdout) = run(
        &[
            "lint",
            "--root",
            dir.path().to_str().unwrap(),
            doc.to_str().unwrap(),
        ],
        "",
    );
    assert_eq!(code, 1);
    assert!(stdout.contains("missing.md"), "got: {stdout}");
    assert!(
        !stdout.contains("exists.md"),
        "should not flag existing file: {stdout}"
    );
}
