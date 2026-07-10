//! The synchronous mainloop over an [`lsp_server::Connection`]
//! (rust-analyzer's shape: `lsp-server` over stdio, no async runtime).
//!
//! # Protocol choices (tranche 1)
//!
//! - **Sync**: `textDocumentSync` is **Full** — every `didChange` carries the
//!   whole buffer, which maps 1:1 onto the analysis host's
//!   `SetOverlay { text }`. Incremental sync is a later optimisation; the
//!   salsa layer already avoids re-analysing unaffected files.
//! - **Positions**: UTF-16 (the protocol default; no `positionEncoding`
//!   negotiation), converted at the boundary by
//!   [`LineIndex`](crate::line_index::LineIndex).
//! - **Diagnostics**: pushed via `textDocument/publishDiagnostics` after
//!   every open/change/close, computed from a fresh [`Analysis`] snapshot.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    InlayHintRequest, RangeFormatting, Request as _, SemanticTokensFullRequest,
};
use lsp_types::{
    CompletionOptions, CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolResponse, GotoDefinitionResponse, Hover,
    HoverProviderCapability, InitializeParams, InitializeResult, InlayHint, Location, OneOf,
    PublishDiagnosticsParams, SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions,
    SemanticTokensResult, SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Uri,
};
use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};
use luabox_resolve::manifest::{Dependency, Manifest};
use luabox_types::{DepShapeExport, ShapeStore};

use crate::line_index::LineIndex;
use crate::sema::FileSema;
use crate::uri::{path_to_uri, uri_to_path};
use crate::{
    completion, diagnostics, fmt, goto_def, hover, inlay_hints, luab, semantic_tokens, symbols,
};

/// Run the server over stdio until the client sends `shutdown`/`exit`.
/// A leading `--stdio` argument, which editors commonly pass, is harmless:
/// stdio is the only transport in this tranche.
pub fn run_stdio() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Run the server over any [`Connection`] (stdio in production,
/// [`Connection::memory`] in tests): initialize handshake, project
/// bootstrap, then the message loop. Returns after a clean shutdown.
pub fn run(connection: Connection) -> anyhow::Result<()> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let result = InitializeResult {
        capabilities: server_capabilities(),
        server_info: Some(ServerInfo {
            name: "luabox-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let root = root_path(&params)
        .or_else(|| std::env::current_dir().ok())
        .context("cannot determine a workspace root")?;
    let mut server = Server::new(connection, root);
    server.bootstrap();
    server.main_loop()
}

/// The capabilities advertised at initialize: full sync, hover, definition,
/// completion triggered on `.`/`:`, document symbols, whole-document and
/// range formatting (range formats the whole document — see [`crate::fmt`]),
/// semantic tokens (full) with a standard-types-only legend, and inlay
/// hints (inferred binding types, see [`crate::inlay_hints`]).
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
            ..CompletionOptions::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens::legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                ..SemanticTokensOptions::default()
            },
        )),
        ..ServerCapabilities::default()
    }
}

/// The workspace root from the initialize params (first workspace folder,
/// falling back to the deprecated `rootUri`).
fn root_path(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(folder) = params.workspace_folders.as_ref().and_then(|f| f.first())
        && let Some(path) = uri_to_path(&folder.uri)
    {
        return Some(path);
    }
    #[allow(
        deprecated,
        reason = "rootUri is the standard fallback for older clients"
    )]
    params.root_uri.as_ref().and_then(uri_to_path)
}

/// Project configuration read from `luabox.toml` at the workspace root
/// (falling back to Lua 5.4 / warn — the same defaults as `luabox check`).
struct ProjectConfig {
    dialect: Dialect,
    strictness: Strictness,
    /// The manifest's `[build] out` directory, skipped when walking.
    out_dir: Option<PathBuf>,
    /// `[types] shape-paths`, absolute, in manifest order (SHAPES.md §6).
    shape_paths: Vec<PathBuf>,
    /// Dependencies that export shape modules (SHAPES.md §6, tier 3).
    dependencies: Vec<DepShapeExport>,
}

