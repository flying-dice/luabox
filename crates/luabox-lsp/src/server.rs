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

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest,
    FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation, GotoTypeDefinition,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, RangeFormatting, References, Rename,
    Request as _, SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest,
    WorkspaceSymbolRequest,
};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyServerCapability, CodeAction, CodeActionKind, CodeActionOrCommand,
    CodeActionProviderCapability, CompletionOptions, CompletionResponse,
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentHighlight, DocumentSymbolResponse, FoldingRange, FoldingRangeProviderCapability,
    GotoDefinitionResponse, Hover, HoverProviderCapability, ImplementationProviderCapability,
    InitializeParams, InitializeResult, InlayHint, Location, OneOf, PrepareRenameResponse,
    PublishDiagnosticsParams, RenameOptions, SelectionRange, SelectionRangeProviderCapability,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo, SignatureHelp,
    SignatureHelpOptions, SymbolInformation, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, TypeDefinitionProviderCapability, Uri, WorkspaceEdit, WorkspaceSymbolResponse,
};
use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};
use luabox_lint::{LintConfig, lint_source};
use luabox_resolve::manifest::{Dependency, Lint, LintLevel, Manifest};
use luabox_types::{Ambient, combined_defs};

use crate::line_index::LineIndex;
use crate::sema::FileSema;
use crate::uri::uri_to_path;
use crate::{
    call_hierarchy, code_action, completion, diagnostics, document_highlight, fmt, folding,
    goto_def, goto_impl, goto_type, hover, inlay_hints, references, rename, selection_range,
    semantic_tokens, signature_help, symbols,
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
/// semantic tokens (full) with a standard-types-only legend, inlay hints
/// (inferred binding types, see [`crate::inlay_hints`]), rename with prepare
/// support (see [`crate::rename`]), document highlight (read/write tagged,
/// see [`crate::document_highlight`]), folding ranges (see [`crate::folding`]),
/// selection ranges (see [`crate::selection_range`]), workspace symbols
/// (fuzzy, case-insensitive name search across every file, see
/// [`crate::symbols::workspace_symbols`]), quick-fix code actions for
/// machine-applicable lint fixes (see [`Server::code_actions`]), signature
/// help triggered on `(`/`,` and retriggered on `,` (see
/// [`crate::signature_help`]), and call hierarchy (prepare/incoming/outgoing,
/// see [`crate::call_hierarchy`]).
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: Some(vec![",".to_string()]),
            ..SignatureHelpOptions::default()
        }),
        // Goto type-definition (value → its `---@class`/`---@alias`/`---@enum`,
        // see [`crate::goto_type`]) and goto-implementation (interface class →
        // its subclasses, see [`crate::goto_impl`]).
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // `prepare_provider` advertises textDocument/prepareRename, so the
        // editor pre-selects the identifier before prompting for a new name.
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp_types::WorkDoneProgressOptions::default(),
        })),
        inlay_hint_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
            ..CompletionOptions::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        // Quick-fixes for machine-applicable lint fixes (SPEC.md §8/§9).
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
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
    /// Ambient definition-package sources, winner-first (SPEC.md §3, #108):
    /// the project's own `[types] defs` then each direct dependency's defs
    /// (the luals `workspace.library` model), so the editor's ambient scope
    /// matches `luabox check`'s. Combined with the dialect stdlib into the
    /// server's [`Ambient`].
    def_sources: Vec<String>,
    /// The resolved `[lint]` configuration (tiers/rules/allowed globals),
    /// built from `manifest.lint` the same way `luabox lint` builds it, so the
    /// editor honours the project's lint config exactly as the CLI does.
    lint: LintConfig,
}

impl ProjectConfig {
    fn discover(root: &Path) -> Self {
        let defaults = Self {
            dialect: Dialect::Lua54,
            strictness: Strictness::Warn,
            out_dir: None,
            def_sources: Vec::new(),
            lint: LintConfig::new(),
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
            def_sources: ambient_def_sources(root, &manifest),
            lint: build_lint_config(&manifest.lint),
        }
    }
}

