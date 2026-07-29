//! The synchronous mainloop over an [`lsp_server::Connection`]
//! (rust-analyzer's shape: `lsp-server` over stdio, no async runtime).
//!
//! # Protocol choices (tranche 1)
//!
//! - **Sync**: `textDocumentSync` is **Incremental** — each `didChange`
//!   carries a batch of ranged edits, applied in order against the current
//!   overlay text (see [`apply_content_changes`]) to produce the new buffer,
//!   which then maps onto the analysis host's `SetOverlay { text }`. The salsa
//!   layer already avoids re-analysing unaffected files.
//! - **Positions**: UTF-16 (the protocol default; no `positionEncoding`
//!   negotiation), converted at the boundary by
//!   [`LineIndex`](crate::line_index::LineIndex).
//! - **Diagnostics**: pushed via `textDocument/publishDiagnostics` after
//!   every open/change/close, computed from a fresh [`Analysis`] snapshot.
//!   Problems with the *project configuration* rather than a document —
//!   currently a `[lint]` key naming no known rule id — have no URI to hang a
//!   diagnostic on and go to the client's log pane via `window/logMessage`
//!   (see [`Server::log_lint_config_problems`]).
//! - **Malformed input is not fatal**: a message whose params do not fit the
//!   method's schema — a hover with no `position`, a `didOpen` carrying a
//!   `file://` URI with an unencoded space (clients do send these; the URI
//!   grammar rejects them) — is a *client* bug, and taking the editor's
//!   language server down over one is the worst possible response. A
//!   **request** is answered with `-32602 InvalidParams` naming the method and
//!   the deserialization failure (see [`cast_request`]); a **notification**,
//!   which has nobody to answer, is logged and dropped (see
//!   [`Server::notification_params`]). Only genuine transport failures — a
//!   closed stdin, a dead [`Connection`] — end the loop.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use lsp_server::{
    Connection, ErrorCode, ExtractError, Message, Notification, Request, RequestId, Response,
};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, Exit, LogMessage, Notification as _, Progress, PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest,
    FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation, GotoTypeDefinition,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, RangeFormatting, References,
    RegisterCapability, Rename, Request as _, SelectionRangeRequest, SemanticTokensFullRequest,
    SignatureHelpRequest, WorkDoneProgressCreate, WorkspaceSymbolRequest,
};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyServerCapability, CodeAction, CodeActionKind, CodeActionOrCommand,
    CodeActionProviderCapability, CompletionOptions, CompletionResponse,
    DidChangeWatchedFilesParams, DidChangeWatchedFilesRegistrationOptions, DocumentHighlight,
    DocumentSymbolResponse, FileChangeType, FileSystemWatcher, FoldingRange,
    FoldingRangeProviderCapability, GlobPattern, GotoDefinitionResponse, Hover,
    HoverProviderCapability, ImplementationProviderCapability, InitializeParams, InitializeResult,
    InlayHint, Location, LogMessageParams, MessageType, OneOf, PrepareRenameResponse,
    ProgressParams, ProgressParamsValue, ProgressToken, PublishDiagnosticsParams, Registration,
    RegistrationParams, RenameOptions, SelectionRange, SelectionRangeProviderCapability,
    SemanticTokens, SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensResult,
    SemanticTokensServerCapabilities, ServerCapabilities, ServerInfo, SignatureHelp,
    SignatureHelpOptions, SymbolInformation, TextDocumentContentChangeEvent,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, TypeDefinitionProviderCapability,
    Uri, WorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressCreateParams,
    WorkDoneProgressEnd, WorkDoneProgressReport, WorkspaceEdit, WorkspaceSymbolResponse,
};
use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};
use luabox_lint::{LintConfig, UnknownRuleId, lint_source};
use luabox_manifest::layout::{self, DefFiles};
use luabox_manifest::model::{DialectId, Manifest};
use luabox_types::{Ambient, build_ambient};

use crate::line_index::LineIndex;
use crate::sema::FileSema;
use crate::uri::uri_to_path;
use crate::{
    call_hierarchy, code_action, completion, diagnostics, document_highlight, fmt, folding,
    goto_definition, goto_implementation, goto_type_definition, hover, inlay_hints, references,
    rename, selection_range, semantic_tokens, signature_help, symbols,
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
    // Params the handshake cannot decode (a `rootUri` with an unencoded space
    // is the realistic one) end the session — there is no workspace to serve —
    // but the client is told so on the id it is blocked on rather than left
    // waiting for a pipe that just closed.
    let params: InitializeParams = match serde_json::from_value(params) {
        Ok(params) => params,
        Err(err) => {
            let message = format!("malformed `initialize` request: {err}");
            let _ = connection.sender.send(Message::Response(Response::new_err(
                id,
                ErrorCode::InvalidParams as i32,
                message.clone(),
            )));
            anyhow::bail!(message);
        }
    };
    let result = InitializeResult {
        capabilities: server_capabilities(),
        server_info: Some(ServerInfo {
            name: "luabox-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    // Client capabilities that gate optional protocol features: work-done
    // progress for the bootstrap index, and dynamic file-watcher registration.
    let work_done_progress = params
        .capabilities
        .window
        .as_ref()
        .and_then(|w| w.work_done_progress)
        .unwrap_or(false);
    let watch_files = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false);

    let root = root_path(&params)
        .or_else(|| std::env::current_dir().ok())
        .context("cannot determine a workspace root")?;
    let mut server = Server::new(connection, root);
    if watch_files {
        server.register_file_watchers();
    }
    server.bootstrap(work_done_progress);
    server.main_loop()
}

/// The capabilities advertised at initialize: incremental sync, hover, definition,
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
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: Some(vec![",".to_string()]),
            ..SignatureHelpOptions::default()
        }),
        // Goto type-definition (value → its `---@class`/`---@alias`/`---@enum`,
        // see [`crate::goto_type_definition`]) and goto-implementation
        // (interface class → its subclasses, see
        // [`crate::goto_implementation`]).
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
    /// `[lint]` keys naming no known rule id, handed back by the same
    /// translation (CC-M8). Reported to the client as `window/logMessage`
    /// warnings — see [`Server::log_lint_config_problems`].
    unknown_lint_rules: Vec<UnknownRuleId>,
}

impl ProjectConfig {
    fn discover(root: &Path) -> Self {
        let defaults = Self {
            dialect: Dialect::Lua54,
            strictness: Strictness::Warn,
            out_dir: None,
            def_sources: Vec::new(),
            lint: LintConfig::new(),
            unknown_lint_rules: Vec::new(),
        };
        let Ok(text) = fs::read_to_string(root.join("luabox.toml")) else {
            return defaults;
        };
        let Ok(manifest) = Manifest::parse(&text) else {
            eprintln!("luabox-lsp: invalid luabox.toml; using defaults (5.4, warn)");
            return defaults;
        };
        // One `[lint]` translation for the whole workspace (`luabox-lint`'s),
        // shared with `luabox lint` — including the unknown-rule-id check the
        // manifest parser cannot do (CC-M8).
        let (lint, unknown_lint_rules) = LintConfig::from_manifest(&manifest.lint);
        Self {
            // `Manifest::parse` types `[package] edition` as a closed
            // `DialectId`, so this maps inward exhaustively — there is no
            // unknown-edition fallback left to take.
            dialect: syntax_dialect(manifest.package.edition),
            strictness: Strictness::from_manifest_flag(manifest.types.strict),
            out_dir: Some(root.join(&manifest.build.out)),
            def_sources: ambient_def_sources(root, &manifest),
            lint,
            unknown_lint_rules,
        }
    }
}