impl ProjectConfig {
    fn discover(root: &Path) -> Self {
        let defaults = Self {
            dialect: Dialect::Lua54,
            strictness: Strictness::Warn,
            out_dir: None,
            shape_paths: Vec::new(),
            dependencies: Vec::new(),
        };
        let Ok(text) = fs::read_to_string(root.join("luabox.toml")) else {
            return defaults;
        };
        let Ok(manifest) = Manifest::parse(&text) else {
            eprintln!("luabox-lsp: invalid luabox.toml; using defaults (5.4, warn)");
            return defaults;
        };
        Self {
            dialect: Dialect::from_manifest_id(&manifest.package.edition).unwrap_or(Dialect::Lua54),
            strictness: Strictness::from_manifest_flag(manifest.types.strict),
            out_dir: Some(root.join(&manifest.build.out)),
            shape_paths: manifest
                .types
                .shape_paths
                .iter()
                .map(|p| root.join(p))
                .collect(),
            dependencies: resolve_dep_shape_exports(root, &manifest),
        }
    }
}

/// Build the dependency shape-export table for resolution tier 3
/// (SHAPES.md §6). Mirrors `check_cmd::resolve_dep_shape_exports` in the
/// CLI — the two frontends currently each own this manifest join;
/// unify if a third consumer appears.
fn resolve_dep_shape_exports(root: &Path, manifest: &Manifest) -> Vec<DepShapeExport> {
    let mut exports = Vec::new();
    for (name, dep) in manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
    {
        let dep_root = match dep {
            Dependency::Path(p) => root.join(p.path.replace('\\', "/")),
            _ => root.join("lua_modules").join(name),
        };
        let Ok(text) = fs::read_to_string(dep_root.join("luabox.toml")) else {
            continue;
        };
        let Ok(dep_manifest) = Manifest::parse(&text) else {
            continue;
        };
        if dep_manifest.types.shapes.is_empty() {
            continue;
        }
        exports.push(DepShapeExport {
            name: name.clone(),
            shape_paths: dep_manifest
                .types
                .shape_paths
                .iter()
                .map(|p| dep_root.join(p))
                .collect(),
            exported: dep_manifest.types.shapes.clone(),
            root: dep_root,
        });
    }
    exports
}

/// The server state: the analysis host plus `.luab` texts (which never enter
/// the Lua host — they are parsed with the shape grammar on demand).
struct Server {
    connection: Connection,
    host: AnalysisHost,
    root: PathBuf,
    dialect: Dialect,
    strictness: Strictness,
    out_dir: Option<PathBuf>,
    /// Effective text of `.luab` files: overlay (open buffers) over disk.
    lb_overlay: HashMap<PathBuf, String>,
    lb_disk: HashMap<PathBuf, String>,
    /// `[types] shape-paths`, absolute (SHAPES.md §6, tier 2).
    shape_paths: Vec<PathBuf>,
    /// Shape-exporting dependencies (SHAPES.md §6, tier 3).
    dependencies: Vec<DepShapeExport>,
    /// Shape-module parse cache for the shaped type pass. The store reads
    /// from disk, so it is dropped and rebuilt whenever a `.luab` buffer
    /// changes (the IDE auto-saves; unsaved shape edits are eventually
    /// consistent, not instant).
    shape_store: ShapeStore,
    /// `.lua` documents currently open in the editor, so a `.luab` change
    /// can republish exactly the diagnostics a user can see.
    open_lua: HashMap<PathBuf, Uri>,
}

impl Server {
    fn new(connection: Connection, root: PathBuf) -> Self {
        let config = ProjectConfig::discover(&root);
        Self {
            connection,
            host: AnalysisHost::new(config.dialect, config.strictness),
            shape_store: ShapeStore::new(root.clone()),
            root,
            dialect: config.dialect,
            strictness: config.strictness,
            out_dir: config.out_dir,
            lb_overlay: HashMap::new(),
            lb_disk: HashMap::new(),
            shape_paths: config.shape_paths,
            dependencies: config.dependencies,
            open_lua: HashMap::new(),
        }
    }

