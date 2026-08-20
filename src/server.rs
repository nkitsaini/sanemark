//! The [`tower_lsp_server`] backend that ties the features together.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use ropey::Rope;
use tokio::sync::RwLock;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::analysis::{analyze, Analysis};
use crate::clipboard;
use crate::config::Config;
use crate::document::Document;
use crate::encoding::{char_to_position, position_to_char, PositionEncoding};
use crate::features::{
    completion, diagnostics, document_link, folding, formatting, hover, navigation, snippets,
    symbols,
};
use crate::{journal, uri};

const CMD_MOVE_REFS: &str = "sanemark.moveReferencesToBottom";
const CMD_INLINE_REFS: &str = "sanemark.inlineReferences";
const CMD_COPY_INLINED: &str = "sanemark.copyAsInlined";
const CMD_CONVERT_INLINE: &str = "sanemark.convertToInline";
const CMD_DAILY_NOTE: &str = "sanemark.openDailyNote";
const DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(200);

/// Project-level config files, discovered at the workspace root.
const PROJECT_CONFIG_FILES: [&str; 4] = [
    ".sanemark.json",
    ".sanemark.jsonc",
    ".markdown-lsp.json",
    ".markdown-lsp.jsonc",
];

/// Shared, cloneable server state so background tasks (debounced diagnostics)
/// can outlive a single request.
struct ServerState {
    client: Client,
    documents: DashMap<String, Document>,
    analyses: DashMap<String, Arc<Analysis>>,
    config: RwLock<Config>,
    /// The editor's settings (initializationOptions / didChangeConfiguration),
    /// kept so the effective config can be re-resolved when the project file
    /// changes.
    client_options: RwLock<Option<serde_json::Value>>,
    encoding: RwLock<PositionEncoding>,
    workspace_root: RwLock<Option<PathBuf>>,
    /// Whether the client advertised support for `window/showDocument`. When it
    /// doesn't, opening a journal note falls back to a `showMessage` with the
    /// note's path instead of silently doing nothing.
    supports_show_document: RwLock<bool>,
}

impl ServerState {
    fn doc_rope(&self, key: &str) -> Option<Rope> {
        self.documents.get(key).map(|d| d.rope.clone())
    }

    fn doc_version(&self, key: &str) -> Option<i32> {
        self.documents.get(key).map(|d| d.version)
    }

    /// Return a cached [`Analysis`], reparsing only when the document version
    /// changed since the cache was populated.
    fn analysis_for(&self, key: &str) -> Option<Arc<Analysis>> {
        let (text, version) = {
            let doc = self.documents.get(key)?;
            (doc.rope.to_string(), doc.version)
        };
        if let Some(existing) = self.analyses.get(key) {
            if existing.version == version {
                return Some(existing.clone());
            }
        }
        let analysis = Arc::new(analyze(&text, version));
        self.analyses.insert(key.to_string(), analysis.clone());
        Some(analysis)
    }

    async fn run_diagnostics(&self, doc_uri: Uri) {
        let key = uri::key(&doc_uri);
        let config = self.config.read().await.clone();

        if !config.diagnostics.broken_links {
            self.client
                .publish_diagnostics(doc_uri, Vec::new(), None)
                .await;
            return;
        }
        let (Some(rope), Some(analysis)) = (self.doc_rope(&key), self.analysis_for(&key)) else {
            return;
        };
        let enc = *self.encoding.read().await;
        let doc_path = uri::to_path(&doc_uri);
        let root = self.workspace_root.read().await.clone();
        let version = self.doc_version(&key);

        let diags = diagnostics::diagnostics(
            &analysis,
            &rope,
            &config.diagnostics,
            enc,
            doc_path.as_deref(),
            root.as_deref(),
        );
        self.client
            .publish_diagnostics(doc_uri, diags, version)
            .await;
    }

    /// Re-run diagnostics for every currently open document.
    async fn revalidate_all(&self) {
        let uris: Vec<Uri> = self
            .documents
            .iter()
            .filter_map(|e| e.key().parse::<Uri>().ok())
            .collect();
        for u in uris {
            self.run_diagnostics(u).await;
        }
    }
}

