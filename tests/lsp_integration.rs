//! End-to-end wiring test: drive the [`LspService`] in-process with JSON-RPC
//! messages (initialize -> initialized -> didOpen -> foldingRange /
//! documentSymbol) and assert on the responses.

use futures_util::StreamExt;
use sanemark::server::Backend;
use serde_json::{json, Value};
use tower::{Service, ServiceExt};
use tower_lsp_server::jsonrpc::{Request, Response};
use tower_lsp_server::LspService;

async fn call(service: &mut LspService<Backend>, req: Request) -> Option<Response> {
    service.ready().await.unwrap().call(req).await.unwrap()
}

fn ok_result(resp: Option<Response>) -> Value {
    let (_, result) = resp.expect("expected a response").into_parts();
    result.expect("expected a success result")
}

/// Build a service and continuously drain the server->client socket so the
/// bounded client channel never blocks the handlers under test.
fn service() -> LspService<Backend> {
    let (service, socket) = LspService::new(Backend::new);
    let (mut requests, _responses) = socket.split();
    tokio::spawn(async move { while requests.next().await.is_some() {} });
    service
}

#[tokio::test]
async fn initialize_open_fold_and_symbols() {
    let mut service = service();

    // initialize
    let resp = call(
        &mut service,
        Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish(),
    )
    .await;
    let init = ok_result(resp);
    assert!(init.get("capabilities").is_some());
    assert_eq!(
        init["capabilities"]["foldingRangeProvider"],
        Value::Bool(true)
    );
    assert_eq!(
        init["capabilities"]["definitionProvider"],
        Value::Bool(true)
    );
    assert_eq!(
        init["capabilities"]["referencesProvider"],
        Value::Bool(true)
    );
    assert!(
        init["capabilities"]["documentLinkProvider"].is_object(),
        "documentLinkProvider should be advertised"
    );

    // initialized (notification -> no response)
    let _ = call(
        &mut service,
        Request::build("initialized").params(json!({})).finish(),
    )
    .await;

    // didOpen (notification)
    let text = "# A\n\ntext under a\n\n## B\nmore text\n";
    let _ = call(
        &mut service,
        Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": {
                    "uri": "file:///tmp/sanemark_it.md",
                    "languageId": "markdown",
                    "version": 1,
                    "text": text
                }
            }))
            .finish(),
    )
    .await;

    // foldingRange
    let resp = call(
        &mut service,
        Request::build("textDocument/foldingRange")
            .id(2)
            .params(json!({ "textDocument": { "uri": "file:///tmp/sanemark_it.md" } }))
            .finish(),
    )
    .await;
    let ranges = ok_result(resp);
    let ranges = ranges.as_array().expect("folding ranges array");
    assert!(!ranges.is_empty(), "expected at least one folding range");

    // documentSymbol
    let resp = call(
        &mut service,
        Request::build("textDocument/documentSymbol")
            .id(3)
            .params(json!({ "textDocument": { "uri": "file:///tmp/sanemark_it.md" } }))
            .finish(),
    )
    .await;
    let symbols = ok_result(resp);
    let symbols = symbols.as_array().expect("symbols array");
    assert_eq!(symbols.len(), 1); // "A" with child "B"
    assert_eq!(symbols[0]["name"], "A");
    assert_eq!(symbols[0]["children"][0]["name"], "B");
}

#[tokio::test]
async fn incremental_change_updates_folding() {
    let mut service = service();

    let _ = call(
        &mut service,
        Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {} }))
            .finish(),
    )
    .await;
    let _ = call(
        &mut service,
        Request::build("initialized").params(json!({})).finish(),
    )
    .await;

    let uri = "file:///tmp/sanemark_it2.md";
    let _ = call(
        &mut service,
        Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": { "uri": uri, "languageId": "markdown", "version": 1, "text": "para\n" }
            }))
            .finish(),
    )
    .await;

    // Incrementally insert a fenced code block after "para\n".
    let _ = call(
        &mut service,
        Request::build("textDocument/didChange")
            .params(json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "range": { "start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 0} },
                    "text": "```\ncode1\ncode2\n```\n"
                }]
            }))
            .finish(),
    )
    .await;

    let resp = call(
        &mut service,
        Request::build("textDocument/foldingRange")
            .id(2)
            .params(json!({ "textDocument": { "uri": uri } }))
            .finish(),
    )
    .await;
    let ranges = ok_result(resp);
    let ranges = ranges.as_array().expect("folding ranges array");
    assert!(
        ranges.iter().any(|r| r["startLine"] == 1),
        "code block inserted via incremental edit should fold: {ranges:?}"
    );
}