    /// Load every `.lua` file under the root into the host (so
    /// `project_diagnostics` and cross-file goto have the full picture) and
    /// remember `.luab` texts.
    fn bootstrap(&mut self) {
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let hidden = entry.file_name().to_string_lossy().starts_with('.');
                if hidden {
                    continue;
                }
                if path.is_dir() {
                    if self.out_dir.as_deref() != Some(path.as_path()) {
                        stack.push(path);
                    }
                    continue;
                }
                match path.extension().and_then(|e| e.to_str()) {
                    Some("lua") => {
                        if let Ok(text) = fs::read_to_string(&path) {
                            self.host.apply_change(Change::SetFileText {
                                path,
                                dialect: self.dialect,
                                text,
                            });
                        }
                    }
                    Some("luab") => {
                        if let Ok(text) = fs::read_to_string(&path) {
                            self.lb_disk.insert(path, text);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn main_loop(&mut self) -> anyhow::Result<()> {
        while let Ok(msg) = self.connection.receiver.recv() {
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    self.handle_request(req)?;
                }
                Message::Notification(not) => self.handle_notification(not)?,
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    // === Requests =========================================================

    fn handle_request(&mut self, req: Request) -> anyhow::Result<()> {
        let response = match req.method.as_str() {
            HoverRequest::METHOD => {
                let (id, params) = cast_request::<HoverRequest>(req)?;
                let doc = params.text_document_position_params;
                let result = self.hover(&doc.text_document.uri, doc.position);
                Response::new_ok(id, result)
            }
            GotoDefinition::METHOD => {
                let (id, params) = cast_request::<GotoDefinition>(req)?;
                let doc = params.text_document_position_params;
                let result = self
                    .definition(&doc.text_document.uri, doc.position)
                    .map(GotoDefinitionResponse::Scalar);
                Response::new_ok(id, result)
            }
            Completion::METHOD => {
                let (id, params) = cast_request::<Completion>(req)?;
                let doc = params.text_document_position;
                let result = self
                    .completion(&doc.text_document.uri, doc.position)
                    .map(CompletionResponse::Array);
                Response::new_ok(id, result)
            }
            DocumentSymbolRequest::METHOD => {
                let (id, params) = cast_request::<DocumentSymbolRequest>(req)?;
                let result = self
                    .document_symbols(&params.text_document.uri)
                    .map(DocumentSymbolResponse::Nested);
                Response::new_ok(id, result)
            }
            Formatting::METHOD => {
                let (id, params) = cast_request::<Formatting>(req)?;
                let result = self.formatting(&params.text_document.uri);
                Response::new_ok(id, result)
            }
            RangeFormatting::METHOD => {
                // MVP range semantics (see `crate::fmt`): the canonical
                // formatters are whole-file, so a range request returns the
                // same whole-document edit as a full format.
                let (id, params) = cast_request::<RangeFormatting>(req)?;
                let result = self.formatting(&params.text_document.uri);
                Response::new_ok(id, result)
            }
            SemanticTokensFullRequest::METHOD => {
                let (id, params) = cast_request::<SemanticTokensFullRequest>(req)?;
                let result = self.semantic_tokens(&params.text_document.uri);
                Response::new_ok(id, result)
            }
            InlayHintRequest::METHOD => {
                let (id, params) = cast_request::<InlayHintRequest>(req)?;
                let result = self.inlay_hints(&params.text_document.uri, params.range);
                Response::new_ok(id, result)
            }
            _ => Response::new_err(
                req.id,
                ErrorCode::MethodNotFound as i32,
                format!("unhandled method `{}`", req.method),
            ),
        };
        self.connection.sender.send(Message::Response(response))?;
        Ok(())
    }

    fn hover(&self, uri: &Uri, position: lsp_types::Position) -> Option<Hover> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            let text = self.lb_text(&path)?.to_string();
            let index = LineIndex::new(text);
            let offset = index.offset(position);
            let (range, decl) = luab::definition(index.text(), offset)?;
            return Some(Hover {
                contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: format!("```\n{}\n```", decl.trim()),
                }),
                range: Some(index.range(usize::from(range.start())..usize::from(range.end()))),
            });
        }
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        hover::hover(&sema, offset)
    }

    fn definition(&self, uri: &Uri, position: lsp_types::Position) -> Option<Location> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            let text = self.lb_text(&path)?.to_string();
            let index = LineIndex::new(text);
            let offset = index.offset(position);
            let (range, _) = luab::definition(index.text(), offset)?;
            return Some(Location {
                uri: path_to_uri(&path),
                range: index.range(usize::from(range.start())..usize::from(range.end())),
            });
        }
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        goto_def::goto_definition(&sema, offset, &self.root)
    }

    fn completion(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            return None;
        }
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        Some(completion::completion(&sema, offset))
    }

    fn document_symbols(&self, uri: &Uri) -> Option<Vec<lsp_types::DocumentSymbol>> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            return None;
        }
        let sema = self.sema(&path)?;
        Some(symbols::document_symbols(&sema))
    }

    /// Full-document formatting; also serves range requests (MVP semantics,
    /// see [`crate::fmt`]). `None` for unknown documents; `Some(vec![])`
    /// when nothing changed — including the formatters' parse-error
    /// "return input unchanged" guarantee, which must not become an error.
    fn formatting(&self, uri: &Uri) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            let text = self.lb_text(&path)?;
            let formatted = luabox_syntax::shape::format(text);
            return Some(fmt::full_document_edits(text, &formatted));
        }
        let text = self.host.snapshot().file_text(&path)?;
        let formatted = luabox_syntax::lua::fmt::format(&text, self.dialect);
        Some(fmt::full_document_edits(&text, &formatted))
    }

    fn semantic_tokens(&self, uri: &Uri) -> Option<SemanticTokensResult> {
        let path = uri_to_path(uri)?;
        let data = if is_lb(&path) {
            semantic_tokens::lb_tokens(self.lb_text(&path)?)
        } else {
            semantic_tokens::lua_tokens(&self.sema(&path)?)
        };
        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }

    /// Inlay hints for the visible `range` of a `.lua` document: the
    /// display-mode inference's binding types and inferred function
    /// returns (see [`crate::inlay_hints`]).
    fn inlay_hints(&self, uri: &Uri, range: lsp_types::Range) -> Option<Vec<InlayHint>> {
        let path = uri_to_path(uri)?;
        if is_lb(&path) {
            return None;
        }
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let inferred = snapshot.binding_types(&path)?;
        let start = sema.index.offset(range.start);
        let end = sema.index.offset(range.end);
        Some(inlay_hints::inlay_hints(
            &sema,
            inferred.bindings(),
            inferred.fn_returns(),
            start,
            end,
        ))
    }

    fn sema(&self, path: &Path) -> Option<FileSema> {
        FileSema::new(&self.host.snapshot(), path)
    }

    fn lb_text(&self, path: &Path) -> Option<&str> {
        self.lb_overlay
            .get(path)
            .or_else(|| self.lb_disk.get(path))
            .map(String::as_str)
    }

    // === Notifications ====================================================

    fn handle_notification(&mut self, not: Notification) -> anyhow::Result<()> {
        match not.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams = serde_json::from_value(not.params)?;
                let uri = params.text_document.uri;
                self.set_text(&uri, params.text_document.text)?;
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams = serde_json::from_value(not.params)?;
                // Full sync: the last change is the whole new buffer.
                if let Some(change) = params.content_changes.into_iter().next_back() {
                    self.set_text(&params.text_document.uri, change.text)?;
                }
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams = serde_json::from_value(not.params)?;
                self.close(&params.text_document.uri)?;
            }
            // `textDocument/didSave` is a deliberate no-op — the overlay is
            // already the saved content. Everything else is ignored.
            _ => {}
        }
        Ok(())
    }

    /// didOpen/didChange: overlay the new text, then publish diagnostics.
    fn set_text(&mut self, uri: &Uri, text: String) -> anyhow::Result<()> {
        let Some(path) = uri_to_path(uri) else {
            return Ok(());
        };
        if is_lb(&path) {
            let diags = diagnostics::lb_diagnostics(&text);
            self.lb_overlay.insert(path, text);
            self.publish(uri, diags)?;
            return self.refresh_shapes();
        }
        self.host.apply_change(Change::SetOverlay {
            path: path.clone(),
            text,
        });
        self.open_lua.insert(path.clone(), uri.clone());
        self.publish_lua(uri, &path)
    }

    /// A `.luab` module changed: drop the disk-backed parse cache and
    /// republish every open `.lua` document, whose shape bindings may now
    /// resolve differently.
    fn refresh_shapes(&mut self) -> anyhow::Result<()> {
        self.shape_store = ShapeStore::new(self.root.clone());
        let open: Vec<(PathBuf, Uri)> = self
            .open_lua
            .iter()
            .map(|(p, u)| (p.clone(), u.clone()))
            .collect();
        for (path, uri) in open {
            self.publish_lua(&uri, &path)?;
        }
        Ok(())
    }

    /// didClose: drop the overlay, refreshing the disk layer first (the file
    /// may have been saved while open), then republish from disk state — or
    /// clear diagnostics entirely for scratch buffers with no disk backing.
    fn close(&mut self, uri: &Uri) -> anyhow::Result<()> {
        let Some(path) = uri_to_path(uri) else {
            return Ok(());
        };
        if is_lb(&path) {
            self.lb_overlay.remove(&path);
            if let Ok(text) = fs::read_to_string(&path) {
                let diags = diagnostics::lb_diagnostics(&text);
                self.lb_disk.insert(path, text);
                self.publish(uri, diags)?;
            } else {
                self.lb_disk.remove(&path);
                self.publish(uri, Vec::new())?;
            }
            return self.refresh_shapes();
        }
        self.open_lua.remove(&path);
        if let Ok(text) = fs::read_to_string(&path) {
            self.host.apply_change(Change::SetFileText {
                path: path.clone(),
                dialect: self.dialect,
                text,
            });
            self.host
                .apply_change(Change::ClearOverlay { path: path.clone() });
            self.publish_lua(uri, &path)
        } else {
            self.host.apply_change(Change::ClearOverlay { path });
            self.publish(uri, Vec::new())
        }
    }

    /// Publish the current diagnostics for one `.lua` file from a fresh
    /// snapshot.
    fn publish_lua(&mut self, uri: &Uri, path: &Path) -> anyhow::Result<()> {
        let analysis: Analysis = self.host.snapshot();
        let shapes = diagnostics::ShapeCtx {
            store: &self.shape_store,
            shape_paths: &self.shape_paths,
            dependencies: &self.dependencies,
            strictness: self.strictness,
        };
        let diags = diagnostics::lua_diagnostics(&analysis, path, self.dialect, &shapes)
            .unwrap_or_default();
        self.publish(uri, diags)
    }

    fn publish(&self, uri: &Uri, diagnostics: Vec<lsp_types::Diagnostic>) -> anyhow::Result<()> {
        let params = PublishDiagnosticsParams {
            uri: uri.clone(),
            diagnostics,
            version: None,
        };
        self.connection
            .sender
            .send(Message::Notification(Notification::new(
                PublishDiagnostics::METHOD.to_string(),
                params,
            )))?;
        Ok(())
    }
}

fn is_lb(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("luab")
}

/// Extract a request's id and params, or surface a protocol error.
fn cast_request<R: lsp_types::request::Request>(
    req: Request,
) -> anyhow::Result<(RequestId, R::Params)> {
    req.extract(R::METHOD)
        .map_err(|e| anyhow::anyhow!("malformed `{}` request: {e:?}", R::METHOD))
}