/// The language server backend.
pub struct Backend {
    state: Arc<ServerState>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self {
            state: Arc::new(ServerState {
                client,
                documents: DashMap::new(),
                analyses: DashMap::new(),
                config: RwLock::new(Config::default()),
                client_options: RwLock::new(None),
                encoding: RwLock::new(PositionEncoding::Utf16),
                workspace_root: RwLock::new(None),
                supports_show_document: RwLock::new(false),
            }),
        }
    }

    /// Publish diagnostics after a short debounce, skipping the run if a newer
    /// edit lands first.
    fn schedule_diagnostics(&self, doc_uri: Uri) {
        let state = self.state.clone();
        let key = uri::key(&doc_uri);
        let version = state.doc_version(&key).unwrap_or(0);
        tokio::spawn(async move {
            tokio::time::sleep(DIAGNOSTIC_DEBOUNCE).await;
            if state.doc_version(&key) == Some(version) {
                state.run_diagnostics(doc_uri).await;
            }
        });
    }

    async fn config(&self) -> Config {
        self.state.config.read().await.clone()
    }

    async fn encoding(&self) -> PositionEncoding {
        *self.state.encoding.read().await
    }

    async fn workspace_root(&self) -> Option<PathBuf> {
        self.state.workspace_root.read().await.clone()
    }

    /// Read + parse the project config file at the workspace root, if present.
    /// Invalid JSONC is reported and ignored (falling back to client settings).
    async fn read_project_config(&self) -> Option<serde_json::Value> {
        let root = self.state.workspace_root.read().await.clone()?;
        let path = PROJECT_CONFIG_FILES
            .iter()
            .map(|name| root.join(name))
            .find(|path| path.is_file())?;
        let name = path.file_name()?.to_string_lossy();
        let text = std::fs::read_to_string(&path).ok()?;
        match crate::config::parse_jsonc_value(&text) {
            Ok(value) => Some(value),
            Err(e) => {
                self.state
                    .client
                    .show_message(
                        MessageType::WARNING,
                        format!("{name}: invalid JSONC ({e}); ignoring"),
                    )
                    .await;
                None
            }
        }
    }

    /// Re-resolve the effective config from the client's settings plus the
    /// project file (the latter wins), then refresh caches and diagnostics.
    async fn reload_config(&self) {
        let client = self.state.client_options.read().await.clone();
        let project = self.read_project_config().await;
        let resolved = Config::resolve(client.as_ref(), project.as_ref());
        *self.state.config.write().await = resolved;
        self.state.analyses.clear();
        self.state.revalidate_all().await;
    }

    /// Compute the formatted document for one of the reference transforms.
    async fn formatted(&self, key: &str, inline: bool) -> Option<(Rope, String, String)> {
        let config = self.config().await;
        let rope = self.state.doc_rope(key)?;
        let source = rope.to_string();
        let out = if inline {
            formatting::to_inline_links(&source, config.gfm, &config.formatting.references_heading)
        } else {
            formatting::to_reference_links(
                &source,
                config.gfm,
                true,
                &config.formatting.references_heading,
            )
        };
        Some((rope, source, out))
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Negotiate position encoding: prefer UTF-8 if the client supports it.
        let enc = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .map(|encs| {
                if encs.contains(&PositionEncodingKind::UTF8) {
                    PositionEncoding::Utf8
                } else {
                    PositionEncoding::Utf16
                }
            })
            .unwrap_or(PositionEncoding::Utf16);
        *self.state.encoding.write().await = enc;

        // Remember whether the client can honour `window/showDocument`, so the
        // journal-note command can fall back gracefully when it can't.
        *self.state.supports_show_document.write().await = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.show_document.as_ref())
            .is_some_and(|c| c.support);

        // Remember the client's settings so the effective config can be
        // re-resolved later (e.g. when the project file changes).
        *self.state.client_options.write().await = params.initialization_options.clone();

        // Determine a workspace root for absolute-path resolution.
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|folders| folders.first())
            .and_then(|f| uri::to_path(&f.uri))
            .or_else(|| {
                #[allow(deprecated)]
                params.root_uri.as_ref().and_then(uri::to_path)
            });
        *self.state.workspace_root.write().await = root;

        // Resolve config = defaults <- client settings <- project file.
        let project = self.read_project_config().await;
        *self.state.config.write().await =
            Config::resolve(params.initialization_options.as_ref(), project.as_ref());

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "sanemark".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: server_capabilities(encoding_kind(enc)),
            offset_encoding: None,
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        // Ask the client to notify us about filesystem changes so broken-link
        // diagnostics can refresh when targets are created/removed. This is a
        // server->client request; run it in the background so we never block the
        // `initialized` handler waiting on the client's response.
        let state = self.state.clone();
        tokio::spawn(async move {
            let registration = Registration {
                id: "sanemark-watch".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![FileSystemWatcher {
                        glob_pattern: GlobPattern::String("**/*".to_string()),
                        kind: None,
                    }],
                })
                .ok(),
            };
            let _ = state.client.register_capability(vec![registration]).await;
            state
                .client
                .log_message(MessageType::INFO, "sanemark initialized")
                .await;
        });
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        let key = uri::key(&doc.uri);
        self.state
            .documents
            .insert(key.clone(), Document::new(&doc.text, doc.version));
        self.state.analyses.remove(&key);
        self.state.run_diagnostics(doc.uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let key = uri::key(&uri);
        let enc = self.encoding().await;

        if let Some(mut doc) = self.state.documents.get_mut(&key) {
            for change in &params.content_changes {
                doc.apply_change(change, enc);
            }
            doc.version = params.text_document.version;
        }
        self.state.analyses.remove(&key);
        self.schedule_diagnostics(uri);
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        self.state.run_diagnostics(params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let key = uri::key(&uri);
        self.state.documents.remove(&key);
        self.state.analyses.remove(&key);
        // Clear diagnostics for the closed document.
        self.state
            .client
            .publish_diagnostics(uri, Vec::new(), None)
            .await;
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        // Only replace our config when the payload actually contains our
        // settings. Some clients (e.g. Zed, when the server is launched via a
        // borrowed adapter) send an empty/unrelated `settings` object here,
        // which previously reset the config to defaults and silently disabled
        // formatting features configured via `initializationOptions`.
        if crate::config::looks_like_ours(&params.settings) {
            *self.state.client_options.write().await = Some(params.settings);
            // Re-resolve against the project file (which keeps priority).
            self.reload_config().await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // If the project config file changed, re-resolve settings (this also
        // revalidates). Otherwise a target file may have appeared/disappeared,
        // so just re-check open docs.
        let config_changed = params.changes.iter().any(|c| {
            uri::to_path(&c.uri)
                .is_some_and(|p| PROJECT_CONFIG_FILES.iter().any(|name| p.ends_with(name)))
        });
        if config_changed {
            self.reload_config().await;
        } else {
            self.state.revalidate_all().await;
        }
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let key = uri::key(&params.text_document.uri);
        let config = self.config().await;
        let (Some(rope), Some(analysis)) =
            (self.state.doc_rope(&key), self.state.analysis_for(&key))
        else {
            return Ok(None);
        };
        Ok(Some(folding::folding_ranges(
            &analysis,
            &rope,
            &config.folding,
        )))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let key = uri::key(&uri);
        let config = self.config().await;
        let enc = self.encoding().await;
        let Some(rope) = self.state.doc_rope(&key) else {
            return Ok(None);
        };
        let doc_path = uri::to_path(&uri);
        let root = self.workspace_root().await;
        let mut items = completion::complete(
            &rope,
            position,
            enc,
            &config.completion,
            doc_path.as_deref(),
            root.as_deref(),
        );
        // Quick `@`/`/` command snippets (dates, times, file links). These are
        // mutually exclusive with path completion by construction (the trigger
        // must sit at a word boundary, never inside a `](…)` destination).
        items.extend(snippets::complete(
            &rope,
            position,
            enc,
            &config,
            chrono::Local::now().naive_local(),
            doc_path.as_deref(),
            root.as_deref(),
        ));
        // Mark the list *incomplete* so the client re-queries the server as the
        // user keeps typing, instead of just filtering this (workspace-capped)
        // batch client-side. Without this, a fuzzy match like `@a_fold` could
        // never surface a file that fell outside the initial `max_items` cut for
        // the empty query — the whole point of the deep, workspace-wide walk.
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: true,
            items,
        })))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let key = uri::key(&uri);
        let enc = self.encoding().await;
        let Some(rope) = self.state.doc_rope(&key) else {
            return Ok(None);
        };
        let Some(analysis) = self.state.analysis_for(&key) else {
            return Ok(None);
        };
        let doc_path = uri::to_path(&uri);
        let root = self.workspace_root().await;
        Ok(hover::hover(
            &analysis,
            &rope,
            position,
            enc,
            doc_path.as_deref(),
            root.as_deref(),
        ))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let key = uri::key(&params.text_document.uri);
        let enc = self.encoding().await;
        let (Some(rope), Some(analysis)) =
            (self.state.doc_rope(&key), self.state.analysis_for(&key))
        else {
            return Ok(None);
        };
        let syms = symbols::document_symbols(&analysis, &rope, enc);
        Ok(Some(DocumentSymbolResponse::Nested(syms)))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let key = uri::key(&params.text_document.uri);
        let config = self.config().await;
        let Some(rope) = self.state.doc_rope(&key) else {
            return Ok(None);
        };
        let source = rope.to_string();
        let formatted = formatting::format_document(&source, config.gfm, &config.formatting);
        if formatted == source {
            return Ok(None);
        }
        let enc = self.encoding().await;
        Ok(Some(vec![full_document_edit(&rope, formatted, enc)]))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let key = uri::key(&uri);
        let enc = self.encoding().await;
        let (Some(rope), Some(analysis)) =
            (self.state.doc_rope(&key), self.state.analysis_for(&key))
        else {
            return Ok(None);
        };
        let doc_path = uri::to_path(&uri);
        let root = self.workspace_root().await;
        let ctx = navigation::NavContext {
            rope: &rope,
            enc,
            doc_uri: &uri,
            doc_path: doc_path.as_deref(),
            workspace_root: root.as_deref(),
        };
        let locations = navigation::goto_definition(&analysis, position, &ctx);
        Ok(locations.map(GotoDefinitionResponse::Array))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        let key = uri::key(&uri);
        let enc = self.encoding().await;
        let (Some(rope), Some(analysis)) =
            (self.state.doc_rope(&key), self.state.analysis_for(&key))
        else {
            return Ok(None);
        };
        let doc_path = uri::to_path(&uri);
        let root = self.workspace_root().await;
        Ok(Some(document_link::document_links(
            &analysis,
            &rope,
            enc,
            doc_path.as_deref(),
            root.as_deref(),
        )))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        let key = uri::key(&uri);
        let enc = self.encoding().await;
        let (Some(rope), Some(analysis)) =
            (self.state.doc_rope(&key), self.state.analysis_for(&key))
        else {
            return Ok(None);
        };
        let doc_path = uri::to_path(&uri);
        let root = self.workspace_root().await;
        let ctx = navigation::NavContext {
            rope: &rope,
            enc,
            doc_uri: &uri,
            doc_path: doc_path.as_deref(),
            workspace_root: root.as_deref(),
        };
        Ok(navigation::references(
            &analysis,
            position,
            &ctx,
            include_declaration,
        ))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri_arg = serde_json::Value::String(params.text_document.uri.as_str().to_string());
        let range_arg = serde_json::to_value(params.range).unwrap_or(serde_json::Value::Null);
        let actions = vec![
            command_action(
                "Markdown: Move link references to bottom",
                CodeActionKind::REFACTOR_REWRITE,
                CMD_MOVE_REFS,
                vec![uri_arg.clone()],
            ),
            command_action(
                "Markdown: Inline link references",
                CodeActionKind::REFACTOR_REWRITE,
                CMD_INLINE_REFS,
                vec![uri_arg.clone()],
            ),
            command_action(
                "Markdown: Copy as inlined Markdown",
                CodeActionKind::REFACTOR_INLINE,
                CMD_COPY_INLINED,
                vec![uri_arg.clone(), range_arg.clone()],
            ),
            command_action(
                "Markdown: Convert selection to inline links",
                CodeActionKind::REFACTOR_INLINE,
                CMD_CONVERT_INLINE,
                vec![uri_arg, range_arg],
            ),
            // Journal notes: not tied to the cursor, but offered here so they can
            // be triggered from the editor's code-action menu. The argument is a
            // day offset from today.
            command_action(
                "Markdown: Open today's journal note",
                CodeActionKind::EMPTY,
                CMD_DAILY_NOTE,
                vec![serde_json::Value::from(0i64)],
            ),
            command_action(
                "Markdown: Open yesterday's journal note",
                CodeActionKind::EMPTY,
                CMD_DAILY_NOTE,
                vec![serde_json::Value::from(-1i64)],
            ),
            command_action(
                "Markdown: Open tomorrow's journal note",
                CodeActionKind::EMPTY,
                CMD_DAILY_NOTE,
                vec![serde_json::Value::from(1i64)],
            ),
        ];
        Ok(Some(actions))
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        let args = &params.arguments;
        match params.command.as_str() {
            CMD_MOVE_REFS | CMD_INLINE_REFS => {
                let Some(uri) = arg_uri(args, 0) else {
                    return Ok(None);
                };
                self.apply_reference_transform(uri, params.command == CMD_INLINE_REFS)
                    .await;
                Ok(None)
            }
            // Inline the selection (or whole document) *in place* by editing the
            // buffer.
            CMD_CONVERT_INLINE => {
                let Some(uri) = arg_uri(args, 0) else {
                    return Ok(None);
                };
                self.apply_inline(uri, arg_range(args, 1)).await;
                Ok(None)
            }
            // Put the inlined Markdown on the system clipboard *without* touching
            // the document, and also return it as the command result (for
            // programmatic clients). References are resolved against the whole
            // document, so a copied selection is self-contained.
            CMD_COPY_INLINED => {
                let Some(uri) = arg_uri(args, 0) else {
                    return Ok(None);
                };
                let Some(text) = self.inlined_markdown(&uri, arg_range(args, 1)).await else {
                    return Ok(None);
                };
                self.copy_to_clipboard(text.clone()).await;
                Ok(Some(serde_json::Value::String(text)))
            }
            // Open (creating from the template if needed) the daily note for
            // `today + <offset>` days. Argument 0 is the integer offset.
            CMD_DAILY_NOTE => {
                let offset = args.first().and_then(|v| v.as_i64()).unwrap_or(0);
                self.open_daily_note(offset).await;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

impl Backend {
    /// Rewrite the whole document with one of the reference transforms via a
    /// client `applyEdit`.
    async fn apply_reference_transform(&self, doc_uri: Uri, inline: bool) {
        let key = uri::key(&doc_uri);
        let Some((rope, source, formatted)) = self.formatted(&key, inline).await else {
            return;
        };
        if formatted == source {
            return;
        }
        let enc = self.encoding().await;
        let edit = full_document_edit(&rope, formatted, enc);
        let mut changes = HashMap::new();
        changes.insert(doc_uri, vec![edit]);
        let _ = self
            .state
            .client
            .apply_edit(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            })
            .await;
    }

    /// Produce the inlined-Markdown text for a document (whole document, or just
    /// `range` when given). References are resolved against the whole document.
    async fn inlined_markdown(&self, doc_uri: &Uri, range: Option<Range>) -> Option<String> {
        let key = uri::key(doc_uri);
        let config = self.config().await;
        let rope = self.state.doc_rope(&key)?;
        let source = rope.to_string();
        let heading = &config.formatting.references_heading;
        let text = match range {
            Some(r) => {
                let enc = self.encoding().await;
                let start = rope.char_to_byte(position_to_char(&rope, r.start, enc));
                let end = rope.char_to_byte(position_to_char(&rope, r.end, enc));
                formatting::inline_links_in_range(
                    &source,
                    config.gfm,
                    heading,
                    start.min(end),
                    start.max(end),
                )
            }
            None => formatting::to_inline_links(&source, config.gfm, heading),
        };
        Some(text)
    }

    /// Inline links *in place*: replace the selection (or whole document) with
    /// its inlined form via a client `applyEdit`.
    async fn apply_inline(&self, doc_uri: Uri, range: Option<Range>) {
        let Some(r) = range else {
            // No selection: inline the whole document (same as `inlineReferences`).
            return self.apply_reference_transform(doc_uri, true).await;
        };
        let key = uri::key(&doc_uri);
        let config = self.config().await;
        let Some(rope) = self.state.doc_rope(&key) else {
            return;
        };
        let enc = self.encoding().await;
        let source = rope.to_string();
        let start = rope.char_to_byte(position_to_char(&rope, r.start, enc));
        let end = rope.char_to_byte(position_to_char(&rope, r.end, enc));
        let (start, end) = (start.min(end), start.max(end));
        let inlined = formatting::inline_links_in_range(
            &source,
            config.gfm,
            &config.formatting.references_heading,
            start,
            end,
        );
        // Skip a no-op edit when the selection has nothing to inline.
        if source.get(start..end) == Some(inlined.as_str()) {
            return;
        }
        let mut changes = HashMap::new();
        changes.insert(
            doc_uri,
            vec![TextEdit {
                range: r,
                new_text: inlined,
            }],
        );
        let _ = self
            .state
            .client
            .apply_edit(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            })
            .await;
    }

    /// Copy `text` to the system clipboard off the async runtime, reporting the
    /// outcome to the user via `window/showMessage`.
    async fn copy_to_clipboard(&self, text: String) {
        let result = tokio::task::spawn_blocking(move || clipboard::copy(&text)).await;
        let (kind, message) = match result {
            Ok(Ok(tool)) => (
                MessageType::INFO,
                format!("Copied inlined Markdown to the clipboard (via {tool})"),
            ),
            Ok(Err(e)) => (MessageType::ERROR, format!("Copy failed: {e}")),
            Err(e) => (MessageType::ERROR, format!("Copy failed: {e}")),
        };
        self.state.client.show_message(kind, message).await;
    }

    /// Open the journal note for `today + offset_days`, creating it from the
    /// configured template (or a minimal default) when it doesn't exist yet.
    async fn open_daily_note(&self, offset_days: i64) {
        let Some(root) = self.workspace_root().await else {
            self.state
                .client
                .show_message(
                    MessageType::ERROR,
                    "Journal notes need a workspace folder; open one first.".to_string(),
                )
                .await;
            return;
        };
        let config = self.config().await;
        let date = chrono::Local::now().date_naive() + chrono::Duration::days(offset_days);

        let note = match journal::ensure(&root, &config.journal, date) {
            Ok(note) => note,
            Err(e) => {
                self.state
                    .client
                    .show_message(
                        MessageType::ERROR,
                        format!("Could not open journal note: {e}"),
                    )
                    .await;
                return;
            }
        };

        let Some(uri) = uri::from_path(&note.path) else {
            return;
        };

        // Open in a background task so this `executeCommand` reply is sent
        // *first*. Some clients (e.g. Zed) only act on a server->client
        // `window/showDocument` once the command that triggered it has returned;
        // awaiting the open inline is what makes "nothing jumps" happen. If the
        // client can't show documents (or the request fails), fall back to a
        // message naming the note so it's never a silent no-op.
        let state = self.state.clone();
        let supported = *self.state.supports_show_document.read().await;
        let path = note.path.display().to_string();
        tokio::spawn(async move {
            if supported {
                match state
                    .client
                    .show_document(ShowDocumentParams {
                        uri,
                        external: Some(false),
                        take_focus: Some(true),
                        selection: None,
                    })
                    .await
                {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(e) => {
                        state
                            .client
                            .log_message(MessageType::WARNING, format!("showDocument failed: {e}"))
                            .await;
                    }
                }
            }
            state
                .client
                .show_message(MessageType::INFO, format!("Journal note ready: {path}"))
                .await;
        });
    }
}