/// Translate the manifest `[lint]` table into a [`LintConfig`] — the id-level
/// then tier-level overrides plus the `global-write` allow-list. Mirrors
/// `luabox-cli::lint_cmd::build_config`; the LSP crate cannot depend on
/// `luabox-cli`, so this is duplicated the same way `ambient_def_sources` is.
fn build_lint_config(lint: &Lint) -> LintConfig {
    let mut config = LintConfig::new();
    for name in &lint.globals {
        config.allow_global(name.clone());
    }
    for (tier, level) in &lint.tiers {
        config.set_tier(tier, lint_level_keyword(*level));
    }
    for (rule, level) in &lint.rules {
        config.set_rule(rule, lint_level_keyword(*level));
    }
    config
}

/// The `LintConfig` level keyword for a manifest [`LintLevel`] (mirrors
/// `lint_cmd::level_keyword`).
fn lint_level_keyword(level: LintLevel) -> &'static str {
    match level {
        LintLevel::Allow => "allow",
        LintLevel::Warn => "warn",
        LintLevel::Deny => "deny",
    }
}

/// Resolve the ambient definition-package sources for a project, winner-first
/// (SPEC.md §3, #108): the project's own `[types] defs` from `<root>/defs/`,
/// then every direct dependency's own `[types] defs` from that dependency's
/// `defs/` (the luals `workspace.library` model). Mirrors
/// `check_cmd::resolve_project_defs` + `resolve_dep_defs` in the CLI — the LSP
/// crate cannot depend on `luabox-cli`, so this join is duplicated here the
/// same way `resolve_dep_shape_exports` already is. The editor and CI thus
/// build the same ambient scope. Cross-package class collisions (`LB0307`) are
/// a project-wide, check-time concern and are not surfaced per file here.
fn ambient_def_sources(root: &Path, manifest: &Manifest) -> Vec<String> {
    let mut sources = Vec::new();
    load_defs_from(&root.join("defs"), &manifest.types.defs, &mut sources);

    // `[dependencies]` + `[dev-dependencies]`, alphabetical by name (the
    // deterministic winner order), one level deep only.
    let mut deps: Vec<(&String, &Dependency)> = manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
        .collect();
    deps.sort_by(|a, b| a.0.cmp(b.0));
    for (name, dep) in deps {
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
        load_defs_from(
            &dep_root.join("defs"),
            &dep_manifest.types.defs,
            &mut sources,
        );
    }
    sources
}

/// Append the `.d.lua` texts for each `[types] defs` entry resolved against
/// `defs_dir` (`<name>.d.lua`, or every `*.d.lua` under `<name>/`), sorted.
fn load_defs_from(defs_dir: &Path, names: &[String], out: &mut Vec<String>) {
    for name in names {
        let single = defs_dir.join(format!("{name}.d.lua"));
        if single.is_file()
            && let Ok(text) = fs::read_to_string(&single)
        {
            out.push(text);
        }
        let dir = defs_dir.join(name);
        if dir.is_dir() {
            let mut files = Vec::new();
            collect_d_lua(&dir, &mut files);
            files.sort();
            for file in files {
                if let Ok(text) = fs::read_to_string(&file) {
                    out.push(text);
                }
            }
        }
    }
}

/// Collect every `*.d.lua` file under `dir`, recursively (mirrors the CLI's
/// helper of the same name).
fn collect_d_lua(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_d_lua(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("lua")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".d.lua"))
        {
            out.push(path);
        }
    }
}

/// The server state: the analysis host over the project's `.lua` files.
struct Server {
    connection: Connection,
    host: AnalysisHost,
    root: PathBuf,
    dialect: Dialect,
    strictness: Strictness,
    out_dir: Option<PathBuf>,
    /// The ambient definition-package layer (dialect stdlib + project defs +
    /// dependency defs, #108), built once at startup so the editor's type
    /// resolution matches `luabox check`.
    ambient: Ambient,
    /// The resolved `[lint]` configuration, driving the lint pass in
    /// [`Self::publish_lua`] and the quick-fixes in [`Self::code_actions`].
    lint: LintConfig,
    /// The `undefined-global` known-globals baseline (dialect stdlib + project
    /// and dependency defs), derived from [`Self::ambient`] the same way
    /// `luabox lint` derives it, so the `undefined-global` rule sees the same
    /// globals in the editor as under the CLI.
    known_globals: HashSet<String>,
}