/// Drive `initialize` (+ optional init options) and `initialized`.
async fn init(service: &mut LspService<Backend>, init_options: Value) {
    let _ = call(
        service,
        Request::build("initialize")
            .id(1)
            .params(json!({ "capabilities": {}, "initializationOptions": init_options }))
            .finish(),
    )
    .await;
    let _ = call(
        service,
        Request::build("initialized").params(json!({})).finish(),
    )
    .await;
}

async fn did_open(service: &mut LspService<Backend>, uri: &str, text: &str) {
    let _ = call(
        service,
        Request::build("textDocument/didOpen")
            .params(json!({
                "textDocument": { "uri": uri, "languageId": "markdown", "version": 1, "text": text }
            }))
            .finish(),
    )
    .await;
}

#[tokio::test]
async fn goto_definition_and_references() {
    let mut service = service();
    init(&mut service, json!({})).await;

    let uri = "file:///tmp/ml_nav.md";
    let text = "[one][a] and [two][a].\n\n[a]: https://example.com\n";
    did_open(&mut service, uri, text).await;

    // Definition of the first reference jumps to the `[a]:` line (line 2).
    let resp = call(
        &mut service,
        Request::build("textDocument/definition")
            .id(2)
            .params(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 6 }
            }))
            .finish(),
    )
    .await;
    let locs = ok_result(resp);
    let locs = locs.as_array().expect("definition locations");
    assert_eq!(locs.len(), 1);
    assert_eq!(locs[0]["range"]["start"]["line"], 2);

    // References of the identifier: both usages + the declaration.
    let resp = call(
        &mut service,
        Request::build("textDocument/references")
            .id(3)
            .params(json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 6 },
                "context": { "includeDeclaration": true }
            }))
            .finish(),
    )
    .await;
    let refs = ok_result(resp);
    let refs = refs.as_array().expect("references array");
    assert_eq!(refs.len(), 3);
}

#[tokio::test]
async fn document_links_for_urls() {
    let mut service = service();
    init(&mut service, json!({})).await;

    let uri = "file:///tmp/ml_links.md";
    did_open(
        &mut service,
        uri,
        "see [g](https://google.com) and [m](mailto:a@b.com)\n",
    )
    .await;

    let resp = call(
        &mut service,
        Request::build("textDocument/documentLink")
            .id(2)
            .params(json!({ "textDocument": { "uri": uri } }))
            .finish(),
    )
    .await;
    let links = ok_result(resp);
    let links = links.as_array().expect("document links array");
    let targets: Vec<&str> = links.iter().filter_map(|l| l["target"].as_str()).collect();
    assert!(targets.contains(&"https://google.com"), "got {targets:?}");
    assert!(
        targets.iter().any(|t| t.starts_with("mailto:")),
        "got {targets:?}"
    );
}

#[tokio::test]
async fn formatting_formats_tables_by_default() {
    let mut service = service();
    // Defaults: table formatting on, reference-move off.
    init(&mut service, json!({})).await;

    let uri = "file:///tmp/ml_tbl.md";
    did_open(&mut service, uri, "a | b | c\n").await;

    let resp = call(
        &mut service,
        Request::build("textDocument/formatting")
            .id(2)
            .params(json!({
                "textDocument": { "uri": uri },
                "options": { "tabSize": 2, "insertSpaces": true }
            }))
            .finish(),
    )
    .await;
    let edits = ok_result(resp);
    let edits = edits.as_array().expect("format edits");
    assert_eq!(edits.len(), 1);
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert!(
        new_text.contains("| a"),
        "expected a table, got:\n{new_text}"
    );
    assert!(
        new_text.contains("| ---"),
        "expected a separator, got:\n{new_text}"
    );
}