/// Parse the `i`th command argument as a document [`Uri`].
fn arg_uri(args: &[serde_json::Value], i: usize) -> Option<Uri> {
    args.get(i)?.as_str()?.parse::<Uri>().ok()
}

/// Parse the `i`th command argument as a non-empty selection [`Range`].
fn arg_range(args: &[serde_json::Value], i: usize) -> Option<Range> {
    args.get(i)
        .and_then(|v| serde_json::from_value::<Range>(v.clone()).ok())
        .filter(|r| r.start != r.end)
}

fn encoding_kind(enc: PositionEncoding) -> PositionEncodingKind {
    match enc {
        PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
        PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
    }
}

fn server_capabilities(encoding: PositionEncodingKind) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(encoding),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                "(".to_string(),
                "/".to_string(),
                ".".to_string(),
                "@".to_string(),
            ]),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![
                CMD_MOVE_REFS.to_string(),
                CMD_INLINE_REFS.to_string(),
                CMD_COPY_INLINED.to_string(),
                CMD_CONVERT_INLINE.to_string(),
                CMD_DAILY_NOTE.to_string(),
            ],
            ..Default::default()
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}

fn command_action(
    title: &str,
    kind: CodeActionKind,
    command: &str,
    args: Vec<serde_json::Value>,
) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(kind),
        command: Some(Command {
            title: title.to_string(),
            command: command.to_string(),
            arguments: Some(args),
        }),
        ..Default::default()
    })
}

/// A [`TextEdit`] that replaces the entire document.
fn full_document_edit(rope: &Rope, new_text: String, enc: PositionEncoding) -> TextEdit {
    let end = char_to_position(rope, rope.len_chars(), enc);
    TextEdit {
        range: Range::new(Position::new(0, 0), end),
        new_text,
    }
}