/// The `luabox-syntax` dialect a validated manifest edition names.
///
/// Distribution never parses syntax (SPEC.md §16), so `luabox-manifest` cannot
/// hand out a `Dialect` itself and each frontend owns this exhaustive match —
/// duplicated with `luabox-cli::dialect::from_manifest` the same way
/// `build_lint_config` is duplicated with `lint_cmd::build_config`.
fn syntax_dialect(id: DialectId) -> Dialect {
    match id {
        DialectId::Lua51 => Dialect::Lua51,
        DialectId::Lua52 => Dialect::Lua52,
        DialectId::Lua53 => Dialect::Lua53,
        DialectId::Lua54 => Dialect::Lua54,
        DialectId::LuaJit => Dialect::LuaJit,
    }
}

/// Resolve the ambient definition-package sources for a project, winner-first
/// (SPEC.md §3, #108): the project's own `[types] defs` from `<root>/defs/`,
/// then every direct dependency's own `[types] defs` from that dependency's
/// `defs/` (the luals `workspace.library` model).
///
/// The resolution is `luabox_manifest::layout`'s — the same walk `luabox
/// check` and `luabox lint` run — so the editor and CI cannot disagree about
/// which definitions are ambient. Only the texts are kept: `build_ambient`
/// takes sources, and the labels exist for diagnostics the CLI renders.
/// Cross-package class collisions (`LB0307`) are a project-wide, check-time
/// concern and are not surfaced per file here; an unresolvable `[types] defs`
/// entry is `luabox check`'s `LB1002` to report, not the editor's.
fn ambient_def_sources(root: &Path, manifest: &Manifest) -> Vec<String> {
    let (project_defs, _unresolved) = layout::resolve_project_defs(root, &manifest.types.defs);
    project_defs
        .into_iter()
        .chain(layout::resolve_dep_defs(root, manifest))
        .map(|def| def.text)
        .collect()
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
    /// The currently open documents (by path → URI), tracked from
    /// didOpen/didClose so a `workspace/didChangeConfiguration` can republish
    /// diagnostics for every open buffer after a settings change.
    open_docs: HashMap<PathBuf, Uri>,
}

impl Server {
    fn new(connection: Connection, root: PathBuf) -> Self {
        let config = ProjectConfig::discover(&root);
        let ambient = build_ambient(config.dialect, &config.def_sources);
        let known_globals = ambient.global_names().clone();
        let mut host = AnalysisHost::new(config.dialect, config.strictness);
        // Anchor the db's `require` resolution at the workspace root so module
        // strings resolve exactly as `luabox check` resolves them on disk (the
        // bundler's SPEC.md §7 path-mapping) — editor and CI in lockstep.
        host.set_root(root.clone());
        let server = Self {
            connection,
            host,
            root,
            dialect: config.dialect,
            strictness: config.strictness,
            out_dir: config.out_dir,
            ambient,
            lint: config.lint,
            known_globals,
            open_docs: HashMap::new(),
        };
        // Safe to send: `run` only builds the server after `initialize_finish`.
        server.log_lint_config_problems(&config.unknown_lint_rules);
        server
    }

    /// Tell the client about `[lint]` keys that name no known rule id, as
    /// `window/logMessage` warnings — one per key, wording (and the "did you
    /// mean" nudge) shared with `luabox lint`'s `LB1004`.
    ///
    /// Not `publishDiagnostics`: that is per-document and keyed by URI, and
    /// this is a manifest problem, not a `.lua` one — the editor would have to
    /// be told to open `luabox.toml` as a diagnostic target it otherwise never
    /// analyses. It is not `window/showMessage` either: a popup per unknown
    /// key on every config reload is noise. `logMessage` is where the other
    /// manifest complaint in this file goes (an unparseable `luabox.toml`,
    /// which still only reaches stderr) and is what the log pane is for.
    fn log_lint_config_problems(&self, unknown: &[UnknownRuleId]) {
        for entry in unknown {
            let mut message = format!("luabox.toml: {}", entry.message());
            for note in entry.notes() {
                let _ = write!(message, " — {note}");
            }
            self.log_message(MessageType::WARNING, message);
        }
    }