impl Server {
    fn new(connection: Connection, root: PathBuf) -> Self {
        let config = ProjectConfig::discover(&root);
        let ambient = combined_defs(config.dialect, &config.def_sources);
        let known_globals = ambient.global_names().clone();
        Self {
            connection,
            host: AnalysisHost::new(config.dialect, config.strictness),
            root,
            dialect: config.dialect,
            strictness: config.strictness,
            out_dir: config.out_dir,
            ambient,
            lint: config.lint,
            known_globals,
        }
    }

    /// Load every `.lua` file under the root into the host (so
    /// `project_diagnostics` and cross-file goto have the full picture).
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
                if path.extension().and_then(|e| e.to_str()) == Some("lua")
                    && let Ok(text) = fs::read_to_string(&path)
                {
                    self.host.apply_change(Change::SetFileText {
                        path,
                        dialect: self.dialect,
                        text,
                    });
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

    #[allow(
        clippy::too_many_lines,
        reason = "a flat per-method dispatch table — one arm per LSP request, each a few lines"
    )]
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
            GotoTypeDefinition::METHOD => {
                let (id, params) = cast_request::<GotoTypeDefinition>(req)?;
                let doc = params.text_document_position_params;
                let result = self
                    .type_definition(&doc.text_document.uri, doc.position)
                    .map(GotoDefinitionResponse::Scalar);
                Response::new_ok(id, result)
            }
            GotoImplementation::METHOD => {
                let (id, params) = cast_request::<GotoImplementation>(req)?;
                let doc = params.text_document_position_params;
                let result = self
                    .implementation(&doc.text_document.uri, doc.position)
                    .map(GotoDefinitionResponse::Array);
                Response::new_ok(id, result)
            }
            References::METHOD => {
                let (id, params) = cast_request::<References>(req)?;
                let doc = params.text_document_position;
                let result = self.references(
                    &doc.text_document.uri,
                    doc.position,
                    params.context.include_declaration,
                );
                Response::new_ok(id, result)
            }
            Rename::METHOD => {
                let (id, params) = cast_request::<Rename>(req)?;
                let doc = params.text_document_position;
                let result = self.rename(&doc.text_document.uri, doc.position, &params.new_name);
                Response::new_ok(id, result)
            }
            PrepareRenameRequest::METHOD => {
                let (id, params) = cast_request::<PrepareRenameRequest>(req)?;
                let result = self.prepare_rename(&params.text_document.uri, params.position);
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
            WorkspaceSymbolRequest::METHOD => {
                let (id, params) = cast_request::<WorkspaceSymbolRequest>(req)?;
                let result = WorkspaceSymbolResponse::Flat(self.workspace_symbols(&params.query));
                Response::new_ok(id, Some(result))
            }
            DocumentHighlightRequest::METHOD => {
                let (id, params) = cast_request::<DocumentHighlightRequest>(req)?;
                let doc = params.text_document_position_params;
                let result = self.document_highlight(&doc.text_document.uri, doc.position);
                Response::new_ok(id, result)
            }
            FoldingRangeRequest::METHOD => {
                let (id, params) = cast_request::<FoldingRangeRequest>(req)?;
                let result = self.folding_ranges(&params.text_document.uri);
                Response::new_ok(id, result)
            }
            SelectionRangeRequest::METHOD => {
                let (id, params) = cast_request::<SelectionRangeRequest>(req)?;
                let result = self.selection_ranges(&params.text_document.uri, &params.positions);
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
            CodeActionRequest::METHOD => {
                let (id, params) = cast_request::<CodeActionRequest>(req)?;
                let result = self.code_actions(&params.text_document.uri, params.range);
                Response::new_ok(id, result)
            }
            SignatureHelpRequest::METHOD => {
                let (id, params) = cast_request::<SignatureHelpRequest>(req)?;
                let doc = params.text_document_position_params;
                let result = self.signature_help(&doc.text_document.uri, doc.position);
                Response::new_ok(id, result)
            }
            CallHierarchyPrepare::METHOD => {
                let (id, params) = cast_request::<CallHierarchyPrepare>(req)?;
                let doc = params.text_document_position_params;
                let result = self.prepare_call_hierarchy(&doc.text_document.uri, doc.position);
                Response::new_ok(id, result)
            }
            CallHierarchyIncomingCalls::METHOD => {
                let (id, params) = cast_request::<CallHierarchyIncomingCalls>(req)?;
                let result = self.incoming_calls(&params.item);
                Response::new_ok(id, result)
            }
            CallHierarchyOutgoingCalls::METHOD => {
                let (id, params) = cast_request::<CallHierarchyOutgoingCalls>(req)?;
                let result = self.outgoing_calls(&params.item);
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
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        hover::hover(&sema, offset)
    }

    /// The callee's resolved signature(s) while `position` sits inside a
    /// call's argument list (see [`crate::signature_help`]).
    fn signature_help(&self, uri: &Uri, position: lsp_types::Position) -> Option<SignatureHelp> {
        let path = uri_to_path(uri)?;
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        signature_help::signature_help(&sema, offset)
    }

    /// The call-hierarchy item for the function the cursor names at `position`
    /// — a declaration or a call site (see [`crate::call_hierarchy`]). Reuses
    /// one snapshot for the whole resolution.
    fn prepare_call_hierarchy(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<CallHierarchyItem>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        call_hierarchy::prepare(&snapshot, &sema, offset)
    }

    /// The call sites across the workspace that call `item`, grouped by their
    /// enclosing function (see [`crate::call_hierarchy`]).
    fn incoming_calls(&self, item: &CallHierarchyItem) -> Option<Vec<CallHierarchyIncomingCall>> {
        let path = uri_to_path(&item.uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        Some(call_hierarchy::incoming_calls(&snapshot, &sema, item))
    }

    /// The functions called within `item`'s body (see
    /// [`crate::call_hierarchy`]).
    fn outgoing_calls(&self, item: &CallHierarchyItem) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let path = uri_to_path(&item.uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        Some(call_hierarchy::outgoing_calls(&snapshot, &sema, item))
    }

    fn definition(&self, uri: &Uri, position: lsp_types::Position) -> Option<Location> {
        let path = uri_to_path(uri)?;
        let sema = self.sema(&path)?;
        let offset = sema.index.offset(position);
        goto_def::goto_definition(&sema, offset, &self.root)
    }

    /// The declaration of the type carried by the value at `position`: its
    /// `---@class`/`---@alias`/`---@enum`, searched workspace-wide (declarations
    /// are workspace-global). Reuses one snapshot for the whole scan.
    fn type_definition(&self, uri: &Uri, position: lsp_types::Position) -> Option<Location> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        goto_type::goto_type_definition(&snapshot, &sema, offset)
    }

    /// Every implementor of the `---@class` at `position`: each workspace class
    /// that lists it as a parent (see [`crate::goto_impl`]). Reuses one snapshot
    /// for the whole cross-file scan.
    fn implementation(&self, uri: &Uri, position: lsp_types::Position) -> Option<Vec<Location>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        goto_impl::goto_implementation(&snapshot, &sema, offset)
    }

    /// All references to the symbol at `position`. Locals/upvalues are found in
    /// the file itself; globals and class members are searched across every
    /// file the snapshot knows about, reusing one snapshot for the whole scan.
    fn references(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        references::references(&snapshot, &sema, offset, include_declaration)
    }

    /// A [`WorkspaceEdit`] renaming the symbol at `position` to `new_name`,
    /// touching every reference and its declaration across the workspace
    /// (reusing the same reference finder, then narrowing each edit to the bare
    /// identifier token; see [`crate::rename`]).
    fn rename(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
        new_name: &str,
    ) -> Option<WorkspaceEdit> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        rename::rename(&snapshot, &sema, offset, new_name)
    }

    /// The identifier range under `position` for the editor to pre-select, or
    /// `None` when the position is not a renameable symbol.
    fn prepare_rename(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<PrepareRenameResponse> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        rename::prepare_rename(&snapshot, &sema, offset).map(PrepareRenameResponse::Range)
    }

    /// Completions at `position`: scope/member items plus auto-require imports
    /// (see [`crate::completion`]). Reuses one snapshot for the whole pass —
    /// the auto-require enumeration reads every workspace file's memoized
    /// module export.
    fn completion(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        Some(completion::completion(&sema, offset, &snapshot, &self.root))
    }

    fn document_symbols(&self, uri: &Uri) -> Option<Vec<lsp_types::DocumentSymbol>> {
        let path = uri_to_path(uri)?;
        let sema = self.sema(&path)?;
        Some(symbols::document_symbols(&sema))
    }

    /// Fuzzy (case-insensitive substring) search for `query` across every
    /// `.lua` file the analysis snapshot knows about, reusing one snapshot
    /// for the whole scan (mirrors [`Self::references`]): classes, functions,
    /// fields/methods, and aliases/enums (see
    /// [`symbols::workspace_symbols`]). Results are deduplicated by name and
    /// location, sorted for a deterministic response, then capped at
    /// [`WORKSPACE_SYMBOL_LIMIT`] — an empty query would otherwise return
    /// every symbol in the workspace.
    fn workspace_symbols(&self, query: &str) -> Vec<SymbolInformation> {
        let snapshot = self.host.snapshot();
        let mut out: Vec<SymbolInformation> = Vec::new();
        let mut seen = HashSet::new();
        for path in snapshot.files() {
            let Some(sema) = FileSema::new(&snapshot, path) else {
                continue;
            };
            for info in symbols::workspace_symbols(&sema, query) {
                if seen.insert(workspace_symbol_key(&info)) {
                    out.push(info);
                }
            }
        }
        out.sort_by_key(workspace_symbol_key);
        out.truncate(WORKSPACE_SYMBOL_LIMIT);
        out
    }

    /// Every occurrence of the symbol at `position` in this file, tagged read
    /// or write (see [`crate::document_highlight`]); reuses [`references`]'
    /// classification, narrowed to the current file.
    fn document_highlight(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<DocumentHighlight>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        let offset = sema.index.offset(position);
        document_highlight::document_highlight(&snapshot, &sema, offset)
    }

    /// Folding regions for one file: blocks, table constructors, and comment
    /// runs (see [`crate::folding`]) — pure syntax-tree geometry, no
    /// semantic analysis needed.
    fn folding_ranges(&self, uri: &Uri) -> Option<Vec<FoldingRange>> {
        let path = uri_to_path(uri)?;
        let sema = self.sema(&path)?;
        Some(folding::folding_ranges(&sema))
    }

    /// The syntax-tree expand chain for each requested position (see
    /// [`crate::selection_range`]).
    fn selection_ranges(
        &self,
        uri: &Uri,
        positions: &[lsp_types::Position],
    ) -> Option<Vec<SelectionRange>> {
        let path = uri_to_path(uri)?;
        let sema = self.sema(&path)?;
        Some(selection_range::selection_ranges(&sema, positions))
    }

    /// Full-document formatting; also serves range requests (MVP semantics,
    /// see [`crate::fmt`]). `None` for unknown documents; `Some(vec![])`
    /// when nothing changed — including the formatters' parse-error
    /// "return input unchanged" guarantee, which must not become an error.
    fn formatting(&self, uri: &Uri) -> Option<Vec<TextEdit>> {
        let path = uri_to_path(uri)?;
        let text = self.host.snapshot().file_text(&path)?;
        let formatted = luabox_syntax::lua::fmt::format(&text, self.dialect);
        Some(fmt::full_document_edits(&text, &formatted))
    }

    fn semantic_tokens(&self, uri: &Uri) -> Option<SemanticTokensResult> {
        let path = uri_to_path(uri)?;
        let data = semantic_tokens::lua_tokens(&self.sema(&path)?);
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

    /// Quick-fix code actions for the requested `range`: run the lint engine
    /// on the file and offer each machine-applicable fix whose byte-range
    /// overlaps the request as a `quickfix`, carrying the `WorkspaceEdit` and
    /// the lint diagnostic it resolves. Uses the same lint config and known
    /// globals as [`Self::publish_lua`], and converts the referenced diagnostic
    /// with the same helper, so the action's diagnostic is byte-identical to
    /// the published one (the editor can pair them). `Some(vec![])` when the
    /// file is known but nothing applies.
    #[allow(
        clippy::mutable_key_type,
        reason = "WorkspaceEdit keys its edits by Uri; the lint's interior-mutability concern does not affect Uri's hash"
    )]
    fn code_actions(&self, uri: &Uri, range: lsp_types::Range) -> Option<Vec<CodeActionOrCommand>> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let text = snapshot.file_text(&path)?;
        let index = LineIndex::new(text);
        let rel = path.to_string_lossy();
        let outcome = lint_source(
            &rel,
            index.text(),
            self.dialect,
            &self.lint,
            &self.known_globals,
        );

        let start = index.offset(range.start);
        let end = index.offset(range.end);
        let mut actions = Vec::new();
        for fix in &outcome.fixes {
            // Inclusive overlap so a bare caret at either edge still offers it.
            if fix.range.end < start || fix.range.start > end {
                continue;
            }
            // The originating diagnostic carries the same edit as a suggestion
            // (`lint_source` mirrors every machine-applicable fix into both),
            // so match on it to reference the exact published diagnostic and to
            // title the action with the rule's own fix message.
            let source_diag = outcome.diagnostics.iter().find(|d| {
                d.suggestions
                    .iter()
                    .any(|s| s.span.range == fix.range && s.replacement == fix.replacement)
            });
            let title = source_diag
                .and_then(|d| d.suggestions.iter().find(|s| s.span.range == fix.range))
                .map_or_else(|| "Apply lint fix".to_string(), |s| s.message.clone());
            let mut changes = std::collections::HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: index.range(fix.range.clone()),
                    new_text: fix.replacement.clone(),
                }],
            );
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: source_diag
                    .map(|d| vec![diagnostics::convert(&index, d, diagnostics::LINT_SOURCE)]),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..WorkspaceEdit::default()
                }),
                ..CodeAction::default()
            }));
        }

        // Type-driven quick-fixes and refactors (#129), gathered alongside the
        // lint fixes above from the same request. The per-file semantic view
        // supplies the AST/annotations; the display inference supplies binding
        // types; the type diagnostics (recomputed with the same helper and
        // context as `publish_lua`, so an `LB0302` offered on a quick-fix is
        // byte-identical to the published one) drive add-missing-field.
        if let Some(sema) = FileSema::new(&snapshot, &path) {
            let inferred = snapshot.binding_types(&path);
            let ctx = diagnostics::CheckCtx {
                strictness: self.strictness,
                ambient: &self.ambient,
                lint: &self.lint,
                known_globals: &self.known_globals,
            };
            let type_diags = diagnostics::lua_diagnostics(&snapshot, &path, self.dialect, &ctx)
                .unwrap_or_default();
            actions.extend(code_action::type_actions(
                &sema,
                inferred.as_ref(),
                &type_diags,
                uri,
                start,
                end,
            ));
        }
        Some(actions)
    }

    fn sema(&self, path: &Path) -> Option<FileSema> {
        FileSema::new(&self.host.snapshot(), path)
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
        self.host.apply_change(Change::SetOverlay {
            path: path.clone(),
            text,
        });
        self.publish_lua(uri, &path)
    }

    /// didClose: drop the overlay, refreshing the disk layer first (the file
    /// may have been saved while open), then republish from disk state — or
    /// clear diagnostics entirely for scratch buffers with no disk backing.
    fn close(&mut self, uri: &Uri) -> anyhow::Result<()> {
        let Some(path) = uri_to_path(uri) else {
            return Ok(());
        };
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
        let ctx = diagnostics::CheckCtx {
            strictness: self.strictness,
            ambient: &self.ambient,
            lint: &self.lint,
            known_globals: &self.known_globals,
        };
        let diags =
            diagnostics::lua_diagnostics(&analysis, path, self.dialect, &ctx).unwrap_or_default();
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

/// Extract a request's id and params, or surface a protocol error.
fn cast_request<R: lsp_types::request::Request>(
    req: Request,
) -> anyhow::Result<(RequestId, R::Params)> {
    req.extract(R::METHOD)
        .map_err(|e| anyhow::anyhow!("malformed `{}` request: {e:?}", R::METHOD))
}

/// The cap on [`Server::workspace_symbols`]'s response: generous for any real
/// project, small enough that an empty (match-everything) query over a huge
/// workspace still returns promptly.
const WORKSPACE_SYMBOL_LIMIT: usize = 500;

/// A total order/dedup key over a workspace symbol: name, then location
/// (file, then range) — mirrors `references::key` over [`Location`]s. Owned
/// (rather than borrowing from `info`) so it can be used both to populate the
/// dedup set and, afterwards, to sort the same `info` values it was built from.
fn workspace_symbol_key(info: &SymbolInformation) -> (String, String, u32, u32, u32, u32) {
    (
        info.name.clone(),
        info.location.uri.as_str().to_string(),
        info.location.range.start.line,
        info.location.range.start.character,
        info.location.range.end.line,
        info.location.range.end.character,
    )
}