#[tokio::test]
async fn copy_as_inlined_command_returns_inlined_text() {
    let mut service = service();
    init(&mut service, json!({})).await;

    let uri = "file:///tmp/ml_inline.md";
    // Reference links whose definitions live at the bottom of the file.
    let text = "See [one][1] here.\n\nAnd [two][2].\n\n# References\n\n[1]: https://one.com\n[2]: https://two.com\n";
    did_open(&mut service, uri, text).await;

    // Whole document (no range argument): every reference is inlined and the
    // definitions / heading are dropped, all without editing the buffer.
    let resp = call(
        &mut service,
        Request::build("workspace/executeCommand")
            .id(2)
            .params(json!({
                "command": "sanemark.copyAsInlined",
                "arguments": [uri]
            }))
            .finish(),
    )
    .await;
    let whole = ok_result(resp);
    let whole = whole.as_str().expect("command should return a string");
    assert!(whole.contains("[one](https://one.com)"), "got: {whole}");
    assert!(whole.contains("[two](https://two.com)"), "got: {whole}");
    assert!(
        !whole.contains("[1]:"),
        "definitions should be gone: {whole}"
    );

    // A selection range: only the links inside it are inlined, and the result
    // is just that slice (references still resolved from the whole document).
    let resp = call(
        &mut service,
        Request::build("workspace/executeCommand")
            .id(3)
            .params(json!({
                "command": "sanemark.copyAsInlined",
                "arguments": [uri, {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 18 }
                }]
            }))
            .finish(),
    )
    .await;
    let slice = ok_result(resp);
    let slice = slice.as_str().expect("command should return a string");
    assert_eq!(slice, "See [one](https://one.com) here.");
}

/// Regression: an empty `didChangeConfiguration` payload (as some clients send)
/// must not clobber the config supplied via `initializationOptions`, so
/// `textDocument/formatting` keeps moving references to the bottom.
#[tokio::test]
async fn did_change_configuration_does_not_clobber_formatting() {
    let mut service = service();
    init(
        &mut service,
        json!({ "formatting": { "moveReferencesToBottom": true } }),
    )
    .await;

    let uri = "file:///tmp/ml_cfg.md";
    did_open(&mut service, uri, "[x](https://a.com)\n").await;

    // A client sends an unrelated / empty settings payload.
    let _ = call(
        &mut service,
        Request::build("workspace/didChangeConfiguration")
            .params(json!({ "settings": {} }))
            .finish(),
    )
    .await;

    let resp = call(
        &mut service,
        Request::build("textDocument/formatting")
            .id(2)
            .params(json!({
                "textDocument": { "uri": uri },
                "options": { "tabSize": 2, "insertSpaces": true }
            }))
            .finish(),
    )
    .await;
    let edits = ok_result(resp);
    let edits = edits.as_array().expect("format edits");
    assert_eq!(edits.len(), 1, "formatting should still produce an edit");
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert!(
        new_text.contains("[x][1]") && new_text.contains("[1]: https://a.com"),
        "references should still move to the bottom, got:\n{new_text}"
    );
}

#[tokio::test]
async fn jsonc_project_config_is_loaded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("test")).unwrap();
    let doc = dir.path().join("test/index.md");
    std::fs::write(&doc, "").unwrap();
    std::fs::write(dir.path().join("test/sibling.md"), "").unwrap();
    std::fs::write(
        dir.path().join(".sanemark.jsonc"),
        r#"{
            // Force workspace-root paths even outside Git.
            "completion": {
                "pathStyle": "absolute",
            },
        }"#,
    )
    .unwrap();

    let root_uri = format!("file://{}", dir.path().display());
    let doc_uri = format!("file://{}", doc.display());
    let mut service = service();
    let _ = call(
        &mut service,
        Request::build("initialize")
            .id(1)
            .params(json!({
                "capabilities": {},
                "workspaceFolders": [{ "uri": root_uri, "name": "test" }]
            }))
            .finish(),
    )
    .await;
    let _ = call(
        &mut service,
        Request::build("initialized").params(json!({})).finish(),
    )
    .await;
    did_open(&mut service, &doc_uri, "[s](sib").await;

    let response = call(
        &mut service,
        Request::build("textDocument/completion")
            .id(2)
            .params(json!({
                "textDocument": { "uri": doc_uri },
                "position": { "line": 0, "character": 7 }
            }))
            .finish(),
    )
    .await;
    let result = ok_result(response);
    let items = result["items"].as_array().expect("completion items");
    assert!(
        items.iter().any(|item| item["label"] == "/test/sibling.md"),
        "JSONC pathStyle should apply, got: {items:?}"
    );
}