    /// Send one `window/logMessage` notification.
    fn log_message(&self, typ: MessageType, message: String) {
        let params = LogMessageParams { typ, message };
        let Ok(value) = serde_json::to_value(params) else {
            return;
        };
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                LogMessage::METHOD.to_owned(),
                value,
            )));
    }

    /// Re-read `luabox.toml` and rebuild every cached setting derived from it —
    /// `strictness`, `lint`, the ambient definition layer, and the
    /// `known_globals` baseline — so a `workspace/didChangeConfiguration` (or a
    /// watched-file edit to the manifest) takes effect without a restart. The
    /// host's project strictness is updated in lock-step, and a dialect change
    /// re-parses every known file under the new dialect. Diagnostics for open
    /// documents are then republished so the change is reflected immediately.
    fn reload_config(&mut self) -> anyhow::Result<()> {
        let config = ProjectConfig::discover(&self.root);
        let ambient = build_ambient(config.dialect, &config.def_sources);
        self.known_globals = ambient.global_names().clone();
        self.ambient = ambient;
        self.lint = config.lint;
        // Re-report: the reload may have introduced (or fixed) a typo'd key.
        self.log_lint_config_problems(&config.unknown_lint_rules);
        self.strictness = config.strictness;
        self.out_dir = config.out_dir;
        self.host
            .apply_change(Change::SetStrictness(config.strictness));
        if config.dialect != self.dialect {
            self.dialect = config.dialect;
            let paths: Vec<PathBuf> = self
                .host
                .snapshot()
                .files()
                .map(Path::to_path_buf)
                .collect();
            for path in paths {
                self.host.apply_change(Change::SetDialect {
                    path,
                    dialect: config.dialect,
                });
            }
        }
        self.republish_open_docs()
    }

    /// Republish diagnostics for every currently open document from a fresh
    /// snapshot — used after a configuration reload changes the cached
    /// strictness/lint/ambient so the visible diagnostics reflect it.
    fn republish_open_docs(&mut self) -> anyhow::Result<()> {
        for (path, uri) in self.open_docs.clone() {
            self.publish_lua(&uri, &path)?;
        }
        Ok(())
    }

    /// Ask the client (dynamic registration) to watch the project's `.lua`
    /// files and `luabox.toml`, so external edits arrive as
    /// `workspace/didChangeWatchedFiles`. Only sent when the client advertised
    /// `workspace.didChangeWatchedFiles.dynamicRegistration`.
    fn register_file_watchers(&self) {
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: ["**/*.lua", "**/luabox.toml"]
                .into_iter()
                .map(|glob| FileSystemWatcher {
                    glob_pattern: GlobPattern::String(glob.to_string()),
                    kind: None,
                })
                .collect(),
        };
        let registration = Registration {
            id: "luabox-watch-files".to_string(),
            method: DidChangeWatchedFiles::METHOD.to_string(),
            register_options: serde_json::to_value(options).ok(),
        };
        let params = RegistrationParams {
            registrations: vec![registration],
        };
        if let Ok(value) = serde_json::to_value(params) {
            let _ = self.connection.sender.send(Message::Request(Request::new(
                RequestId::from("luabox-register-watchers".to_string()),
                RegisterCapability::METHOD.to_string(),
                value,
            )));
        }
    }

    /// Load every `.lua` file under the root into the host (so
    /// `project_diagnostics` and cross-file goto have the full picture).
    ///
    /// When the client supports work-done progress (`progress`), the index is
    /// wrapped in a server-created `$/progress` token — begin, a report per
    /// file loaded, then end — so a large workspace shows a progress indicator
    /// at startup instead of an unexplained pause.
    fn bootstrap(&mut self, progress: bool) {
        let files = self.collect_lua_files();
        let token = progress.then(|| self.begin_progress(files.len()));

        for (i, path) in files.into_iter().enumerate() {
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.host.apply_change(Change::SetFileText {
                path,
                dialect: self.dialect,
                text,
            });
            if let Some(token) = &token {
                self.report_progress(token, i + 1, &name);
            }
        }

        if let Some(token) = &token {
            self.send_progress(token, WorkDoneProgress::End(WorkDoneProgressEnd::default()));
        }
    }

    /// Every first-party `.lua` file under the workspace root, in the walk's
    /// deterministic order — `luabox_manifest::layout`'s walk, the one
    /// `luabox check` uses, so the editor indexes exactly the files CI checks.
    ///
    /// This used to be a private copy of the walk, and had drifted twice: it
    /// descended into vendored `lua_modules/` rock trees (indexing whatever
    /// luarocks materialized as if it were your code) and it visited entries
    /// in `read_dir` order rather than sorted.
    ///
    /// An unreadable root leaves the index empty rather than killing the
    /// server: open buffers still analyse from their overlays, which is a far
    /// better editor experience than refusing to start.
    fn collect_lua_files(&self) -> Vec<PathBuf> {
        layout::collect_lua_files(&self.root, self.out_dir.as_deref(), DefFiles::Include)
            .unwrap_or_else(|err| {
                eprintln!("luabox-lsp: cannot index workspace: {err}");
                Vec::new()
            })
    }

    /// Create the bootstrap progress token on the client and send `begin`.
    fn begin_progress(&self, total: usize) -> ProgressToken {
        let token = ProgressToken::String("luabox/bootstrap".to_string());
        let create = WorkDoneProgressCreateParams {
            token: token.clone(),
        };
        if let Ok(value) = serde_json::to_value(create) {
            let _ = self.connection.sender.send(Message::Request(Request::new(
                RequestId::from("luabox-bootstrap-progress".to_string()),
                WorkDoneProgressCreate::METHOD.to_string(),
                value,
            )));
        }
        self.send_progress(
            &token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: "Indexing workspace".to_string(),
                message: Some(format!("0/{total} files")),
                percentage: Some(0),
                ..WorkDoneProgressBegin::default()
            }),
        );
        token
    }

    /// Send a `report` for the bootstrap index after loading file `done` of the
    /// batch (1-based), named `name`.
    fn report_progress(&self, token: &ProgressToken, done: usize, name: &str) {
        self.send_progress(
            token,
            WorkDoneProgress::Report(WorkDoneProgressReport {
                message: Some(format!("{done} files ({name})")),
                ..WorkDoneProgressReport::default()
            }),
        );
    }

    /// Send one `$/progress` notification under `token`.
    fn send_progress(&self, token: &ProgressToken, value: WorkDoneProgress) {
        let params = ProgressParams {
            token: token.clone(),
            value: ProgressParamsValue::WorkDone(value),
        };
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                Progress::METHOD.to_string(),
                params,
            )));
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

    /// Answer one request. Every path produces a response — a result, a
    /// `MethodNotFound` for a method outside the advertised set, or the
    /// `InvalidParams` [`cast_request`] hands back for params that do not
    /// decode — so the client is never left holding an unanswered id. Only
    /// the send itself can fail, and a dead transport is fatal by design.
    fn handle_request(&mut self, req: Request) -> anyhow::Result<()> {
        let response = self
            .dispatch(req)
            .unwrap_or_else(|invalid_params| invalid_params);
        self.connection.sender.send(Message::Response(response))?;
        Ok(())
    }

    /// The per-method dispatch table. `Err` carries a ready-to-send
    /// `InvalidParams` response rather than a fatal error: [`handle_request`]
    /// sends either arm, so a malformed request costs the client one error
    /// reply and costs the session nothing.
    ///
    /// [`handle_request`]: Self::handle_request
    #[allow(
        clippy::too_many_lines,
        reason = "a flat per-method dispatch table — one arm per LSP request, each a few lines"
    )]
    fn dispatch(&mut self, req: Request) -> Result<Response, Response> {
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
        Ok(response)
    }

    /// The analysis snapshot and semantic view for the document `uri` names,
    /// or `None` when the URI is not a file the analysis knows.
    ///
    /// Every handler starts here. The snapshot is *returned alongside* the
    /// view rather than taken and dropped inside: a handler that also queries
    /// the workspace (references, rename, the goto family) must see the same
    /// consistent snapshot the view was built from, and salsa memoization only
    /// pays off while it is alive.
    fn file(&self, uri: &Uri) -> Option<(Analysis, FileSema)> {
        let path = uri_to_path(uri)?;
        let snapshot = self.host.snapshot();
        let sema = FileSema::new(&snapshot, &path)?;
        Some((snapshot, sema))
    }

    /// [`Self::file`] plus the byte offset `position` names in that document —
    /// the prelude of every position-addressed request.
    fn at(&self, uri: &Uri, position: lsp_types::Position) -> Option<(Analysis, FileSema, usize)> {
        let (snapshot, sema) = self.file(uri)?;
        let offset = sema.index.offset(position);
        Some((snapshot, sema, offset))
    }

    fn hover(&self, uri: &Uri, position: lsp_types::Position) -> Option<Hover> {
        let (_snapshot, sema, offset) = self.at(uri, position)?;
        hover::hover(&sema, offset)
    }

    /// The callee's resolved signature(s) while `position` sits inside a
    /// call's argument list (see [`crate::signature_help`]).
    fn signature_help(&self, uri: &Uri, position: lsp_types::Position) -> Option<SignatureHelp> {
        let (_snapshot, sema, offset) = self.at(uri, position)?;
        signature_help::signature_help(&sema, offset)
    }

    /// The call-hierarchy item for the function the cursor names at `position`
    /// — a declaration or a call site (see [`crate::call_hierarchy`]).
    fn prepare_call_hierarchy(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<CallHierarchyItem>> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        call_hierarchy::prepare_call_hierarchy(&snapshot, &sema, offset)
    }

    /// The call sites across the workspace that call `item`, grouped by their
    /// enclosing function (see [`crate::call_hierarchy`]).
    fn incoming_calls(&self, item: &CallHierarchyItem) -> Option<Vec<CallHierarchyIncomingCall>> {
        let (snapshot, sema) = self.file(&item.uri)?;
        Some(call_hierarchy::incoming_calls(&snapshot, &sema, item))
    }

    /// The functions called within `item`'s body (see
    /// [`crate::call_hierarchy`]).
    fn outgoing_calls(&self, item: &CallHierarchyItem) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let (snapshot, sema) = self.file(&item.uri)?;
        Some(call_hierarchy::outgoing_calls(&snapshot, &sema, item))
    }

    fn definition(&self, uri: &Uri, position: lsp_types::Position) -> Option<Location> {
        let (_snapshot, sema, offset) = self.at(uri, position)?;
        goto_definition::definition(&sema, offset, &self.root, self.dialect)
    }

    /// The declaration of the type carried by the value at `position`: its
    /// `---@class`/`---@alias`/`---@enum`, searched workspace-wide (declarations
    /// are workspace-global).
    fn type_definition(&self, uri: &Uri, position: lsp_types::Position) -> Option<Location> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        goto_type_definition::type_definition(&snapshot, &sema, offset)
    }

    /// Every implementor of the `---@class` at `position`: each workspace class
    /// that lists it as a parent (see [`crate::goto_implementation`]).
    fn implementation(&self, uri: &Uri, position: lsp_types::Position) -> Option<Vec<Location>> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        goto_implementation::implementation(&snapshot, &sema, offset)
    }

    /// All references to the symbol at `position`. Locals/upvalues are found in
    /// the file itself; globals and class members are searched across every
    /// file the snapshot knows about.
    fn references(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
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
        let (snapshot, sema, offset) = self.at(uri, position)?;
        rename::rename(&snapshot, &sema, offset, new_name)
    }

    /// The identifier range under `position` for the editor to pre-select, or
    /// `None` when the position is not a renameable symbol.
    fn prepare_rename(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<PrepareRenameResponse> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        rename::prepare_rename(&snapshot, &sema, offset).map(PrepareRenameResponse::Range)
    }

    /// Completions at `position`: scope/member items plus auto-require imports
    /// (see [`crate::completion`]) — the auto-require enumeration reads every
    /// workspace file's memoized module export off the same snapshot.
    fn completion(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<Vec<lsp_types::CompletionItem>> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        Some(completion::completion(&sema, offset, &snapshot, &self.root))
    }

    fn document_symbols(&self, uri: &Uri) -> Option<Vec<lsp_types::DocumentSymbol>> {
        let (_snapshot, sema) = self.file(uri)?;
        Some(symbols::document_symbols(&sema))
    }

    /// Fuzzy (case-insensitive substring) search for `query` across every
    /// `.lua` file the analysis snapshot knows about — one snapshot for the
    /// whole scan, like [`Self::references`]: classes, functions,
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
        let (snapshot, sema, offset) = self.at(uri, position)?;
        document_highlight::document_highlight(&snapshot, &sema, offset)
    }

    /// Folding regions for one file: blocks, table constructors, and comment
    /// runs (see [`crate::folding`]) — pure syntax-tree geometry, no
    /// semantic analysis needed.
    fn folding_ranges(&self, uri: &Uri) -> Option<Vec<FoldingRange>> {
        let (_snapshot, sema) = self.file(uri)?;
        Some(folding::folding_ranges(&sema))
    }

    /// The syntax-tree expand chain for each requested position (see
    /// [`crate::selection_range`]).
    fn selection_ranges(
        &self,
        uri: &Uri,
        positions: &[lsp_types::Position],
    ) -> Option<Vec<SelectionRange>> {
        let (_snapshot, sema) = self.file(uri)?;
        Some(selection_range::selection_ranges(&sema, positions))
    }

    /// Full-document formatting; also serves range requests (MVP semantics,
    /// see [`crate::fmt`]). `None` for unknown documents; `Some(vec![])`
    /// when nothing changed — including the formatters' parse-error
    /// "return input unchanged" guarantee, which must not become an error.
    fn formatting(&self, uri: &Uri) -> Option<Vec<TextEdit>> {
        // The one handler that needs no semantic view: the formatter takes the
        // text and re-parses it itself.
        let path = uri_to_path(uri)?;
        let text = self.host.snapshot().file_text(&path)?;
        let formatted = luabox_syntax::lua::fmt::format(&text, self.dialect);
        Some(fmt::formatting(&text, &formatted))
    }

    fn semantic_tokens(&self, uri: &Uri) -> Option<SemanticTokensResult> {
        let (_snapshot, sema) = self.file(uri)?;
        Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: semantic_tokens::semantic_tokens(&sema),
        }))
    }

    /// Inlay hints for the visible `range` of a `.lua` document: the
    /// display-mode inference's binding types and inferred function
    /// returns (see [`crate::inlay_hints`]).
    fn inlay_hints(&self, uri: &Uri, range: lsp_types::Range) -> Option<Vec<InlayHint>> {
        let (snapshot, sema) = self.file(uri)?;
        let inferred = snapshot.binding_types(&sema.path)?;
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
        let (snapshot, sema) = self.file(uri)?;
        let index = &sema.index;
        let rel = sema.path.to_string_lossy();
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
                    .map(|d| vec![diagnostics::convert(index, d, diagnostics::LINT_SOURCE)]),
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
        let inferred = snapshot.binding_types(&sema.path);
        let ctx = diagnostics::CheckCtx {
            strictness: self.strictness,
            ambient: &self.ambient,
            lint: &self.lint,
            known_globals: &self.known_globals,
        };
        let type_diags =
            diagnostics::diagnostics(&snapshot, &sema.path, self.dialect, &ctx).unwrap_or_default();
        actions.extend(code_action::code_actions(
            &sema,
            inferred.as_ref(),
            &type_diags,
            uri,
            start,
            end,
        ));
        Some(actions)
    }

    // === Notifications ====================================================

    /// Decode a notification's params, or log the problem and drop it.
    ///
    /// A notification carries no id, so there is nobody to answer and the
    /// protocol forbids replying to one: the client's log pane is the only
    /// honest channel, and it is where this file's other "your project is
    /// misconfigured" complaints already go (see
    /// [`Self::log_lint_config_problems`]). What must *not* happen is the old
    /// behaviour — propagating the error out of the message loop, so a
    /// `didOpen` whose URI held an unencoded space (or whose `languageId` the
    /// client forgot) ended the editor session.
    fn notification_params<N: lsp_types::notification::Notification>(
        &self,
        params: serde_json::Value,
    ) -> Option<N::Params> {
        match serde_json::from_value(params) {
            Ok(params) => Some(params),
            Err(err) => {
                self.log_message(
                    MessageType::ERROR,
                    format!("ignoring malformed `{}` notification: {err}", N::METHOD),
                );
                None
            }
        }
    }

    fn handle_notification(&mut self, not: Notification) -> anyhow::Result<()> {
        match not.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Some(params) = self.notification_params::<DidOpenTextDocument>(not.params)
                else {
                    return Ok(());
                };
                let uri = params.text_document.uri;
                if let Some(path) = uri_to_path(&uri) {
                    self.open_docs.insert(path, uri.clone());
                }
                self.set_text(&uri, params.text_document.text)?;
            }
            DidChangeTextDocument::METHOD => {
                let Some(params) = self.notification_params::<DidChangeTextDocument>(not.params)
                else {
                    return Ok(());
                };
                let uri = params.text_document.uri;
                // Incremental sync: apply each ranged edit in order against the
                // current overlay/disk text to rebuild the new buffer.
                let Some(path) = uri_to_path(&uri) else {
                    return Ok(());
                };
                let current = self.host.snapshot().file_text(&path).unwrap_or_default();
                let text = apply_content_changes(current, params.content_changes);
                self.set_text(&uri, text)?;
            }
            DidCloseTextDocument::METHOD => {
                let Some(params) = self.notification_params::<DidCloseTextDocument>(not.params)
                else {
                    return Ok(());
                };
                if let Some(path) = uri_to_path(&params.text_document.uri) {
                    self.open_docs.remove(&path);
                }
                self.close(&params.text_document.uri)?;
            }
            DidChangeConfiguration::METHOD => {
                // A settings change may alter dialect/strictness/lint/defs;
                // re-read the manifest and republish every open document.
                self.reload_config()?;
            }
            DidChangeWatchedFiles::METHOD => {
                let Some(params) = self.notification_params::<DidChangeWatchedFiles>(not.params)
                else {
                    return Ok(());
                };
                self.watched_files_changed(&params)?;
            }
            // `exit` only reaches this table when it was *not* preceded by
            // `shutdown`: the ordered pair is consumed inside
            // `Connection::handle_shutdown`, which waits for the notification
            // itself. The spec is explicit about the unordered case — the
            // server exits with code 1 — and this error is how the CLI
            // frontend reaches that code (`main` maps any `Err` to
            // `ExitCode::FAILURE`).
            Exit::METHOD => {
                anyhow::bail!("received `exit` without a prior `shutdown`");
            }
            // `textDocument/didSave` is a deliberate no-op — the overlay is
            // already the saved content. Everything else is ignored.
            _ => {}
        }
        Ok(())
    }

    /// Handle `workspace/didChangeWatchedFiles`: re-read each changed `.lua`
    /// file from disk into the host (so an external edit invalidates the
    /// analysis) and, if `luabox.toml` changed, reload the project config. A
    /// deleted `.lua` file cannot be read, so its overlay/disk text is left as
    /// is — a subsequent open will refresh it. Diagnostics for any changed file
    /// that is currently open are republished; a manifest change republishes
    /// every open document via [`Self::reload_config`].
    fn watched_files_changed(
        &mut self,
        params: &DidChangeWatchedFilesParams,
    ) -> anyhow::Result<()> {
        let mut reload = false;
        for event in &params.changes {
            let Some(path) = uri_to_path(&event.uri) else {
                continue;
            };
            if path.file_name().and_then(|n| n.to_str()) == Some("luabox.toml") {
                reload = true;
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            if event.typ == FileChangeType::DELETED {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path) {
                self.host.apply_change(Change::SetFileText {
                    path: path.clone(),
                    dialect: self.dialect,
                    text,
                });
                // Republish only if the file is not shadowed by an editor
                // overlay — an open buffer's diagnostics come from the overlay,
                // not disk, so re-reading disk changes nothing visible there.
                if !self.open_docs.contains_key(&path) {
                    self.publish_lua(&event.uri, &path)?;
                }
            }
        }
        if reload {
            self.reload_config()?;
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
            diagnostics::diagnostics(&analysis, path, self.dialect, &ctx).unwrap_or_default();
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

/// Apply a batch of `didChange` content changes to `text` in order, returning
/// the resulting document text.
///
/// Each change is relative to the state produced by the previous one, so the
/// working text is rebuilt (and re-indexed) between changes:
///
/// - a change with a `range` splices its `text` over that range — the range's
///   UTF-16 `(line, character)` endpoints are mapped to byte offsets through a
///   fresh [`LineIndex`] over the *current* working text, so multi-byte
///   characters and offset shifts from earlier changes resolve correctly;
/// - a change with no `range` (full-replace) replaces the whole document.
///
/// This is the incremental-sync core, factored out so it is unit-testable
/// independently of the server loop.
fn apply_content_changes(text: String, changes: Vec<TextDocumentContentChangeEvent>) -> String {
    let mut text = text;
    for change in changes {
        match change.range {
            #[expect(
                clippy::string_slice,
                reason = "start and end are LineIndex byte offsets (char boundaries, <= len); end is clamped >= start, so both splice slices are valid"
            )]
            Some(range) => {
                let index = LineIndex::new(text);
                let start = index.offset(range.start);
                // A malformed range with `end` before `start` would panic the
                // splice; clamp defensively (LSP guarantees start <= end).
                let end = index.offset(range.end).max(start);
                let current = index.text();
                let mut next =
                    String::with_capacity(current.len() - (end - start) + change.text.len());
                next.push_str(&current[..start]);
                next.push_str(&change.text);
                next.push_str(&current[end..]);
                text = next;
            }
            None => text = change.text,
        }
    }
    text
}

/// Extract a request's id and params, or the JSON-RPC error response to send
/// in their place.
///
/// The failure is a *client* bug — a hover with no `position`, a line number
/// sent as a string, a `file://` URI the grammar rejects because a space in it
/// was never percent-encoded — and the protocol has an answer for exactly
/// that: `-32602 InvalidParams`, naming the method and what would not decode.
/// This used to return an `anyhow::Error` that propagated out of the message
/// loop, so one bad message killed the process and left the request the client
/// was blocked on unanswered forever.
fn cast_request<R: lsp_types::request::Request>(
    req: Request,
) -> Result<(RequestId, R::Params), Response> {
    // `extract` consumes the request, so keep the id for the error response.
    let id = req.id.clone();
    req.extract(R::METHOD).map_err(|err| {
        let detail = match err {
            ExtractError::JsonError { error, .. } => error.to_string(),
            // Unreachable: the caller dispatches on the method first.
            ExtractError::MethodMismatch(req) => format!("method mismatch (`{}`)", req.method),
        };
        Response::new_err(
            id,
            ErrorCode::InvalidParams as i32,
            format!("malformed `{}` request: {detail}", R::METHOD),
        )
    })
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::notification::{DidOpenTextDocument, Notification as _};
    use lsp_types::request::{HoverRequest, Request as _};
    use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
    use luabox_lint::LintConfig;
    use luabox_manifest::model::{Lint, LintLevel, LintTier, Manifest};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::{
        ErrorCode, ProjectConfig, Server, ambient_def_sources, apply_content_changes, root_path,
    };

    /// A ranged change replacing `[start, end)` with `text`.
    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position::new(start.0, start.1),
                end: Position::new(end.0, end.1),
            }),
            range_length: None,
            text: text.to_string(),
        }
    }

    /// A full-replace change (no range).
    fn full(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn ranged_insert_splices_at_offset() {
        // Insert `X` between `ab` and `cd` (a zero-width range at col 2).
        let out = apply_content_changes("abcd".to_string(), vec![edit((0, 2), (0, 2), "X")]);
        assert_eq!(out, "abXcd");
    }

    #[test]
    fn ranged_deletion_removes_span() {
        // Delete `bc` from `abcd` (cols 1..3, empty replacement).
        let out = apply_content_changes("abcd".to_string(), vec![edit((0, 1), (0, 3), "")]);
        assert_eq!(out, "ad");
    }

    #[test]
    fn ranged_replacement_across_lines() {
        // Replace from line 0 col 1 through line 1 col 1 with `Z`.
        let out = apply_content_changes("abc\ndef".to_string(), vec![edit((0, 1), (1, 1), "Z")]);
        assert_eq!(out, "aZef");
    }

    #[test]
    fn insert_at_end_of_document() {
        let out = apply_content_changes("abc".to_string(), vec![edit((0, 3), (0, 3), "de")]);
        assert_eq!(out, "abcde");
    }

    #[test]
    fn insert_into_empty_document() {
        let out = apply_content_changes(String::new(), vec![edit((0, 0), (0, 0), "hi")]);
        assert_eq!(out, "hi");
    }

    #[test]
    fn multi_change_batch_applies_in_order() {
        // First insert `123 ` at the start, then, against the *new* text,
        // append `!` at what is now column 8 (`123 wxyz`). The second change's
        // offsets must be relative to the result of the first.
        let out = apply_content_changes(
            "wxyz".to_string(),
            vec![edit((0, 0), (0, 0), "123 "), edit((0, 8), (0, 8), "!")],
        );
        assert_eq!(out, "123 wxyz!");
    }

    #[test]
    fn full_replace_ignores_prior_text() {
        let out = apply_content_changes("old content".to_string(), vec![full("brand new")]);
        assert_eq!(out, "brand new");
    }

    #[test]
    fn full_replace_then_ranged_edit() {
        // A full replace resets the buffer, then a ranged edit applies to it.
        let out = apply_content_changes(
            "old".to_string(),
            vec![full("hello"), edit((0, 5), (0, 5), " world")],
        );
        assert_eq!(out, "hello world");
    }

    #[test]
    fn ranged_edit_maps_utf16_columns() {
        // `😀` is 4 bytes but 2 UTF-16 units; inserting after it must land at
        // the right byte offset, not treat the column as a byte index.
        let out = apply_content_changes("😀ab".to_string(), vec![edit((0, 2), (0, 2), "-")]);
        assert_eq!(out, "😀-ab");
    }

    // === Workspace root ===================================================

    /// Absolute test paths differ per platform; build one under a fake root.
    fn abs(rel: &str) -> PathBuf {
        Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" }).join(rel)
    }

    #[allow(deprecated, reason = "InitializeParams carries deprecated fields")]
    fn init_params() -> lsp_types::InitializeParams {
        lsp_types::InitializeParams::default()
    }

    #[test]
    fn workspace_folders_win_over_the_deprecated_root_uri() {
        let mut params = init_params();
        params.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: crate::path_to_uri(&abs("folder")),
            name: "folder".to_string(),
        }]);
        #[allow(deprecated, reason = "rootUri is the older-client fallback")]
        {
            params.root_uri = Some(crate::path_to_uri(&abs("legacy")));
        }
        assert_eq!(root_path(&params), Some(abs("folder")));
    }

    #[test]
    fn root_uri_is_the_fallback_when_there_are_no_workspace_folders() {
        let mut params = init_params();
        #[allow(deprecated, reason = "rootUri is the older-client fallback")]
        {
            params.root_uri = Some(crate::path_to_uri(&abs("legacy")));
        }
        assert_eq!(root_path(&params), Some(abs("legacy")));
    }

    #[test]
    fn a_non_file_workspace_folder_falls_through_to_the_root_uri() {
        use std::str::FromStr as _;

        let mut params = init_params();
        params.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: lsp_types::Uri::from_str("untitled:scratch").expect("uri"),
            name: "scratch".to_string(),
        }]);
        #[allow(deprecated, reason = "rootUri is the older-client fallback")]
        {
            params.root_uri = Some(crate::path_to_uri(&abs("legacy")));
        }
        assert_eq!(root_path(&params), Some(abs("legacy")));
    }

    #[test]
    fn no_folders_and_no_root_uri_means_no_root() {
        assert_eq!(root_path(&init_params()), None);
    }

    // === Project configuration ============================================

    #[test]
    fn a_missing_manifest_yields_the_defaults() {
        let dir = TempDir::new().expect("tempdir");
        let config = ProjectConfig::discover(dir.path());
        assert_eq!(config.dialect, luabox_db::Dialect::Lua54);
        assert_eq!(config.strictness, luabox_db::Strictness::Warn);
        assert_eq!(config.out_dir, None);
        assert!(config.def_sources.is_empty());
    }

    #[test]
    fn an_unparseable_manifest_falls_back_to_the_defaults() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(dir.path().join("luabox.toml"), "this is not = = toml").expect("write");
        let config = ProjectConfig::discover(dir.path());
        assert_eq!(config.dialect, luabox_db::Dialect::Lua54);
        assert_eq!(config.strictness, luabox_db::Strictness::Warn);
        assert_eq!(config.out_dir, None);
    }

    #[test]
    fn a_manifest_supplies_the_dialect_strictness_and_out_dir() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(
            dir.path().join("luabox.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.1\"\n\n[build]\nout = \"build\"\n\n[types]\nstrict = true\n",
        )
        .expect("write");
        let config = ProjectConfig::discover(dir.path());
        assert_eq!(config.dialect, luabox_db::Dialect::Lua51);
        assert_eq!(config.strictness, luabox_db::Strictness::Strict);
        assert_eq!(config.out_dir, Some(dir.path().join("build")));
    }

    #[test]
    fn an_unknown_edition_falls_back_to_lua_54() {
        let dir = TempDir::new().expect("tempdir");
        fs::write(
            dir.path().join("luabox.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
        )
        .expect("write");
        assert_eq!(
            ProjectConfig::discover(dir.path()).dialect,
            luabox_db::Dialect::Lua54
        );
    }

    // === Lint configuration ===============================================

    #[test]
    fn manifest_globals_tiers_and_rules_reach_the_lint_config() {
        let manifest = Manifest::parse(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[lint]\nglobals = [\"MY_GLOBAL\"]\nstyle = \"deny\"\nunused-local = \"allow\"\n",
        )
        .expect("manifest");
        let (config, unknown) = LintConfig::from_manifest(&manifest.lint);
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(config.is_allowed_global("MY_GLOBAL"));
        assert!(!config.is_allowed_global("other"));
        // A tier key lands in `tiers`, anything else in `rules`.
        assert_eq!(
            manifest.lint.tiers.get(&LintTier::Style),
            Some(&LintLevel::Deny)
        );
        assert_eq!(
            manifest.lint.rules.get("unused-local"),
            Some(&LintLevel::Allow)
        );
        // Both overrides reach the config through the typed setters.
        let mut probe = LintConfig::new();
        probe.set_tier(LintTier::Style.into(), LintLevel::Deny.into());
        probe.set_rule("unused-local", LintLevel::Allow.into());
    }

    #[test]
    fn a_typod_lint_key_reaches_the_server_as_an_unknown_rule_id() {
        // The editor honours `[lint]` through the same translation as the CLI,
        // so it sees the same unknown ids — and surfaces them on
        // `window/logMessage` (CC-M8). `pedantics` covers the tier-typo case:
        // it lands in `rules`, and the nudge points back at the tier.
        let dir = TempDir::new().expect("tempdir");
        fs::write(
            dir.path().join("luabox.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[lint]\nunused-locl = \"allow\"\npedantics = \"warn\"\n",
        )
        .expect("write");

        let config = ProjectConfig::discover(dir.path());
        let reported: Vec<(&str, Option<&str>)> = config
            .unknown_lint_rules
            .iter()
            .map(|u| (u.id(), u.suggestion()))
            .collect();
        assert_eq!(
            reported,
            [
                ("pedantics", Some("pedantic")),
                ("unused-locl", Some("unused-local")),
            ]
        );
    }

    #[test]
    fn a_project_without_a_manifest_reports_no_lint_config_problems() {
        let dir = TempDir::new().expect("tempdir");
        assert!(
            ProjectConfig::discover(dir.path())
                .unknown_lint_rules
                .is_empty()
        );
    }

    #[test]
    fn an_empty_lint_table_leaves_the_config_untouched() {
        let (config, unknown) = LintConfig::from_manifest(&Lint::default());
        assert!(config.unknown_rule_ids().is_empty());
        assert!(unknown.is_empty());
        assert!(!config.is_allowed_global("anything"));
    }

    // === Ambient definition sources =======================================

    /// Write `text` to `root/rel`, creating parents.
    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, text).expect("write");
    }

    fn manifest_of(text: &str) -> Manifest {
        Manifest::parse(text).expect("manifest")
    }

    const MANIFEST_HEAD: &str =
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n";

    #[test]
    fn project_defs_come_before_dependency_defs() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        write(root, "defs/own.d.lua", "---@meta\n-- own\n");
        write(
            root,
            "vendor/dep/luabox.toml",
            &format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"dep\"]\n"),
        );
        write(root, "vendor/dep/defs/dep.d.lua", "---@meta\n-- dep\n");

        let manifest = manifest_of(&format!(
            "{MANIFEST_HEAD}\n[types]\ndefs = [\"own\"]\n\n[dependencies]\ndep = {{ path = \"vendor/dep\" }}\n"
        ));
        let sources = ambient_def_sources(root, &manifest);
        assert_eq!(sources.len(), 2, "{sources:?}");
        assert!(sources[0].contains("-- own"), "{sources:?}");
        assert!(sources[1].contains("-- dep"), "{sources:?}");
    }

    #[test]
    fn a_dependency_without_a_manifest_contributes_nothing() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        // `lua_modules/<name>` is the default location for non-path deps.
        write(root, "lua_modules/dep/defs/dep.d.lua", "---@meta\n-- dep\n");
        let manifest = manifest_of(&format!(
            "{MANIFEST_HEAD}\n[dependencies]\ndep = \"1.0.0\"\n"
        ));
        assert!(ambient_def_sources(root, &manifest).is_empty());
    }

    #[test]
    fn a_dependency_with_an_unparseable_manifest_is_skipped() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        write(root, "lua_modules/dep/luabox.toml", "= not toml =");
        write(root, "lua_modules/dep/defs/dep.d.lua", "---@meta\n-- dep\n");
        let manifest = manifest_of(&format!(
            "{MANIFEST_HEAD}\n[dependencies]\ndep = \"1.0.0\"\n"
        ));
        assert!(ambient_def_sources(root, &manifest).is_empty());
    }

    #[test]
    fn dependency_defs_are_ordered_alphabetically_by_dependency_name() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        for name in ["zulu", "alpha"] {
            write(
                root,
                &format!("lua_modules/{name}/luabox.toml"),
                &format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"{name}\"]\n"),
            );
            write(
                root,
                &format!("lua_modules/{name}/defs/{name}.d.lua"),
                &format!("---@meta\n-- {name}\n"),
            );
        }
        // `zulu` is a dev-dependency: both tables feed the same sorted list.
        let manifest = manifest_of(&format!(
            "{MANIFEST_HEAD}\n[dependencies]\nzulu = \"1.0.0\"\n\n[dev-dependencies]\nalpha = \"1.0.0\"\n"
        ));
        let sources = ambient_def_sources(root, &manifest);
        assert_eq!(sources.len(), 2, "{sources:?}");
        assert!(sources[0].contains("-- alpha"), "{sources:?}");
        assert!(sources[1].contains("-- zulu"), "{sources:?}");
    }

    #[test]
    fn a_defs_directory_contributes_every_nested_d_lua_file_sorted() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        write(root, "defs/pkg/a.d.lua", "---@meta\n-- a\n");
        write(root, "defs/pkg/nested/b.d.lua", "---@meta\n-- b\n");
        // A plain `.lua` file is not a definition file.
        write(root, "defs/pkg/ignored.lua", "-- ignored\n");

        let manifest = manifest_of(&format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"pkg\"]\n"));
        let sources = ambient_def_sources(root, &manifest);
        assert_eq!(sources.len(), 2, "{sources:?}");
        assert!(sources[0].contains("-- a"), "{sources:?}");
        assert!(sources[1].contains("-- b"), "{sources:?}");
    }

    #[test]
    fn a_defs_name_matching_neither_a_file_nor_a_directory_contributes_nothing() {
        // The editor does not report the unresolved entry (that is `luabox
        // check`'s `LB1002`); it simply builds an ambient layer without it.
        let dir = TempDir::new().expect("tempdir");
        let manifest = manifest_of(&format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"absent\"]\n"));
        assert!(ambient_def_sources(dir.path(), &manifest).is_empty());
    }

    // === Malformed messages ===============================================
    //
    // The server is spoken to by editors, and editors send nonsense: params
    // that predate a protocol revision, a URI whose spaces were never
    // percent-encoded, a `null` where an object belongs. None of it may end
    // the session. These drive `Server`'s two entry points directly over an
    // in-memory connection, so the assertions are about the *messages the
    // client would see*, not about a process that happens to still be alive.

    /// A server on an in-memory connection over an empty project, plus the
    /// client end of the wire and the tempdir that must outlive both.
    fn test_server() -> (TempDir, Server, Connection) {
        let dir = TempDir::new().expect("tempdir");
        let (server_end, client) = Connection::memory();
        let server = Server::new(server_end, dir.path().to_path_buf());
        (dir, server, client)
    }

    /// Everything the client end has received so far.
    fn drain(client: &Connection) -> Vec<Message> {
        std::iter::from_fn(|| client.receiver.try_recv().ok()).collect()
    }

    /// The single response among `messages`.
    fn sole_response(messages: &[Message]) -> &lsp_server::Response {
        let mut responses = messages.iter().filter_map(|m| match m {
            Message::Response(response) => Some(response),
            _ => None,
        });
        let response = responses.next().expect("the server answered the request");
        assert!(responses.next().is_none(), "exactly one response");
        response
    }

    /// A `textDocument/hover` request carrying `params`.
    fn hover_request(id: i32, params: Value) -> Request {
        Request {
            id: RequestId::from(id),
            method: HoverRequest::METHOD.to_string(),
            params,
        }
    }

    #[test]
    fn a_request_whose_params_do_not_decode_is_answered_with_invalid_params() {
        let (_dir, mut server, client) = test_server();
        // A hover with no `position` at all: `HoverParams` cannot be built
        // from it, and the client is blocked on id 1 until somebody says so.
        let request = hover_request(1, json!({ "textDocument": { "uri": "file:///tmp/a.lua" } }));
        server
            .handle_request(request)
            .expect("a malformed request is not a transport failure");

        let messages = drain(&client);
        let response = sole_response(&messages);
        assert_eq!(response.id, RequestId::from(1));
        let error = response.error.as_ref().expect("an error response");
        assert_eq!(error.code, ErrorCode::InvalidParams as i32);
        // The message names the method and what would not decode, so the
        // client's log says which request it got wrong.
        assert!(
            error.message.contains("textDocument/hover"),
            "{}",
            error.message
        );
        assert!(error.message.contains("position"), "{}", error.message);
    }

    #[test]
    fn a_uri_with_an_unencoded_space_is_an_invalid_params_reply_not_a_dead_server() {
        let (_dir, mut server, client) = test_server();
        // Real clients emit these: the URI path grammar rejects the raw
        // space, so `Uri`'s deserializer fails before any handler runs.
        let request = hover_request(
            7,
            json!({
                "textDocument": { "uri": "file:///tmp/a b.lua" },
                "position": { "line": 0, "character": 0 },
            }),
        );
        server
            .handle_request(request)
            .expect("a malformed URI is not a transport failure");
        let messages = drain(&client);
        let error = sole_response(&messages)
            .error
            .as_ref()
            .expect("an error response");
        assert_eq!(error.code, ErrorCode::InvalidParams as i32);
    }

    #[test]
    fn a_well_formed_request_is_still_answered_after_a_malformed_one() {
        let (_dir, mut server, client) = test_server();
        server
            .handle_request(hover_request(1, Value::Null))
            .expect("malformed");
        // The whole point: the session survives, so the next request lands.
        server
            .handle_request(hover_request(
                2,
                json!({
                    "textDocument": { "uri": "file:///tmp/unknown.lua" },
                    "position": { "line": 0, "character": 0 },
                }),
            ))
            .expect("well-formed");

        let messages = drain(&client);
        let responses: Vec<_> = messages
            .iter()
            .filter_map(|m| match m {
                Message::Response(response) => Some(response),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2);
        assert!(responses[0].error.is_some(), "the first is the error");
        // An unknown document is `null`, not an error — the request was
        // answerable and got answered.
        assert!(responses[1].error.is_none(), "{:?}", responses[1].error);
        assert_eq!(responses[1].id, RequestId::from(2));
    }

    /// The `window/logMessage` payloads the server has sent.
    fn log_messages(messages: &[Message]) -> Vec<String> {
        messages
            .iter()
            .filter_map(|m| match m {
                Message::Notification(not) if not.method == super::LogMessage::METHOD => Some(
                    not.params["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                ),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_malformed_notification_is_logged_and_dropped() {
        let (_dir, mut server, client) = test_server();
        // `languageId` is required by `TextDocumentItem`; a client that omits
        // it has nothing to be answered with — notifications carry no id — so
        // the complaint goes to the log pane and the loop carries on.
        let notification = Notification {
            method: DidOpenTextDocument::METHOD.to_string(),
            params: json!({
                "textDocument": { "uri": "file:///tmp/a.lua", "version": 1, "text": "" },
            }),
        };
        server
            .handle_notification(notification)
            .expect("a malformed notification is not fatal");

        let messages = drain(&client);
        assert!(
            messages.iter().all(|m| !matches!(m, Message::Response(_))),
            "a notification is never answered"
        );
        let logged = log_messages(&messages);
        assert!(
            logged
                .iter()
                .any(|m| m.contains("textDocument/didOpen") && m.contains("languageId")),
            "{logged:?}"
        );
    }

    #[test]
    fn a_did_open_whose_uri_has_an_unencoded_space_is_logged_and_dropped() {
        let (_dir, mut server, client) = test_server();
        let notification = Notification {
            method: DidOpenTextDocument::METHOD.to_string(),
            params: json!({
                "textDocument": {
                    "uri": "file:///tmp/a b.lua",
                    "languageId": "lua",
                    "version": 1,
                    "text": "local x = 1\n",
                },
            }),
        };
        server
            .handle_notification(notification)
            .expect("a malformed URI is not fatal");
        let logged = log_messages(&drain(&client));
        assert!(
            logged.iter().any(|m| m.contains("textDocument/didOpen")),
            "{logged:?}"
        );
    }

    #[test]
    fn exit_without_a_prior_shutdown_ends_the_loop_with_an_error() {
        let (_dir, mut server, _client) = test_server();
        // The ordered `shutdown` → `exit` pair never reaches this table
        // (`Connection::handle_shutdown` consumes it), so an `exit` here is
        // the unordered case the spec assigns exit code 1 — which the CLI
        // reaches by way of this error.
        let error = server
            .handle_notification(Notification {
                method: super::Exit::METHOD.to_string(),
                params: Value::Null,
            })
            .expect_err("`exit` without `shutdown` is an error");
        assert!(
            error.to_string().contains("without a prior `shutdown`"),
            "{error}"
        );
    }
}