#[tokio::test]
async fn broken_link_quick_fixes_preserve_destinations_and_filter_actions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("docs")).unwrap();
    std::fs::write(dir.path().join("docs/guide book.md"), "").unwrap();
    std::fs::write(dir.path().join("photo.png"), "").unwrap();
    std::fs::write(dir.path().join(".ignore"), "ignored.md\n").unwrap();
    std::fs::write(dir.path().join("ignored.md"), "").unwrap();
    let uri = sanemark::uri::from_path(&dir.path().join("index.md")).unwrap();
    let root = sanemark::uri::from_path(dir.path()).unwrap();
    let mut service = service();
    call(
        &mut service,
        Request::build("initialize")
            .id(1)
            .params(
                json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "test"}]}),
            )
            .finish(),
    )
    .await;
    call(
        &mut service,
        Request::build("initialized").params(json!({})).finish(),
    )
    .await;
    for (i, (text, expected)) in [
        (
            "😀 [guide](./old/guide%20bok.md?raw=1#intro)",
            Some("./docs/guide%20book.md?raw=1#intro"),
        ),
        (
            "[guide](</docs/guide bok.md#intro>)",
            Some("/docs/guide%20book.md#intro"),
        ),
        (
            "[guide][g]\n\n[g]: ./guide%20bok.md \"Title\"",
            Some("./docs/guide%20book.md"),
        ),
        ("![photo](./phoot.png)", Some("./photo.png")),
        ("[exists](./photo.png)", None),
        ("[external](https://example.com/missing.md)", None),
        ("[ignored](./ignoredd.md)", None),
        ("[unrelated](./zzzzzzzzzz.md)", None),
    ]
    .into_iter()
    .enumerate()
    {
        call(&mut service, Request::build("textDocument/didOpen").params(json!({
            "textDocument": {"uri": uri, "languageId": "markdown", "version": i as i32 + 1, "text": text}
        })).finish()).await;
        let result = ok_result(call(&mut service, Request::build("textDocument/codeAction").id(i as i64 + 2)
            .params(json!({"textDocument": {"uri": uri},
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 10, "character": 0}},
                "context": {"diagnostics": [], "only": ["quickfix"]}})).finish()).await);
        let actions = result.as_array().unwrap();
        if let Some(expected) = expected {
            assert!(!actions.is_empty(), "{text}");
            assert_eq!(actions[0]["kind"], "quickfix");
            let edit = &actions[0]["edit"]["changes"][uri.as_str()][0];
            assert_eq!(edit["newText"], expected, "{text}");
            // Apply the returned UTF-16 edit and ensure it repairs the diagnostic.
            let mut rope = ropey::Rope::from_str(text);
            let range = serde_json::from_value(edit["range"].clone()).unwrap();
            let edit = tower_lsp_server::ls_types::TextEdit {
                range,
                new_text: expected.into(),
            };
            let start = sanemark::encoding::position_to_char(
                &rope,
                edit.range.start,
                sanemark::encoding::PositionEncoding::Utf16,
            );
            let end = sanemark::encoding::position_to_char(
                &rope,
                edit.range.end,
                sanemark::encoding::PositionEncoding::Utf16,
            );
            rope.remove(start..end);
            rope.insert(start, expected);
            let analysis = sanemark::analysis::analyze(&rope.to_string(), 1);
            assert!(sanemark::features::diagnostics::diagnostics(
                &analysis,
                &rope,
                &Default::default(),
                sanemark::encoding::PositionEncoding::Utf16,
                Some(&dir.path().join("index.md")),
                Some(dir.path())
            )
            .is_empty());
        } else {
            assert!(actions.is_empty(), "{text}: {actions:?}");
        }
    }
}
