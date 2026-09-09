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

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::Context;
use lsp_server::{
    Connection, ErrorCode, ExtractError, Message, Notification, Request, RequestId, Response,
};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, Exit, LogMessage, Notification as _, Progress, PublishDiagnostics,
    SetTrace,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest,
    FoldingRangeRequest, Formatting, GotoDefinition, GotoImplementation, GotoTypeDefinition,
    HoverRequest, InlayHintRequest, PrepareRenameRequest, RangeFormatting, References,
    RegisterCapability, Rename, Request as _, SelectionRangeRequest, SemanticTokensFullRequest,
    Shutdown, SignatureHelpRequest, WorkDoneProgressCreate, WorkspaceSymbolRequest,
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
    TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, TraceValue,
    TypeDefinitionProviderCapability, Uri, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressCreateParams, WorkDoneProgressEnd, WorkDoneProgressReport, WorkspaceEdit,
    WorkspaceSymbolResponse,
};
use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};
use luabox_lint::{LintConfig, UnknownRuleId, lint_source};
use luabox_manifest::layout::{self, DefFiles};
use luabox_manifest::model::{DialectId, Manifest};
use luabox_types::{Ambient, RockModule, RockSurfaces, build_ambient};
use rayon::prelude::*;

use crate::line_index::LineIndex;
use crate::merged_ambient::MergedAmbient;
use crate::requires::RequireExports;
use crate::sema::FileSema;
use crate::uri::uri_to_path;
use crate::{
    call_hierarchy, code_action, completion, diagnostics, document_highlight, fmt, folding,
    goto_definition, goto_implementation, goto_type_definition, hover, inlay_hints, references,
    rename, selection_range, semantic_tokens, signature_help, symbols,
};

/// Best-effort stderr logging for a long-running server.
///
/// `eprintln!` panics when the write fails, and the release profile is
/// `panic = "abort"` — so a client that closed our stderr (editor restart, a
/// torn-down log pane, a client that never captured it) would take the whole
/// language server down with SIGABRT the moment we tried to log. A dead log
/// pipe must never kill the server: the message is dropped instead. (The
/// CLI's emit seam exits 0 on a departed reader; that policy would be wrong
/// here — an LSP must keep serving.)
fn log_to_stderr(message: &str) {
    let _ = writeln!(std::io::stderr(), "{message}");
}

/// Run the server over stdio until the client sends `shutdown`/`exit`.
/// A leading `--stdio` argument, which editors commonly pass, is harmless:
/// stdio is the only transport in this tranche.
pub fn run_stdio() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run(connection)?;
    io_threads.join()?;
    Ok(())
}

/// The stack budget every thread that recurses over syntax trees gets,
/// mirroring `luabox-cli`'s `PINNED_STACK_BYTES`. Recursion depth is bounded
/// by the parser's own `MAX_DEPTH`, so this is a constant of the design and
/// not of whichever platform default applies: an unconfigured rayon pool
/// hands workers Rust's 2 MiB.
///
/// Public so that "mirroring `luabox-cli`'s" is a *checkable* claim rather
/// than a comment — `luabox-cli` asserts the two are equal
/// (`the_lsp_and_the_cli_pin_the_same_worker_stack`). Nothing else reads it.
pub const PINNED_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Pin the global rayon pool's worker stacks, best effort.
///
/// The `luabox lsp` subcommand reaches this crate through `luabox-cli`'s
/// `real_main`, which already pins the pool — but [`run_stdio`] and [`run`]
/// are `pub`, and an embedder calling either directly got rayon's 2 MiB
/// default on every worker of the startup rock harvest (Shockwave round 4).
/// Pinning at the library entry closes that. `Err` means a pool was already
/// built for this process — exactly what the CLI path does, and what a test
/// harness may do — and whoever configured it keeps their choice.
///
/// # What isolates this call
///
/// No program the *parser* accepts can tell a pinned worker from an unpinned
/// one. Recursion over a syntax tree is bounded by `MAX_DEPTH`, and
/// `luabox-syntax`'s `parsing_at_the_depth_limit_fits_a_default_stack` proves
/// — in a *debug* build, whose frames are several times fatter than
/// release's — that parsing at exactly that limit fits 2 MiB;
/// `a_deep_rock_tree_survives_the_startup_harvest` makes the same point for
/// the whole harvest pipeline through deliberately *unpinned* rayon workers.
/// Anything deeper than `MAX_DEPTH` is rejected before recursion begins.
///
/// That is a claim about parser-driven recursion, and wave 16's version of
/// this comment overreached by generalising it to "no such test can exist"
/// (Shockwave round 6). A **synthetic** recursion is not parser-capped, and
/// one calibrated to need well over 2 MiB and well under 16 MiB does isolate
/// the pin. `tests/pinned_stack.rs` is that test: it reaches this function
/// through [`run`] — the production call site, not a back door — and then
/// recurses on a global-pool worker. Delete the call in `run`, drop
/// `.stack_size` from the builder below, or lower both constants to rayon's
/// 2 MiB default, and that worker overflows and takes the test binary with
/// it.
///
/// The rest is still tested here: the constant matches the CLI's
/// (`luabox-cli`, above), and calling this is idempotent and never fatal
/// ([`tests::pinning_worker_stacks_is_idempotent_and_never_fatal`]) — the
/// `Err` arm, which is what production actually takes on the CLI path, where
/// `real_main` has already built the pool.
fn pin_worker_stacks() {
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(PINNED_STACK_BYTES)
        .build_global();
}

/// Run the server over any [`Connection`] (stdio in production,
/// [`Connection::memory`] in tests): initialize handshake, project
/// bootstrap, then the message loop. Returns after a clean shutdown.
pub fn run(connection: Connection) -> anyhow::Result<()> {
    // Both public entry points pin, not just `run_stdio`: an embedder that
    // owns its own transport reaches the same harvest through this one.
    pin_worker_stacks();
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
    // progress for every pause the server announces (the startup rock harvest,
    // the bootstrap index, the config reload), and dynamic file-watcher
    // registration. This rides on the `Server` rather than staying a
    // `bootstrap` argument because a client that does not advertise
    // `window.workDoneProgress` must see no `$/progress` traffic *at all* —
    // the reload path was sending it unconditionally (Shockwave round 5).
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
    // The initial trace level (N22): `Off` unless the client explicitly asks
    // for tracing, per spec ("If omitted trace is disabled ('off')") — the
    // server's own diagnostic chatter (`Self::merged_ambient`'s rebuild log)
    // is silent by default and stays silent for every client that never asks.
    let trace = params.trace.unwrap_or_default();

    let root = root_path(&params)
        .or_else(|| std::env::current_dir().ok())
        .context("cannot determine a workspace root")?;
    // `Server::new` runs the startup rock harvest, and announces it — which is
    // a server-initiated `window/workDoneProgress/create` before the client's
    // `initialized` notification. That is protocol-legal: LSP only bars the
    // server from speaking *until it has responded to `initialize`*, and
    // `initialize_finish` above is that response. The bootstrap token below
    // has been sent from the same window since it shipped.
    let mut server = Server::new(connection, root, work_done_progress, trace);
    if watch_files {
        server.register_file_watchers();
    }
    server.bootstrap();
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
    /// The absolute paths `def_sources`'s project-local entries were read
    /// from (N18) — [`ambient_def_paths`], computed alongside `def_sources`
    /// from the same resolution so the two can never disagree about what is
    /// genuinely ambient.
    def_paths: HashSet<PathBuf>,
    /// The `---@alias`/`---@enum` names `def_sources` declares (round 8
    /// review, F7) — [`merged_ambient::alias_or_enum_names`] over the very
    /// same `Vec`, so the names and the layer built from those texts cannot
    /// name different things.
    def_alias_names: HashSet<String>,
    /// The `lua_modules/share/lua/<X.Y>/` version directory whose installed rock
    /// sources are harvested for their type surfaces (#30) — chosen by `[build]
    /// target` (the edition when unset), exactly as `luabox check` chooses it, so
    /// the editor harvests the tree the build resolves against.
    rock_version_dir: &'static str,
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
            def_paths: HashSet::new(),
            def_alias_names: HashSet::new(),
            rock_version_dir: luabox_bundle::rocks_version_dir(Dialect::Lua54),
            lint: LintConfig::new(),
            unknown_lint_rules: Vec::new(),
        };
        let Ok(text) = fs::read_to_string(root.join("luabox.toml")) else {
            return defaults;
        };
        let Ok(manifest) = Manifest::parse(&text) else {
            log_to_stderr("luabox-lsp: invalid luabox.toml; using defaults (5.4, warn)");
            return defaults;
        };
        // One `[lint]` translation for the whole workspace (`luabox-lint`'s),
        // shared with `luabox lint` — including the unknown-rule-id check the
        // manifest parser cannot do (CC-M8).
        let (lint, unknown_lint_rules) = LintConfig::from_manifest(&manifest.lint);
        // One resolution, read twice (F7): the alias/enum names are harvested
        // from the exact `Vec` the ambient layer is built from, not from a
        // second walk that could resolve a different set of files.
        let def_sources = ambient_def_sources(root, &manifest);
        let def_alias_names = crate::merged_ambient::alias_or_enum_names(&def_sources);
        Self {
            // `Manifest::parse` types `[package] edition` as a closed
            // `DialectId`, so this maps inward exhaustively — there is no
            // unknown-edition fallback left to take.
            dialect: syntax_dialect(manifest.package.edition),
            strictness: Strictness::from_manifest_flag(manifest.types.strict),
            out_dir: Some(root.join(&manifest.build.out)),
            def_sources,
            def_alias_names,
            def_paths: ambient_def_paths(root, &manifest),
            rock_version_dir: luabox_bundle::rocks_version_dir(syntax_dialect(
                manifest.build.target,
            )),
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

/// The absolute paths of the project's own `[types] defs` files (N18) — the
/// genuine ambient scope [`crate::merged_ambient::MergedAmbient::ambient_paths`]
/// backs [`crate::sema::locate_field`]'s elevated precedence with, so a
/// `*.d.lua`-named file that is not actually configured here gets none.
///
/// Project-local only: [`layout::resolve_dep_defs`]'s files live under
/// `lua_modules/<dep>/defs/` (`layout::VENDOR_DIR`), which
/// [`layout::collect_lua_files`] always excludes from the ordinary source
/// walk — they never appear in [`luabox_db::Analysis::files`] at all, so
/// `locate_field` could never visit one regardless of what this returns.
/// [`layout::resolve_project_defs`] labels a project-local def by its
/// root-relative path (`defs/love.d.lua`) — the exact same walk `bootstrap`
/// uses to populate the host, so a label here and a path in
/// [`luabox_db::Analysis::files`] name the same file whenever both exist.
fn ambient_def_paths(root: &Path, manifest: &Manifest) -> HashSet<PathBuf> {
    let (project_defs, _unresolved) = layout::resolve_project_defs(root, &manifest.types.defs);
    project_defs
        .into_iter()
        .map(|def| root.join(&def.label))
        .collect()
}

/// Harvest the type surfaces of the project's vendored luarocks tree (#30) —
/// `luabox_types::rocks::harvest_file` over `layout::collect_rock_sources`,
/// folded by `RockSurfaces::fold`, the same pair `luabox check` drives, so a
/// rock's classes and export types resolve in the editor exactly as they do in
/// CI.
///
/// # It runs on two paths, and both are synchronous
///
/// - [`Server::new`] — after the `initialize` handshake, before the main
///   loop, so every millisecond here is a millisecond the editor shows no
///   diagnostics. Wrapped in a work-done token since round 5
///   ([`Server::harvest_announced`]): the handshake has already been answered
///   by then, so a server-initiated `window/workDoneProgress/create` is
///   protocol-legal, and the pause is attributable rather than silent;
/// - [`Server::reload_config`] — reached from `workspace/
///   didChangeConfiguration` and from a watched edit to `luabox.toml`. A
///   manifest edit can move the version directory (`[build] target`) and the
///   tree itself may have grown a rock, so the reload re-harvests. That
///   happens **on the main loop**, so the same wall-clock cost lands as a
///   mid-session stall every time the manifest is saved — not once, at
///   startup, as this comment used to claim (Shockwave round 4). It is wrapped
///   in the same work-done progress token, so the pause is visible in the
///   editor instead of looking like a hang.
///
/// Both tokens are gated on the client's `window.workDoneProgress` capability
/// ([`Server::progress`]): a client that did not ask for progress receives
/// none, and simply sees the pause.
///
/// Per-file reduction is pure and independent, so it rides the rayon pool;
/// the fold is what orders the surfaces (path order = precedence), so the
/// parallel and sequential forms give the identical result.
///
/// The pool is the global one, pinned to a 16 MiB worker stack by
/// `luabox-cli`'s `real_main` on the `luabox lsp` path and by
/// [`pin_worker_stacks`] for a library caller that owns its own transport —
/// so these workers get the stack the syntax walks need, not rayon's 2 MiB
/// default. `a_deep_rock_tree_survives_the_startup_harvest` drives the
/// harvest at depth 195 over 32 files through the *unpinned* path on purpose.
///
/// # Measured, by a committed harness
///
/// `scripts/lsp-startup-bench.sh` reproduces every number below end to end:
/// it generates the corpus (`gen-corpus --rock-tree`: 50 files, 102,813 lines
/// of `---@class`-annotated Lua under `lua_modules/share/lua/5.4/`), drives
/// the real stdio protocol (`initialize` -> `initialized` -> `didOpen`) and
/// times to the FIRST `publishDiagnostics`. It is not a CI gate — SPEC.md's
/// LSP perf budget is future work — but the claim is re-runnable, which the
/// prose-described corpus and scratch-directory driver it replaces were not.
///
/// **Host** (the numbers below are one host's, and only that host's):
/// `Intel(R) Xeon(R) Processor @ 2.80GHz`, 4 vCPU, containerized/virtualized
/// sandbox, 7 runs per row.
///
/// | harvest                                         | median  | min     |
/// |-------------------------------------------------|---------|---------|
/// | sequential (`RAYON_NUM_THREADS=1`, same binary)  | 2751 ms | 2673 ms |
/// | parallel                                        |  760 ms |  718 ms |
///
/// 3.6x. The baseline is the same binary forced to one rayon worker rather
/// than a pre-fix build, so the comparison is of the parallelism and of
/// nothing else in a commit range. The same project with the rock tree removed
/// publishes in 11 ms, so the harvest is effectively the whole of that number.
///
/// **The sequential row is single-host and does not reproduce.** A reviewer's
/// own 4 vCPU box measured a *maximum* of 2384 ms against this host's
/// *minimum* of 2673 — non-overlapping ranges — and a ratio of 2.8x rather
/// than 3.6x. One rayon worker doing all the parsing is exactly the shape that
/// is sensitive to CPU steal on a shared virtualized host, which is why the
/// host is recorded here and why the harness, not the constant, is the thing
/// that is committed. Re-run it before quoting either number.
///
/// # Why it is still synchronous
///
/// The asynchronous form (background thread, republish on completion) buys
/// the latency with a *correctness* cost the synchronous form does not have:
/// between the first publish and the harvest landing, every project file
/// naming a rock class would be diagnosed against an empty rock layer, so
/// `LB0305`/`LB0306` would flash red and then vanish. A burst of wrong
/// squiggles is worse than a pause with a progress indicator on it.
///
/// That trade is easier to defend at startup than on reload, and the reload
/// stall is the honest reason to reopen it — along with the *shape* of the
/// tree, not this constant: a vendored tree several times larger changes the
/// trade, and at that point the republish path (and the false-positive window
/// it opens) is the thing to design, not another constant factor here.
///
/// Rock sources that could not be parsed are named in the client's log pane:
/// that is where a debug-level note about vendored code belongs, and it is the
/// answer to "why did this rock's types not show up?". They are never published
/// as diagnostics — no document owns them.
fn harvest_rock_tree(root: &Path, version_dir: &str, ambient: &Ambient) -> RockSurfaces {
    let sources: Vec<RockModule> = layout::collect_rock_sources(root, version_dir)
        .into_iter()
        .map(|source| RockModule {
            module: source.module,
            label: source.label,
            path: source.path,
            text: source.text,
        })
        .collect();
    let files: Vec<luabox_types::RockFile> = sources
        .par_iter()
        .map(|source| luabox_types::rocks::harvest_file(ambient, source))
        .collect();
    let harvested = RockSurfaces::fold(&sources, files);
    for label in harvested.skipped() {
        log_to_stderr(&format!(
            "luabox-lsp: skipped unparseable rock source `{label}` while harvesting types"
        ));
    }
    harvested
}

/// The bookkeeping behind cross-file diagnostics (round 8 review, F4).
///
/// # What it is for
///
/// Checking one file can produce a diagnostic whose primary span belongs to
/// **another** — the `LB0317`/`LB0318` `---@class` ancestry pair points at
/// the offending class's declaration, wherever it lives
/// ([`diagnostics::FileDiagnostics`]). LSP has no way to say "add this one
/// diagnostic to that document": `publishDiagnostics` **replaces** the whole
/// set for a URI. So publishing a foreign group means publishing everything
/// that document should show, which means knowing everything that document
/// should show — and the code this replaces knew none of it. It published a
/// foreign group and forgot it had, which produced four distinct failures
/// with one root cause:
///
/// - **never cleared** — fix the cycle in the declaring file and the new pass
///   produces no foreign group at all, so the loop that would have
///   republished the other document never runs; its stale diagnostic stayed
///   until that document was itself touched;
/// - **vanished on a keystroke** — the group is produced by the *consuming*
///   file's pass, so publishing the file it was painted onto replaced that
///   file's set with its own half alone;
/// - **last writer won** — two consumers each producing a group for one
///   target: whichever published last replaced the other's finding;
/// - **nondeterministic** — a batch republish walked a `HashMap`, so which of
///   the two survived depended on hash order.
///
/// # The record
///
/// [`Self::contributions`] is the cross-file half: contributor → target →
/// that pass's group. It is authoritative and total — a pass over a
/// contributor **replaces** its whole entry, so a group that is no longer
/// produced is a group that is no longer in the ledger, which is what makes
/// clearing fall out rather than needing its own path.
/// [`Self::own`] is each file's same-file half, as of the last pass over it,
/// and prunes on the same rule (R11-4): both halves store only what a pass
/// actually produced, so neither grows an entry per file in the workspace.
/// A published set is then, for any target, that target's own half plus every
/// contributor's group for it — with `BTreeMap` ordering throughout, so the
/// merge is a function of the workspace and nothing else.
#[derive(Default)]
struct ForeignLedger {
    /// Contributor path → (target path → the group that pass produced for
    /// that target). A contributor with nothing to contribute has no entry:
    /// [`Self::record`] removes it rather than storing an empty map, so
    /// `contributions` never grows an entry per file in the workspace.
    contributions: BTreeMap<PathBuf, BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>>,
    /// Path → the own half of the last pass over that file, i.e. exactly the
    /// same-file diagnostics that file's URI is currently showing.
    ///
    /// Self-pruning on the same rule as [`Self::contributions`] (R11-4): a
    /// pass that produces nothing removes the entry rather than storing an
    /// empty vector. [`Self::published_set`] reads this map through
    /// `unwrap_or_default`, so an absent entry and an empty one are the same
    /// answer — and storing the empty one would grow `own` by an entry per
    /// distinct file ever checked, never freed for the life of the session,
    /// which is precisely the unbounded growth `contributions` avoids.
    own: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// Path → the workspace-derived findings that belong to it, as of the
    /// last pass that computed them — the declared-`---@class` cycle set
    /// (`diagnostics::class_cycle_diagnostics`, round 12 review R12-1).
    ///
    /// **Not** per contributor, unlike [`Self::contributions`], and that is
    /// the whole point. Every pass over any file derives the SAME answer for
    /// the whole workspace, so filing it as a contribution would put one
    /// cycle on a document once per file the session had ever checked, and
    /// each of those copies would go stale on its own: fixing the file that
    /// closed the loop clears the copy contributed by the pass that just ran
    /// and leaves every other contributor's standing until they are
    /// themselves re-checked. One authoritative map, replaced whole by each
    /// pass that computes it, has neither problem — the same reason
    /// `contributions` replaces a contributor's whole entry rather than
    /// merging into it.
    workspace: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
}

/// Whether one completed pass derived the workspace-derived findings, and if
/// so what they are.
///
/// An enum rather than [`ForeignLedger::record`]'s former `Option<map>`
/// positional parameter (round 13 review, clean-code note). The two states
/// are opposite instructions — "here is the whole workspace answer, replace
/// yours" versus "I did not look, keep yours" — and `Option` spells the first
/// one's *empty* case exactly like a mistake: a `Some(BTreeMap::new())`
/// slipped in by a caller that had nothing to say would read as "the
/// workspace has no cycles" and wipe every one of them off every document
/// until the next pass anywhere put them back. Named variants make that
/// call site say which it means.
enum WorkspaceScan {
    /// This pass derived the whole workspace answer. It **replaces** the
    /// stored one, and the targets of both the old and the new are
    /// republished — an empty map here is a real, measured "no cycles left".
    Recomputed(BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>),
    /// This pass derived nothing about the workspace; the stored answer
    /// stands, and no document is republished on its account.
    Skipped,
}

impl ForeignLedger {
    /// Record one completed pass over `source` and answer with every path
    /// whose published set may have moved, in sorted order.
    ///
    /// The affected set is the union of what `source` contributed *before*
    /// this pass, what it contributes now, `source` itself, and — when this
    /// pass recomputed one — the targets of the previous and the new
    /// workspace answer. The "before" terms are what make a cleared
    /// diagnostic clear: a target that has just dropped out is still
    /// republished, now without it.
    ///
    /// `workspace` is [`WorkspaceScan::Skipped`] for a publish that did
    /// **not** derive the workspace-derived findings — a path the analysis
    /// does not know, which has no answer to give about anything. See that
    /// type for why this is not an `Option`.
    fn record(
        &mut self,
        source: &Path,
        own: Vec<lsp_types::Diagnostic>,
        foreign: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
        workspace: WorkspaceScan,
    ) -> Vec<PathBuf> {
        let mut affected: BTreeSet<PathBuf> = BTreeSet::new();
        affected.insert(source.to_path_buf());
        if let Some(previous) = self.contributions.get(source) {
            affected.extend(previous.keys().cloned());
        }
        affected.extend(foreign.keys().cloned());
        if let WorkspaceScan::Recomputed(workspace) = workspace {
            affected.extend(self.workspace.keys().cloned());
            affected.extend(workspace.keys().cloned());
            self.workspace = workspace;
        }

        if foreign.is_empty() {
            self.contributions.remove(source);
        } else {
            self.contributions.insert(source.to_path_buf(), foreign);
        }
        if own.is_empty() {
            self.own.remove(source);
        } else {
            self.own.insert(source.to_path_buf(), own);
        }
        affected.into_iter().collect()
    }

    /// Everything `target`'s document should currently show: its own half,
    /// then every contributor's group for it, contributors in path order.
    ///
    /// Every contributor is merged, not just the one that happens to be
    /// publishing — two consumers reporting different findings against one
    /// declaring file both appear, and neither erases the other.
    ///
    /// The workspace-derived half sits between the two: after the target's
    /// own findings, before the per-contributor ones, and once — see
    /// [`Self::workspace`].
    fn published_set(&self, target: &Path) -> Vec<lsp_types::Diagnostic> {
        let mut out = self.own.get(target).cloned().unwrap_or_default();
        out.extend(self.workspace.get(target).into_iter().flatten().cloned());
        for groups in self.contributions.values() {
            if let Some(group) = groups.get(target) {
                out.extend(group.iter().cloned());
            }
        }
        out
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
    /// The absolute paths of the project's own `[types] defs` files
    /// [`Self::ambient`] was built from (N18) — [`ambient_def_paths`], kept
    /// alongside `ambient` and installed into every [`MergedAmbient`] so
    /// `sema::locate_field` can tell a genuinely ambient `.d.lua` file from
    /// one that merely looks like one.
    ambient_paths: HashSet<PathBuf>,
    /// The `---@alias`/`---@enum` names [`Self::ambient`]'s definition
    /// sources declare (round 8 review, F7) — project *and* dependency defs,
    /// installed into every [`MergedAmbient`] so
    /// `requires::require_struct_fields`'s gate can recognise a
    /// dependency-declared alias the `analysis.files()` scan structurally
    /// cannot see. Names only, not the source texts: a large definition
    /// package is megabytes, and the gate asks only for a name.
    ambient_alias_names: HashSet<String>,
    /// The type surfaces harvested from the project's vendored luarocks tree
    /// (#30), built once at startup alongside [`Self::ambient`]: rock classes,
    /// enums and aliases, plus each rock module's `require`-export type. Merged
    /// per file in [`crate::diagnostics`] — *after* the project's own types, so
    /// explicit beats implicit.
    rocks: RockSurfaces,
    /// The merged editor ambient — [`Self::ambient`] + the workspace-global
    /// project types + the rock tree — cached per
    /// [`luabox_db::Analysis::revision`]. What this delivers, **measured**
    /// (#57): every surface reading the SAME revision reuses one merge —
    /// `code_actions` reusing the entry `publish_lua` installed for the same
    /// request, and hover/completion/signature-help/goto-definition between
    /// edits. What it does **not** deliver, also measured: diagnostics on a
    /// keystroke. `DidChangeTextDocument` bumps the revision (`host.rs`)
    /// immediately before `publish_lua` reads this cache, so the very next
    /// read is guaranteed a miss — one full re-clone + re-merge per
    /// keystroke, same as before this cache existed. A prior revision of
    /// this comment claimed the keystroke case as a beneficiary; it was
    /// not, and nothing had measured it.
    ///
    /// The revision key makes staleness structural: any [`Change`] bumps it,
    /// and [`Self::reload_config`] — which swaps the base layers the merge
    /// is built from without touching the host — clears the cache by hand
    /// (see the comment there, and #59, for why that line is *not* dead
    /// code despite [`Self::host`]'s revision bumping on the very next line
    /// too).
    ///
    /// `RefCell` for the same reason as [`Self::pending`]: the read paths run
    /// behind `&self`, and the server is single-threaded.
    merged_ambient: RefCell<Option<(u64, Rc<MergedAmbient>)>>,
    /// The declared-`---@class` cycle pass for the whole workspace, cached on
    /// the host revision beside [`Self::merged_ambient`] and by the same
    /// pattern (round 13 review, noted cost).
    ///
    /// Every publish derives the SAME answer from the same workspace — that
    /// is the whole reason it lives in one ledger slot rather than per
    /// contributor — so recomputing it inside every [`diagnostics::diagnostics`]
    /// call meant re-collecting every `---@class` the project declares, and
    /// re-running Tarjan over them, on every keystroke of every open buffer.
    ///
    /// Unlike the merge, this needs **no** manual invalidation: its only
    /// inputs are the analysis and the strictness, and both move the host
    /// revision ([`luabox_db::AnalysisHost::apply_change`]), so there is no
    /// staleness the key cannot see.
    class_cycles: RefCell<Option<(u64, Rc<diagnostics::ClassCycles>)>>,
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
    ///
    /// A `BTreeMap`, not a `HashMap` (round 8 review, F4): a batch republish
    /// walks this map and publishes as it goes, so hash order made *which*
    /// document published last — and, before the ledger below, which
    /// cross-file group survived — depend on nothing the workspace can see.
    /// The same unchanged workspace emitted different diagnostics run to run.
    open_docs: BTreeMap<PathBuf, Uri>,
    /// Which file contributed which cross-file diagnostic to which other
    /// file, and each file's own half — see [`ForeignLedger`].
    foreign: ForeignLedger,
    /// Whether the client advertised `window.workDoneProgress`. Every
    /// `$/progress` the server sends is gated on this — a client that did not
    /// ask for progress receives none, on any path.
    progress: bool,
    /// The client's negotiated trace level (`initialize`'s `trace` field,
    /// `$/setTrace` afterwards) — `Off` unless a client opts in, per spec
    /// default. Gates [`Self::merged_ambient`]'s "rebuilt@" log (N22): that
    /// trace fired unconditionally on every cache miss, which a keystroke
    /// guarantees (`Self::merged_ambient`'s own doc), so a client with no
    /// interest in it — the overwhelming majority, since `trace` defaults to
    /// `Off` and nothing else in this server asks a user to turn it on — used
    /// to receive one `window/logMessage` per revision for the life of the
    /// session regardless. `Cell` for the same reason `progress_seq` is: the
    /// logging call sites run behind `&self`.
    trace: Cell<TraceValue>,
    /// A monotonic counter making every server-created progress token — and
    /// the `window/workDoneProgress/create` request id that carries it —
    /// unique within the session.
    ///
    /// Both used to be derived from the token *name* alone, which is a
    /// compile-time constant, so three config reloads sent three `create`
    /// requests sharing one id and one token (Shockwave round 6). Neither is
    /// legal: JSON-RPC ids must be unique among outstanding requests, and LSP
    /// requires a server-generated token to be unique. A client that tracks
    /// its outstanding requests by id sees the second `create` collide with
    /// the first. The startup tokens fire once each, but get the same
    /// treatment — a rule with an exception is a rule nobody can check.
    ///
    /// A `Cell` because the progress helpers take `&self`:
    /// `harvest_announced` is called as `self.rocks = self.harvest_announced(…)`,
    /// which a `&mut self` chain cannot express. The server is
    /// single-threaded, so there is nothing to share it across.
    progress_seq: Cell<u64>,
    /// Client messages read off the wire by [`Self::await_progress_create`]
    /// while it was waiting for a `window/workDoneProgress/create` response,
    /// and not yet handled. [`Self::main_loop`] drains this before reading
    /// anything new, so a notification the client sent during the startup
    /// harvest is handled in arrival order rather than lost.
    ///
    /// A `RefCell` for the same reason [`Self::progress_seq`] is a `Cell`:
    /// the progress helpers run behind `&self`.
    pending: RefCell<VecDeque<Message>>,
    /// Whether a `window/workDoneProgress/create` has already gone
    /// unanswered. A client that ignores one ignores them all, and the wait
    /// costs [`PROGRESS_CREATE_TIMEOUT`] every time it is paid: two startup
    /// tokens plus one per config reload.
    ///
    /// Once set, **no further create is sent at all** and every later
    /// announcement is silent for the rest of the session. Wave 19 only
    /// skipped the *wait*, which kept the latency win and lost the guarantee
    /// the wait exists for: the create still went out, the client's late
    /// error answer landed in [`Self::main_loop`]'s discard arm, and
    /// `$/progress` went out under a token it had just refused (Shockwave
    /// round 9, B2). A refusal check a latency optimisation can step around
    /// is not a check. Not sending is simpler than polling for a late answer
    /// and strictly stronger — there is no token, so there is nothing to
    /// report under and no traffic to a client that is not listening.
    ///
    /// The cost is disclosed rather than hidden: **one timeout mutes progress
    /// for the session**, so a client that stalls once during startup and
    /// recovers gets no progress on later reloads. That client was already
    /// getting `$/progress` under tokens it never acknowledged, which is the
    /// thing this mechanism was built to stop.
    ///
    /// Only a *timeout* sets this. A client that answers — with a result or
    /// with an error — has told the server something, and the next create is
    /// sent and waited for normally.
    create_unanswered: Cell<bool>,
    /// Whether the client has asked to end the session: a `shutdown` request
    /// or an `exit` notification has been seen, wherever it was seen.
    ///
    /// Sticky, and that is the point. Aborting *one* create window on a
    /// session-ender is not enough, because `run` opens two before the loop
    /// (the startup harvest, then `bootstrap`) and a queued reload can open
    /// more from inside it. Window 1 would abort on the `shutdown` and leave
    /// the `exit` on the channel — correct — and window 2 would then drain
    /// that `exit` onto [`Self::pending`], where
    /// [`Self::shutdown_handshake`]'s predecessor could not see it: 30 s of
    /// waiting for a notification the server was holding, then exit 1,
    /// byte-identical to the pre-round-8 behaviour (Shockwave round 9, B1).
    ///
    /// Once set, [`Self::create_progress_token`] sends nothing and returns no
    /// token, so no window opens and nothing is announced to a client that is
    /// leaving.
    shutting_down: Cell<bool>,
}

/// What a `window/workDoneProgress/create` came back as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CreateOutcome {
    /// The client answered with a result: the token is live.
    Accepted,
    /// The client answered with an error: the token is refused and nothing
    /// may be reported under it.
    Refused,
    /// No answer arrived — the wait timed out or the transport died. The
    /// caller reports under the token regardless, which is the behaviour this
    /// whole mechanism replaced and so is no worse than it.
    Unanswered,
    /// The session is ending: the wait was abandoned because a `shutdown` or
    /// an `exit` arrived. Distinct from [`Self::Unanswered`] because the
    /// caller must **not** report under the token — a `begin`/`report`/`end`
    /// sequence to a client that has asked to shut down is traffic nobody
    /// wants, and it used to go out because the abort path returned the token
    /// like any other unanswered create.
    Ending,
}

/// Whether a message is one of the two that end the session, and so must
/// reach [`Server::main_loop`] rather than wait behind a progress token.
fn ends_the_session(message: &Message) -> bool {
    match message {
        Message::Request(request) => request.method == Shutdown::METHOD,
        Message::Notification(notification) => notification.method == Exit::METHOD,
        Message::Response(_) => false,
    }
}

/// How long the server waits for the client to answer
/// `window/workDoneProgress/create` before reporting under the token anyway.
///
/// The wait is what the protocol asks for; the bound is what keeps a client
/// that answers nothing from freezing the session. Timing out and sending the
/// `begin` regardless is exactly the behaviour this whole mechanism replaced,
/// so the degraded path is no worse than the old unconditional one — and a
/// local editor answers a create in well under a millisecond (Shockwave's
/// round-7 capture measured the *server's* two sends 0.1 ms apart).
const PROGRESS_CREATE_TIMEOUT: Duration = Duration::from_millis(250);

/// How long [`Server::shutdown_handshake`] waits for the `exit` that must
/// follow a `shutdown`. Matches what `Connection::handle_shutdown` allowed,
/// so replacing it changes which messages are *found*, not how long a client
/// that really does go silent is given.
const SHUTDOWN_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The progress token id and title the startup rock harvest announces itself
/// under. Distinct from the reload's so a client (and the protocol tests) can
/// tell the two pauses apart.
const STARTUP_HARVEST_PROGRESS: (&str, &str) = ("luabox/rock-harvest", "Indexing luabox rock tree");

/// The same, for the config reload's re-harvest.
const RELOAD_HARVEST_PROGRESS: (&str, &str) = ("luabox/reload", "Reloading luabox configuration");

impl Server {
    fn new(connection: Connection, root: PathBuf, progress: bool, trace: TraceValue) -> Self {
        let config = ProjectConfig::discover(&root);
        let ambient = build_ambient(config.dialect, &config.def_sources);
        let known_globals = ambient.global_names().clone();
        let mut host = AnalysisHost::new(config.dialect, config.strictness);
        // Anchor the db's `require` resolution at the workspace root so module
        // strings resolve exactly as `luabox check` resolves them on disk (the
        // bundler's SPEC.md §7 path-mapping) — editor and CI in lockstep.
        host.set_root(root.clone());
        let mut server = Self {
            connection,
            host,
            root,
            dialect: config.dialect,
            strictness: config.strictness,
            out_dir: config.out_dir,
            ambient,
            ambient_paths: config.def_paths,
            ambient_alias_names: config.def_alias_names,
            rocks: RockSurfaces::default(),
            merged_ambient: RefCell::new(None),
            class_cycles: RefCell::new(None),
            lint: config.lint,
            known_globals,
            open_docs: BTreeMap::new(),
            foreign: ForeignLedger::default(),
            progress,
            trace: Cell::new(trace),
            progress_seq: Cell::new(0),
            pending: RefCell::new(VecDeque::new()),
            create_unanswered: Cell::new(false),
            shutting_down: Cell::new(false),
        };
        // Safe to send: `run` only builds the server after `initialize_finish`.
        server.log_lint_config_problems(&config.unknown_lint_rules);
        // The startup harvest runs *here*, after the struct exists, so it can
        // go through the same announced-harvest helper the reload uses. It used
        // to run before the struct was built, which is why it was the one
        // synchronous pause with no token on it — Shockwave captured 746 ms of
        // protocol silence between the `initialize` response and the bootstrap
        // token, covering an index that itself took 0.1 ms (round 5). The work
        // is unchanged; what is new is that the client can attribute the wait.
        server.rocks = server.harvest_announced(config.rock_version_dir, STARTUP_HARVEST_PROGRESS);
        server
    }

    /// Run the rock-tree harvest inside a work-done progress token, so the
    /// synchronous pause is attributable in the editor instead of reading as a
    /// hang. Both callers — startup and [`Self::reload_config`] — come through
    /// here, and both inherit the client-capability gate in
    /// [`Self::begin_progress_titled`].
    ///
    /// Harvests against [`Self::ambient`], which both callers set before
    /// calling: `new` builds it into the struct, `reload_config` installs the
    /// freshly built one first.
    fn harvest_announced(&self, version_dir: &str, progress: (&str, &str)) -> RockSurfaces {
        let token = self.begin_progress_titled(progress.0, progress.1);
        let rocks = harvest_rock_tree(&self.root, version_dir, &self.ambient);
        if let Some(token) = &token {
            self.end_progress(token);
        }
        rocks
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
        // Re-harvested too: a manifest edit can move the version directory
        // (`[build] target`), and the tree itself may have grown a rock since
        // startup — a reload is the cheapest honest moment to notice.
        //
        // This is the startup harvest's whole cost, landing mid-session on the
        // main loop (see `harvest_rock_tree`), so it is announced: without the
        // token the editor goes unresponsive for the best part of a second
        // every time `luabox.toml` is saved, with nothing to attribute it to.
        // The work itself is unchanged — this is a label on the pause, not a
        // restructuring of the loop.
        //
        // The new ambient layer is installed first because the harvest reads
        // `self.ambient`, and the freshly discovered one is what the rocks must
        // be resolved against.
        self.known_globals = ambient.global_names().clone();
        self.ambient = ambient;
        self.ambient_paths = config.def_paths;
        self.ambient_alias_names = config.def_alias_names;
        self.rocks = self.harvest_announced(config.rock_version_dir, RELOAD_HARVEST_PROGRESS);
        // The merge's BASE layers just changed while the host (and so the
        // revision) did not — the one staleness the revision key cannot see.
        *self.merged_ambient.borrow_mut() = None;
        self.lint = config.lint;
        // Re-report: the reload may have introduced (or fixed) a typo'd key.
        self.log_lint_config_problems(&config.unknown_lint_rules);
        // Guarded (#59): an unconditional `apply_change` here bumps the
        // host's revision on every reload regardless of whether strictness
        // actually moved, which pays for a full `clone_surface` +
        // `merge_file_types` over every project file it never asked for —
        // and, worse, makes the manual `merged_ambient` clear above
        // untestable, since the bump alone would invalidate the cache and
        // hide whether that line did anything. A reload that only touches
        // `[types] defs` or the rock tree (this test's shape) now leaves the
        // host's revision untouched, so the clear above is what is doing the
        // work — and is the only thing that can be doing it.
        let strictness_changed = config.strictness != self.strictness;
        self.strictness = config.strictness;
        self.out_dir = config.out_dir;
        if strictness_changed {
            self.host
                .apply_change(Change::SetStrictness(config.strictness));
        }
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
    ///
    /// In path order, because [`Self::open_docs`] is a `BTreeMap` (F4): each
    /// iteration publishes as it goes, so the walk order is externally
    /// observable, and hash order made an unchanged workspace produce
    /// different batches run to run.
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
    /// When the client supports work-done progress ([`Self::progress`]), the
    /// index is wrapped in a server-created `$/progress` token — begin, a
    /// report per file loaded, then end — so a large workspace shows a
    /// progress indicator at startup instead of an unexplained pause.
    fn bootstrap(&mut self) {
        let files = self.collect_lua_files();
        let token = self.begin_progress(files.len());

        for (i, path) in files.into_iter().enumerate() {
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                // Skipping is right — one unreadable file must not stop the
                // index — but skipping *silently* is not: every cross-file
                // answer this file would have contributed to is then wrong
                // with no trace of why. `collect_lua_files` already reports
                // its own failure; this is the per-file half of that rule.
                Err(err) => {
                    self.log_message(
                        MessageType::WARNING,
                        format!(
                            "workspace index: skipping unreadable file {}: {err}",
                            path.display()
                        ),
                    );
                    continue;
                }
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
            self.end_progress(token);
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
                log_to_stderr(&format!("luabox-lsp: cannot index workspace: {err}"));
                Vec::new()
            })
    }

    /// Create the bootstrap progress token on the client and send `begin`.
    ///
    /// `None` when the client did not advertise `window.workDoneProgress`, so
    /// the caller sends nothing at all — the same shape, and the same gate, as
    /// [`Self::begin_progress_titled`]. The gate used to live at the single
    /// call site instead, which made "every `$/progress` is gated" a property
    /// of *where this is called from* rather than of the function. The reload
    /// path is what a second call site forgetting it looks like (Shockwave
    /// round 5); this is the structural version of that fix.
    fn begin_progress(&self, total: usize) -> Option<ProgressToken> {
        if !self.progress {
            return None;
        }
        let token = self.create_progress_token("luabox/bootstrap")?;
        self.send_progress(
            &token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: "Indexing workspace".to_string(),
                message: Some(format!("0/{total} files")),
                percentage: Some(0),
                ..WorkDoneProgressBegin::default()
            }),
        );
        Some(token)
    }

    /// An untotalled work-done token for a single indivisible step — a rock
    /// harvest, which has no per-file report to make from inside
    /// `harvest_rock_tree`'s `par_iter`.
    ///
    /// `None` when the client did not advertise `window.workDoneProgress`, so
    /// the caller sends nothing at all. This gate used to live only at
    /// `bootstrap`'s call site, which left the reload path announcing itself
    /// to clients that never asked (Shockwave round 5).
    ///
    /// Also `None` when the client **refused** the token — see
    /// [`Self::create_progress_token`]. Both are the same fact from the
    /// caller's side ("there is no token to report under"), so both take the
    /// same silent path and no `begin`/`report`/`end` goes out.
    fn begin_progress_titled(&self, token_name: &str, title: &str) -> Option<ProgressToken> {
        if !self.progress {
            return None;
        }
        let token = self.create_progress_token(token_name)?;
        self.send_progress(
            &token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: title.to_string(),
                ..WorkDoneProgressBegin::default()
            }),
        );
        Some(token)
    }

    fn end_progress(&self, token: &ProgressToken) {
        self.send_progress(token, WorkDoneProgress::End(WorkDoneProgressEnd::default()));
    }

    /// Ask the client to create a server-side progress token, and **wait for
    /// it to answer** before the caller reports anything under that token.
    ///
    /// LSP puts the token in the client's hands: `window/workDoneProgress/create`
    /// is a *request*, and a `$/progress` under a token the client has not
    /// acknowledged is a notification it is entitled to drop. The server used
    /// to send the create and the `begin` back to back — Shockwave's round-7
    /// capture has them 0.1 ms apart with no response in between, and
    /// [`Self::main_loop`] discarded every `Message::Response`, so the answer
    /// was never read at all. Against VS Code that works (its client responds,
    /// and tolerates an early `$/progress`); against a strict client the
    /// `begin` is dropped and the token announces nothing, which defeats the
    /// point of having one.
    ///
    /// `name` is a *kind* (`luabox/reload`), not an identity: the token that
    /// goes on the wire is `name` plus the next value of
    /// [`Self::progress_seq`], so the third reload of a session announces
    /// itself as `luabox/reload-2` and its `create` request carries the
    /// matching id. Both must be unique — see [`Self::progress_seq`] — and
    /// deriving them from one counter is what keeps them in step.
    /// `None` when there is no usable token: the request could not be sent, or
    /// the client answered it with an **error**. A `$/progress` under a token
    /// the client refused is exactly the traffic this mechanism exists to
    /// stop, and the refusal used to be invisible — `await_progress_create`
    /// matched on the response id and never looked at `response.error`, so a
    /// client replying `-32601` still received a `begin`, five `report`s and
    /// an `end` under a token it had just declined (Shockwave round 8).
    /// Refusal is now the `progress: false` path exactly: nothing goes out.
    ///
    /// Two things make this `None` **before anything is sent**, so the client
    /// sees no request at all:
    ///
    /// - **the session is ending** ([`Self::session_is_ending`]). A client
    ///   that has sent `shutdown` is not going to render a progress bar, and
    ///   the request would open a window whose only effect is to swallow the
    ///   `exit` that follows — which is exactly the round-9 defect;
    /// - **a previous create went unanswered** ([`Self::create_unanswered`]).
    ///   Wave 19 skipped the wait but still sent the create, which is how a
    ///   late refusal ended up reported under. There is nothing to poll for
    ///   if nothing was asked.
    ///
    /// A client that answers *nothing* to the first create keeps the old
    /// behaviour for that one token — see [`Self::await_progress_create`].
    fn create_progress_token(&self, name: &str) -> Option<ProgressToken> {
        if self.session_is_ending() || self.create_unanswered.get() {
            return None;
        }
        let seq = self.progress_seq.get();
        self.progress_seq.set(seq.wrapping_add(1));
        let unique = format!("{name}-{seq}");
        let token = ProgressToken::String(unique.clone());
        let id = RequestId::from(format!("{unique}-progress"));
        let create = WorkDoneProgressCreateParams {
            token: token.clone(),
        };
        let value = serde_json::to_value(create).ok()?;
        self.connection
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                WorkDoneProgressCreate::METHOD.to_string(),
                value,
            )))
            .ok()?;
        match self.await_progress_create(&id) {
            CreateOutcome::Accepted | CreateOutcome::Unanswered => Some(token),
            CreateOutcome::Refused | CreateOutcome::Ending => None,
        }
    }

    /// Whether the client has asked to end the session — see
    /// [`Self::shutting_down`].
    ///
    /// Two ways to know, and both are consulted. The flag is the record of a
    /// session-ender this server has already drained, and it is what stops
    /// the *next* create window opening. The scan of [`Self::pending`] is
    /// belt and braces: a session-ender sitting on the queue means the loop
    /// is about to handle it whatever else happens, so opening a window and
    /// reading the channel first can only get in the way. It also keeps the
    /// answer right if a session-ender ever reaches the queue by a route the
    /// flag does not cover.
    ///
    /// Cheap: the queue holds what a client said during one create window,
    /// which is a handful of messages, and this is asked once per token.
    fn session_is_ending(&self) -> bool {
        if self.shutting_down.get() {
            return true;
        }
        if self.pending.borrow().iter().any(ends_the_session) {
            self.shutting_down.set(true);
            return true;
        }
        false
    }

    /// Read from the connection until the client answers `id`, buffering
    /// everything else in [`Self::pending`] for [`Self::main_loop`].
    ///
    /// Buffering is the whole difficulty. This runs *before* the main loop on
    /// the startup path (`Server::new` announces the rock harvest) and *on* it
    /// for a config reload, and in both windows the client is free to speak —
    /// `initialized`, a `didOpen` for the file the editor restored, a
    /// `didChangeConfiguration`. Those cannot be dropped, so they go on a
    /// queue the loop drains first. Responses other than the one being waited
    /// for are discarded, which is what the loop does with every response
    /// anyway: this server tracks no outstanding requests of its own.
    ///
    /// The wait is bounded by [`PROGRESS_CREATE_TIMEOUT`]. A client that never
    /// answers gets the old behaviour — the `begin` goes out unacknowledged —
    /// rather than a server that stops serving it, and after the first such
    /// timeout it is not waited for again ([`Self::create_unanswered`]).
    ///
    /// # Shutdown ends the wait immediately
    ///
    /// `shutdown` and `exit` are the two messages that must not sit on the
    /// queue. [`Connection::handle_shutdown`] answers the request and then
    /// reads the *channel* for the `exit` that follows, and it cannot see
    /// [`Self::pending`] — so an `exit` drained in here was invisible to it:
    /// the server answered the shutdown, waited 30 s for a notification it was
    /// already holding, and exited 1. A control session without the progress
    /// capability exited 0, which is what makes it a regression; VS Code and
    /// Neovim surface it as abnormal termination. It reproduced on the reload
    /// path as well as at startup.
    ///
    /// Aborting the wait on either message fixes it without touching
    /// `Connection`'s contract: the message goes on the queue in arrival
    /// order, this returns, and [`Self::main_loop`] drains it into the
    /// [handshake](Self::shutdown_handshake) — which looks in the queue as
    /// well as on the channel. Waiting out the remaining 250 ms for a token
    /// nobody will use is pointless anyway.
    ///
    /// Round 9 found that aborting is not sufficient on its own, because
    /// `run` opens a second window straight afterwards; the abort therefore
    /// also records [`Self::shutting_down`], and returns
    /// [`CreateOutcome::Ending`] rather than `Unanswered` so the caller
    /// reports nothing under a token the client will never see.
    fn await_progress_create(&self, id: &RequestId) -> CreateOutcome {
        // The queue may already hold the `shutdown` a previous window drained
        // — reading the channel ahead of it would take the `exit` too.
        if self.session_is_ending() {
            return CreateOutcome::Ending;
        }
        let deadline = Instant::now() + PROGRESS_CREATE_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.connection.receiver.recv_timeout(left) {
                Ok(Message::Response(response)) if response.id == *id => {
                    // A response is not an acceptance. An error answer means
                    // the client declined the token, and reporting under it
                    // anyway is the traffic this wait exists to prevent.
                    return if response.error.is_some() {
                        CreateOutcome::Refused
                    } else {
                        CreateOutcome::Accepted
                    };
                }
                Ok(Message::Response(_)) => {}
                Ok(other) => {
                    let ends_the_session = ends_the_session(&other);
                    self.pending.borrow_mut().push_back(other);
                    if ends_the_session {
                        self.shutting_down.set(true);
                        return CreateOutcome::Ending;
                    }
                }
                // Timed out, or the client hung up. Either way there is
                // nothing left to wait for; the caller reports regardless and
                // the loop notices the disconnect on its next read.
                Err(_) => {
                    self.create_unanswered.set(true);
                    return CreateOutcome::Unanswered;
                }
            }
        }
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
        loop {
            // Anything [`Self::await_progress_create`] took off the wire while
            // it waited comes first, in arrival order — a `didOpen` sent
            // during the startup harvest must be handled, and must be handled
            // before whatever the client says next.
            let queued = self.pending.borrow_mut().pop_front();
            let msg = match queued {
                Some(msg) => msg,
                None => match self.connection.receiver.recv() {
                    Ok(msg) => msg,
                    Err(_) => return Ok(()),
                },
            };
            match msg {
                Message::Request(req) => {
                    if self.shutdown_handshake(&req)? {
                        return Ok(());
                    }
                    self.handle_request(req)?;
                }
                Message::Notification(not) => self.handle_notification(not)?,
                // The server sends two kinds of request, and reads the
                // response to one of them. `window/workDoneProgress/create`
                // is read by `await_progress_create`, where the answer
                // decides whether the token may be used. `client/registerCapability`
                // — the file-watcher registration in `register_file_watchers`
                // — is fire-and-forget, and its response lands here and is
                // dropped.
                //
                // That is a real gap, stated rather than papered over: a
                // client that *rejects* the watcher registration leaves this
                // server believing file watching is live, so external edits
                // go unnoticed until the buffer is touched. Nothing currently
                // detects it. Closing it means tracking the id and degrading
                // on an error answer (polling, or a log message telling the
                // user why the editor is stale), which is a change of
                // behaviour rather than a comment fix and is not in this
                // round.
                Message::Response(_) => {}
            }
        }
    }

    /// Answer a `shutdown` request and wait for the `exit` that must follow
    /// it, looking in [`Self::pending`] **before** the channel. `false` for
    /// any other request, which the caller then dispatches normally.
    ///
    /// This is [`Connection::handle_shutdown`] with two differences, and it
    /// replaces it rather than wrapping it because neither can be added from
    /// outside:
    ///
    /// - **it can see the queue.** `Connection` reads the channel and nothing
    ///   else, which is the whole round-8 defect: an `exit` a create window
    ///   drained was invisible to it, and it waited out its own 30 s bound
    ///   for a notification the server was already holding. The sticky
    ///   [`Self::shutting_down`] flag means no window opens after the first
    ///   session-ender, so in practice the `exit` is still on the channel —
    ///   but "in practice" is how the round-8 fix was argued, and this makes
    ///   it structural instead;
    /// - **a repeated `shutdown` is answered, not fatal.** `Connection`
    ///   returns a protocol error for any non-`exit` message, so a client
    ///   that sends `shutdown` twice — which some editors do when a window
    ///   close races a user quit — turned a clean session into exit 1. Every
    ///   request this server receives gets a response, and there is no reason
    ///   for the last one to be the exception. Anything else during the
    ///   handshake is still a protocol error, with `Connection`'s wording.
    ///
    /// The bound is [`SHUTDOWN_EXIT_TIMEOUT`], matching what `Connection`
    /// allowed, so a client that answers the request and then vanishes fails
    /// exactly as it used to.
    fn shutdown_handshake(&self, req: &Request) -> anyhow::Result<bool> {
        if req.method != Shutdown::METHOD {
            return Ok(false);
        }
        // Set before the response goes out: from here the session is ending
        // whatever else arrives, and nothing may open another create window.
        self.shutting_down.set(true);
        self.connection
            .sender
            .send(Message::Response(Response::new_ok(req.id.clone(), ())))?;
        let deadline = Instant::now() + SHUTDOWN_EXIT_TIMEOUT;
        loop {
            let queued = self.pending.borrow_mut().pop_front();
            let message = if let Some(message) = queued {
                message
            } else {
                let left = deadline.saturating_duration_since(Instant::now());
                self.connection.receiver.recv_timeout(left).map_err(|err| {
                    anyhow::anyhow!("no `exit` notification after `shutdown`: {err}")
                })?
            };
            match message {
                Message::Notification(not) if not.method == Exit::METHOD => return Ok(true),
                Message::Request(repeat) if repeat.method == Shutdown::METHOD => {
                    self.connection
                        .sender
                        .send(Message::Response(Response::new_ok(repeat.id, ())))?;
                }
                other => anyhow::bail!("unexpected message during shutdown: {other:?}"),
            }
        }
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
        let (snapshot, sema, offset) = self.at(uri, position)?;
        let exports = self.require_exports(&snapshot, &sema.path);
        let ambient = self.merged_ambient(&snapshot);
        hover::hover(&sema, offset, &exports, &ambient, &snapshot)
    }

    /// The shared `require` resolution for one file — the same map the
    /// diagnostics pipeline checks against (#54), so hover and completion
    /// cannot type a `require` binding differently from the problems pane.
    fn require_exports(&self, snapshot: &Analysis, path: &Path) -> RequireExports {
        RequireExports::resolve(snapshot, path, &self.rocks)
    }

    /// The merged ambient layer every editor surface reads — defs, then the
    /// workspace-global project types, then the rock tree — so diagnostics,
    /// hover, completion, signature help and goto-definition all resolve
    /// against the very environment `luabox check` enforces (#56). Cached on
    /// [`Analysis::revision`]: a request against an unchanged world reuses
    /// the merge instead of re-cloning the defs surface and re-merging every
    /// project file (see the field doc on [`Self::merged_ambient`] for what
    /// this reuse does and does not reach, measured).
    ///
    /// Traced via [`Self::log_message`] at [`MessageType::LOG`] on the
    /// **rebuild** arm only (#60, #58's `#[cfg(test)]`-only trace was proven
    /// indistinguishable from a stale merge in a release build, R26 — that
    /// requirement stands unchanged: a rebuild must stay externally
    /// observable in a release build). R26 also logged the *hit* arm, on the
    /// reasoning that clients filter `LOG` out of their visible pane by
    /// default — an assumption about client behaviour the LSP spec does not
    /// make, and several real clients surface every `window/logMessage` line
    /// in an always-on output channel regardless of severity. A hit fires on
    /// every hover, completion, signature-help, goto-definition and
    /// diagnostics publish — once per request for the life of the session,
    /// not once per revision — so logging it unconditionally is unbounded
    /// chatter.
    ///
    /// **The rebuild arm is not rare either (N22).** The doc that shipped
    /// with this trace argued "a rebuild happens once per revision, not once
    /// per request, so volume collapses" — true in general, but every
    /// `textDocument/didChange` bumps the revision (this function's own
    /// paragraph above), so on the *editing* path "once per revision" is
    /// "once per keystroke": measured, 100 edits produced 101 log messages.
    /// The same "several real clients surface every `window/logMessage` line
    /// in an always-on output channel" argument R26 used to drop the *hit*
    /// arm applies unchanged to a rebuild log that fires that often.
    ///
    /// Gated on [`Self::trace`] (N22), not unconditional: `initialize`'s
    /// `trace` field (spec default `Off`, updated live via `$/setTrace`) is
    /// the protocol's own mechanism for exactly this kind of optional
    /// execution trace — not a bespoke capability or config flag (the
    /// alternative this doc used to reject "a negotiation path and a silent
    /// default for a signal nobody asked to opt into" — `trace` already *is*
    /// that negotiation, standardised, and a client that never sets it now
    /// receives zero merged-ambient log lines, addressing the volume
    /// complaint without touching R26's guarantee: a client that *does* ask
    /// for `messages`/`verbose` sees a stale merge the same way it always
    /// did — a missing `rebuilt@` line where an edit should have produced
    /// one. What is lost, same as before: a client can no longer tell
    /// "second read at this revision, cache hit" apart from "second read at
    /// this revision, the server was never asked again" — both produce zero
    /// additional log lines, and nothing downstream of this trace ever
    /// needed that distinction (the cache-hit test below asserts the
    /// *absence* of a second `rebuilt@`, not the presence of a `hit@`, for
    /// exactly this reason). `revision` stays on the rebuild line so a
    /// client's log pane can tell two rebuilds at different revisions apart
    /// from a rebuild firing twice at the same one — a bug, since the cache
    /// key is the revision.
    ///
    /// `LOG`, not `WARNING`/`INFO`: `WARNING`, used elsewhere in this file
    /// (manifest problems, malformed messages), is for something the user
    /// should notice unprompted; a cache rebuild is not that. `LOG` is the
    /// tier LSP reserves for exactly this volume — clients that do filter it
    /// stay silent, the same tier
    /// [`luabox_db::AnalysisHost::take_execution_log`] answers for salsa's
    /// own cache.
    fn merged_ambient(&self, snapshot: &Analysis) -> Rc<MergedAmbient> {
        let revision = snapshot.revision();
        if let Some((cached_at, merged)) = self.merged_ambient.borrow().as_ref()
            && *cached_at == revision
        {
            return Rc::clone(merged);
        }
        let merged = Rc::new(
            MergedAmbient::build(&self.ambient, &snapshot.project_types(), self.rocks.types())
                .with_ambient_paths(self.ambient_paths.clone())
                .with_ambient_alias_names(self.ambient_alias_names.clone()),
        );
        // N22: gated on the negotiated trace level, not unconditional — see
        // the `trace` field doc. A keystroke bumps the revision every time
        // (this function's own doc), so an ungated log here is a
        // `window/logMessage` per keystroke for the life of the session, for
        // every client, with no way to turn it off; `trace` defaults to
        // `Off`, so the overwhelming majority of sessions now see none of it,
        // while a client that asks for `messages`/`verbose` still gets
        // exactly the observability R26 established.
        if self.trace.get() != TraceValue::Off {
            self.log_message(
                MessageType::LOG,
                format!("merged ambient rebuilt@{revision}"),
            );
        }
        *self.merged_ambient.borrow_mut() = Some((revision, Rc::clone(&merged)));
        merged
    }

    /// The workspace's declared-`---@class` cycle pass at `snapshot`'s
    /// revision, computed at most once per revision — see
    /// [`Self::class_cycles`]' field doc.
    ///
    /// Deliberately unlogged, unlike [`Self::merged_ambient`]'s rebuild: that
    /// log exists because the merge's cache key cannot see its own base
    /// layers moving (N22/#59), and this one's can.
    fn cycle_pass(&self, snapshot: &Analysis) -> Rc<diagnostics::ClassCycles> {
        let revision = snapshot.revision();
        if let Some((cached_at, cycles)) = self.class_cycles.borrow().as_ref()
            && *cached_at == revision
        {
            return Rc::clone(cycles);
        }
        let cycles = Rc::new(diagnostics::class_cycle_diagnostics(
            snapshot,
            self.strictness,
        ));
        *self.class_cycles.borrow_mut() = Some((revision, Rc::clone(&cycles)));
        cycles
    }

    /// The callee's resolved signature(s) while `position` sits inside a
    /// call's argument list (see [`crate::signature_help`]).
    fn signature_help(&self, uri: &Uri, position: lsp_types::Position) -> Option<SignatureHelp> {
        let (snapshot, sema, offset) = self.at(uri, position)?;
        let exports = self.require_exports(&snapshot, &sema.path);
        let ambient = self.merged_ambient(&snapshot);
        signature_help::signature_help(&sema, offset, &exports, &ambient, &snapshot)
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
        let (snapshot, sema, offset) = self.at(uri, position)?;
        let exports = self.require_exports(&snapshot, &sema.path);
        let ambient = self.merged_ambient(&snapshot);
        goto_definition::definition(
            &sema,
            offset,
            &self.root,
            self.dialect,
            &snapshot,
            &exports,
            &ambient,
        )
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
        let exports = self.require_exports(&snapshot, &sema.path);
        let ambient = self.merged_ambient(&snapshot);
        Some(completion::completion(
            &sema, offset, &snapshot, &self.root, &exports, &ambient,
        ))
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
                // `convert` derives the source from the code, so this site
                // cannot hardcode `LINT_SOURCE` any more — it did, until round
                // 5. The client pairs an action with a published diagnostic by
                // matching the whole record, source included, so a future
                // fix-carrying diagnostic outside the lint band is
                // re-converted under the source it was published with, and
                // there is no argument here to get wrong.
                diagnostics: source_diag.map(|d| vec![diagnostics::convert(index, d)]),
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
        let merged = self.merged_ambient(&snapshot);
        let cycles = self.cycle_pass(&snapshot);
        let ctx = diagnostics::CheckCtx {
            strictness: self.strictness,
            ambient: &merged,
            rocks: &self.rocks,
            lint: &self.lint,
            known_globals: &self.known_globals,
            cycles: &cycles,
        };
        // `own` only: a quick fix is matched against ranges in *this*
        // document, so a diagnostic whose span belongs to another file has
        // nothing here to attach to (production readiness review, finding 4).
        let type_diags = diagnostics::diagnostics(&snapshot, &sema.path, self.dialect, &ctx)
            .map(|found| found.own)
            .unwrap_or_default();
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
                let Some(current) = self.base_text_for_change(&path, &params.content_changes)
                else {
                    return Ok(());
                };
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
            SetTrace::METHOD => {
                // The client's live opt-in/opt-out of trace-level chatter
                // (N22) — `initialize`'s `trace` field is only the starting
                // value; `$/setTrace` is how a client turns tracing on
                // mid-session (e.g. a "Report LSP issue" flow) or back off.
                if let Some(params) = self.notification_params::<SetTrace>(not.params) {
                    self.trace.set(params.value);
                }
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
    /// deleted `.lua` file's text is cleared to empty rather than left as-is
    /// (N24) — everything it exported or declared must stop resolving for
    /// every other file that referenced it, which leaving the last-known
    /// text in place would not do.
    ///
    /// Diagnostics for a changed file that is **not** shadowed by an editor
    /// overlay are published for its own URI directly — an open buffer's
    /// come from the overlay, so re-reading disk changes nothing visible
    /// there. On top of that, any batch touching a `.lua` file at all —
    /// created, changed or deleted — republishes every *open* document once,
    /// after the loop, because the workspace surface those documents were
    /// last checked against has moved. A manifest change republishes them
    /// the same way via [`Self::reload_config`], which subsumes it.
    ///
    /// The sentence this replaces claimed the open files were the ones
    /// republished and the others were not, which was the inverse of the
    /// code on both counts (production readiness review, finding 2).
    fn watched_files_changed(
        &mut self,
        params: &DidChangeWatchedFilesParams,
    ) -> anyhow::Result<()> {
        let mut reload = false;
        let mut republish_open = false;
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
                // N24: the file is gone from disk, but the host still holds
                // its last-known text (and everything derived from it —
                // exports, class declarations) until told otherwise, so a
                // stale `require` target keeps resolving and hover keeps
                // answering from content that no longer exists. There is no
                // `Change::RemoveFile` (`luabox_db::Change` has no delete
                // variant) — clearing the text to empty is the closest
                // in-repo equivalent: an empty file exports nothing and
                // declares nothing, so every surface that read something out
                // of it starts reading nothing, the same externally
                // observable effect a real removal would have. Guarded by
                // `open_docs` the same way the write-then-publish arm below
                // is: an editor overlay still shadows disk, so a buffer left
                // open after its backing file was deleted keeps showing what
                // the user is looking at rather than being blanked out from
                // under them.
                self.host.apply_change(Change::SetFileText {
                    path: path.clone(),
                    dialect: self.dialect,
                    text: String::new(),
                });
                if !self.open_docs.contains_key(&path) {
                    self.publish_lua(&event.uri, &path)?;
                }
                // M58 (round 6 review): the deleted file's export/class
                // surface just vanished from the merged workspace ambient —
                // every already-*open* document whose diagnostics depended
                // on it (a `require` of the deleted module, a class
                // inherited from it, ...) must be re-checked against the
                // new surface, the same way a manifest reload already
                // re-checks every open document
                // (`reload_config`/`republish_open_docs`). Resolution itself
                // is correct the instant the change above lands (hover
                // reads a fresh `Analysis` per request), but nothing
                // re-published a *diagnostics* notification for anyone but
                // the deleted file's own URI — an unrelated open consumer's
                // Problems panel stayed clean until its own next keystroke
                // re-triggered `set_text`'s publish, disagreeing with hover
                // about the same instant in the meantime.
                //
                // Flagged, not called here: this arm runs once per deleted
                // file in the batch, but `republish_open_docs` re-checks
                // every open document from scratch (fresh `Analysis`,
                // `publish_lua` per file) — calling it per event turns a
                // batch of D deletes against O open documents into D×O full
                // diagnostics passes. Set the flag and act once after the
                // loop, same idiom as `reload`/`reload_config` below.
                republish_open = true;
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
                // The CREATED/CHANGED counterpart of M58's deleted-file
                // republish (production readiness review, finding 2).
                // Deletion is not the only event that moves the merged
                // workspace ambient out from under an already-open consumer:
                // a `git checkout` *restoring* a dependency, or an external
                // edit adding the `---@field` an open buffer was being
                // flagged for missing, arrives here as CREATED or CHANGED.
                // The publish above covers the changed file's own URI and
                // nothing else, so before this an open consumer kept a stale
                // `LB0306` until its own next keystroke — M58's exact
                // asymmetry, in the arm M58 did not touch.
                //
                // Flagged, acted on once after the loop, same idiom as the
                // DELETED arm and `reload`: `republish_open_docs` re-checks
                // every open document from scratch, so calling it per event
                // turns a C-file batch against O open documents into C×O
                // full diagnostics passes (H1).
                republish_open = true;
            }
        }
        if reload {
            // Subsumes `republish_open_docs` (see its call at the end of
            // `reload_config`) — running both would publish every open
            // document's diagnostics twice for a batch that both deletes a
            // `.lua` file and touches `luabox.toml`.
            self.reload_config()?;
        } else if republish_open {
            self.republish_open_docs()?;
        }
        Ok(())
    }

    /// The text a `didChange` batch applies to, or `None` when the batch must
    /// be dropped.
    ///
    /// `didChange` carries *edits*, not state (#78). A ranged change means
    /// nothing without the buffer its range indexes into, and only an open
    /// document has one: the client's ranges address the text it last synced,
    /// which is the `didOpen` text plus every `didChange` since.
    ///
    /// "Does the host hold text?" is the wrong question to gate on.
    /// `file_text` answers "overlay when set, disk otherwise", and
    /// `bootstrap` indexes every `.lua` file under the root — so it says
    /// `Some` for any indexed file, whose *disk* text no client ever agreed
    /// to. Splicing a range into that, or into `""` as the
    /// `unwrap_or_default()` this replaced did, invents a document: the
    /// result becomes the overlay and gets diagnostics published for it,
    /// painting the problems pane of a file the client never opened with
    /// errors for code that exists in no buffer. For an indexed file it also
    /// outlives the edit — the overlay shadows disk, and with no `didOpen`
    /// there is no `didClose` to drop it.
    ///
    /// A batch whose *first* change is a full replace (`range: None`) needs no
    /// base text: it supplies the whole document, and any later ranged edit in
    /// the batch indexes into what that replace established. So the gate is on
    /// the first change alone, not on "the batch contains a range": a batch is
    /// accepted iff its first change is a full replace, and a batch that
    /// begins ranged is dropped even if a later change in it is one — one
    /// rule for an off-spec client, and a batch whose first change is already
    /// ranged is a client that has already lost sync, not one to be trusted
    /// to have found it again mid-batch.
    fn base_text_for_change(
        &self,
        path: &Path,
        changes: &[TextDocumentContentChangeEvent],
    ) -> Option<String> {
        if self.open_docs.contains_key(path)
            && let Some(text) = self.host.snapshot().file_text(path)
        {
            return Some(text);
        }
        if changes.is_empty() {
            // An empty batch has nothing to warn about: it is a legal no-op,
            // not an edit with no base text, so it must stay silent.
            return None;
        }
        if changes.first().is_some_and(|change| change.range.is_none()) {
            return Some(String::new());
        }
        self.log_message(
            MessageType::WARNING,
            format!(
                "ignoring `{}` for {}: the server holds no buffer for it (no \
                 `textDocument/didOpen`), so an incremental edit has nothing \
                 to apply to",
                DidChangeTextDocument::METHOD,
                path.display()
            ),
        );
        None
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
            // A buffer with no disk backing: it stops contributing the
            // instant it closes, so this goes through the ledger rather than
            // publishing an empty set straight to the wire (F4). An untracked
            // clear here would leave a cycle it had painted onto a real
            // project file's panel with nothing left to ever remove it.
            self.host
                .apply_change(Change::ClearOverlay { path: path.clone() });
            // And the workspace half is recomputed, not skipped (round 13
            // review R13-A). The overlay that just went away was carrying
            // this file's `---@class` declarations, so a cycle that only
            // closed through them is now genuinely gone — but the cycle set
            // is workspace-derived and lives in ONE ledger slot, so "not
            // recomputed" left the last answer standing on every other
            // member's document. Measured sequence: `a.lua` and
            // `b.lua` declare a mutual cycle and are both open, `a.lua` is
            // deleted on disk (the watched-DELETE arm blanks the disk text
            // but the overlay still shadows it, so the cycle survives that
            // pass), the user closes the orphaned tab — `read_to_string`
            // fails, the overlay drops, and `b.lua` kept an `LB0318` naming
            // a class nothing declares any more until it was itself touched.
            // A false error on a clean open file, and the same "painted with
            // nothing to remove it" class F4 closed for the other two slots.
            //
            // Recomputing is the honest answer, not a clear: it runs the
            // same workspace pass over the post-`ClearOverlay` snapshot, so
            // every cycle that survives this close survives in the ledger.
            let workspace = self.workspace_findings();
            self.publish_recorded(
                uri,
                &path,
                Vec::new(),
                BTreeMap::new(),
                WorkspaceScan::Recomputed(workspace),
            )
        }
    }

    /// The workspace-derived findings as of the current host state, with no
    /// file in flight — [`diagnostics::workspace_diagnostics`] over a fresh
    /// snapshot.
    ///
    /// Routes [`Self::cycle_pass`]' answer and nothing else, for the one
    /// caller that must leave the ledger's workspace slot correct without
    /// having a file to check ([`Self::close`]). The `ClearOverlay` that
    /// precedes it has just moved the revision, so this genuinely derives a
    /// new pass rather than reusing the cached one — which is the point.
    fn workspace_findings(&self) -> BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>> {
        let analysis: Analysis = self.host.snapshot();
        let cycles = self.cycle_pass(&analysis);
        diagnostics::workspace_diagnostics(&analysis, &cycles)
    }

    /// Publish the current diagnostics for one `.lua` file from a fresh
    /// snapshot.
    ///
    /// Checking one file can produce a diagnostic that belongs to **another**
    /// — the cross-file `---@class` ancestry pair `LB0317`/`LB0318` points at
    /// the offending class's declaration, wherever that lives
    /// ([`diagnostics::FileDiagnostics`]). Each such group is published under
    /// its own document URI, at its own file's line numbers, rather than
    /// rendered inside this document at an offset its text cannot carry
    /// (production readiness review, finding 4).
    ///
    /// A `publishDiagnostics` notification *replaces* the whole set for its
    /// URI, so every publish here goes through [`ForeignLedger`], which is
    /// the record of what each document should be showing — see its doc for
    /// the four failures the un-recorded version produced.
    ///
    /// # Exactly one diagnostics pass per publish (round 8 review, F19)
    ///
    /// There is one `diagnostics::diagnostics` call in this function and it
    /// is not in a loop, so a publish costs one whole-file pass (parse +
    /// validate + type-check + lint) no matter how many URIs it ends up
    /// notifying. The version this replaces ran a **further** full pass per
    /// foreign target, per publish — i.e. per keystroke, single-threaded, no
    /// debounce — purely to recover that target's own half, which the ledger
    /// now already holds.
    ///
    /// What that costs, stated rather than hidden: a target's own half is the
    /// one from the last pass over *that* file, so a document nobody has
    /// opened shows the cross-file group alone until it is itself checked
    /// (opening it, editing it, or a watched-file event for it — each of
    /// which publishes it in full and merges the groups back in). That is
    /// strictly what the client is already displaying for that URI plus the
    /// group being added: re-stating a file's own half from the ledger can
    /// never contradict the panel, because the ledger *is* what was put in
    /// the panel. The alternative — re-deriving every foreign target's whole
    /// file on every keystroke of every open buffer — is the O(open ×
    /// foreign) stall F19 measures, paid to refresh diagnostics for
    /// documents the user never asked about.
    fn publish_lua(&mut self, uri: &Uri, path: &Path) -> anyhow::Result<()> {
        let analysis: Analysis = self.host.snapshot();
        let found = {
            let merged = self.merged_ambient(&analysis);
            let cycles = self.cycle_pass(&analysis);
            let ctx = diagnostics::CheckCtx {
                strictness: self.strictness,
                ambient: &merged,
                rocks: &self.rocks,
                lint: &self.lint,
                known_globals: &self.known_globals,
                cycles: &cycles,
            };
            diagnostics::diagnostics(&analysis, path, self.dialect, &ctx)
        };
        // No pass at all (a path the analysis does not know) publishes an
        // empty own half — but must NOT publish an empty *workspace* half:
        // that map is the whole answer, not this file's share of it, and a
        // default-constructed one would read as "the workspace has no
        // cycles" and wipe every one of them off every document. That is
        // what `WorkspaceScan::Skipped` says, in the one place it is true.
        let (own, foreign, workspace) = match found {
            Some(found) => (
                found.own,
                found.foreign,
                WorkspaceScan::Recomputed(found.workspace),
            ),
            None => (Vec::new(), BTreeMap::new(), WorkspaceScan::Skipped),
        };
        self.publish_recorded(uri, path, own, foreign, workspace)
    }

    /// Fold one pass's result into the ledger and publish every document it
    /// moved — the checked file itself included, and in sorted path order so
    /// a batch is reproducible.
    ///
    /// `uri` is used verbatim for `path`'s own notification rather than
    /// re-derived, so a document keeps the exact URI spelling its client
    /// opened it under; the other affected paths have no client-supplied URI
    /// to preserve.
    fn publish_recorded(
        &mut self,
        uri: &Uri,
        path: &Path,
        own: Vec<lsp_types::Diagnostic>,
        foreign: BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
        workspace: WorkspaceScan,
    ) -> anyhow::Result<()> {
        for target in self.foreign.record(path, own, foreign, workspace) {
            let set = self.foreign.published_set(&target);
            if target == path {
                self.publish(uri, set)?;
            } else {
                self.publish(&crate::uri::path_to_uri(&target), set)?;
            }
        }
        Ok(())
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
    use std::fmt::Write as _;
    use std::fs;
    use std::path::{Path, PathBuf};

    use lsp_server::{Connection, Message, Notification, Request, RequestId};
    use lsp_types::notification::{
        DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
        DidOpenTextDocument, Notification as _,
    };
    use lsp_types::request::{HoverRequest, Request as _};
    use lsp_types::{MessageType, Position, Range, TextDocumentContentChangeEvent, TraceValue};
    use luabox_lint::LintConfig;
    use luabox_manifest::model::{Lint, LintLevel, LintTier, Manifest};
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use std::collections::BTreeMap;

    use super::{
        CodeActionOrCommand, ErrorCode, ForeignLedger, ProjectConfig, PublishDiagnostics, Rc,
        Server, WorkspaceScan, ambient_def_sources, apply_content_changes, root_path,
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

    // === Merged ambient cache (#57–#60) ====================================
    //
    // What the cache measurably delivers (#57, correcting the prior comment's
    // false "diagnostics on each keystroke" claim): the SAME revision's
    // second read reuses the merge — read here through the *absence* of a
    // second `rebuilt@` `window/logMessage` line (#58/#60's observability,
    // made real in a release build by R26 — no `#[cfg(test)]` gate, so
    // `merged_ambient_log` reads the protocol the server actually sent, not
    // an in-process vec only tests can see), not by inference from timing or
    // a deleted cache still passing the suite green. R26 also logged the hit
    // arm directly as a `hit@` line; a later pass (finding 1, volume flagged
    // independently by three review passes) dropped it — a hit fires once
    // per request for the life of a session, unconditionally, and several
    // real `window/logMessage` panes surface every line regardless of
    // severity. `merged_ambient`'s own doc carries the full trade; what's
    // pinned here is what survives it.

    /// The `merged ambient rebuilt@` `window/logMessage` payloads among
    /// `messages`, in order — the same signal [`super::Server::merged_ambient`]
    /// sends a real client, so a test reading it is reading exactly what
    /// production would log, not a parallel test-only channel (R26). No
    /// `hit@` payload exists to read (finding 1: the hit arm is not logged),
    /// so this list is exactly the server's rebuild history at this
    /// revision-key granularity.
    fn merged_ambient_log(messages: &[Message]) -> Vec<String> {
        log_messages(messages)
            .into_iter()
            .filter(|m| m.starts_with("merged ambient "))
            .collect()
    }

    /// Open one document and hover it twice with no edit in between: the
    /// `didOpen` publish is the first read at this revision (a rebuild), the
    /// hover right after is the second. Only the rebuild arm is logged
    /// (finding 1), so the cross-surface reuse the cache actually delivers
    /// shows up as the *absence* of a second `rebuilt@` line, not a `hit@`
    /// one — the only way the wire now exposes it.
    #[test]
    fn a_second_read_at_the_same_revision_does_not_rebuild_again() {
        let (dir, mut server, client) = test_server_with_trace(TraceValue::Messages);
        let path = dir.path().join("main.lua");
        let source = "local x = 1\nprint(x)\n";
        fs::write(&path, source).expect("write the document");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": source,
                    },
                }),
            })
            .expect("didOpen");
        // The `didOpen` publish is itself a read at this revision (the
        // rebuild) — captured, not discarded, since the log is now the real
        // `window/logMessage` protocol and a dropped drain is a dropped
        // message, not just a cleared in-process buffer (R26).
        let mut trace = merged_ambient_log(&drain(&client));

        server.hover(&uri, lsp_types::Position::new(1, 6));
        trace.extend(merged_ambient_log(&drain(&client)));

        assert_eq!(
            trace.iter().filter(|e| e.contains("rebuilt@")).count(),
            1,
            "a second read at the same revision must reuse the merge, not \
             rebuild it again: {trace:?}"
        );
    }

    /// N22: a client that never sets `trace` (the spec default, and what
    /// `test_server`/`test_server_with` build) sees **no** "merged ambient
    /// rebuilt@" chatter at all, even across several edits that each
    /// guarantee a rebuild — the volume complaint's fix, measured the same
    /// way the complaint itself was: counting `window/logMessage` traffic
    /// across a run of edits.
    #[test]
    fn a_client_that_never_sets_trace_receives_no_merged_ambient_log() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("main.lua");
        fs::write(&path, "local x = 1\n").expect("write the document");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": "local x = 1\n",
                    },
                }),
            })
            .expect("didOpen");
        let mut trace = merged_ambient_log(&drain(&client));
        for version in 2..=4 {
            server
                .handle_notification(Notification {
                    method: DidChangeTextDocument::METHOD.to_string(),
                    params: json!({
                        "textDocument": { "uri": uri.to_string(), "version": version },
                        "contentChanges": [{ "text": format!("local x = {version}\n") }],
                    }),
                })
                .expect("didChange");
            trace.extend(merged_ambient_log(&drain(&client)));
        }
        assert!(trace.is_empty(), "{trace:?}");
    }

    /// N22: `$/setTrace` turns the log on mid-session — a client is not
    /// limited to whatever `initialize`'s `trace` field said at startup.
    #[test]
    fn a_set_trace_notification_turns_the_merged_ambient_log_on_live() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("main.lua");
        fs::write(&path, "local x = 1\n").expect("write the document");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": "local x = 1\n",
                    },
                }),
            })
            .expect("didOpen");
        // Silent before the client asks for tracing.
        assert!(merged_ambient_log(&drain(&client)).is_empty());

        server
            .handle_notification(Notification {
                method: "$/setTrace".to_string(),
                params: json!({ "value": "messages" }),
            })
            .expect("setTrace");
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [{ "text": "local x = 2\n" }],
                }),
            })
            .expect("didChange");
        let trace = merged_ambient_log(&drain(&client));
        assert_eq!(
            trace.iter().filter(|e| e.contains("rebuilt@")).count(),
            1,
            "{trace:?}"
        );
    }

    /// A keystroke bumps the revision (`host.rs`) immediately before
    /// `publish_lua` reads this cache, so the very next read is a guaranteed
    /// miss — measured, not assumed: two edits in a row rebuild twice, never
    /// reuse a merge from before either edit.
    #[test]
    fn a_keystroke_always_rebuilds_never_hits() {
        let (dir, mut server, client) = test_server_with_trace(TraceValue::Messages);
        let path = dir.path().join("main.lua");
        fs::write(&path, "local x = 1\n").expect("write the document");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": "local x = 1\n",
                    },
                }),
            })
            .expect("didOpen");
        drop(drain(&client));

        // Each `didChange` republishes diagnostics, which reads the cache
        // (`publish_lua` → `merged_ambient`) — the drain must happen inside
        // the loop, per iteration, or the `window/logMessage` for an earlier
        // change is indistinguishable from a later one once both are mixed
        // into one drain. Each edit's text must genuinely differ from the
        // last (round 4 review R17): `apply_change` now bumps the revision
        // only when a salsa input actually changed, so two edits setting
        // the identical text would make the second a no-op — a legitimate
        // cache *hit* — and this test would stop measuring what its name
        // claims.
        let mut trace = Vec::new();
        for version in 2..=3 {
            server
                .handle_notification(Notification {
                    method: DidChangeTextDocument::METHOD.to_string(),
                    params: json!({
                        "textDocument": { "uri": uri.to_string(), "version": version },
                        "contentChanges": [{ "text": format!("local x = {version}\n") }],
                    }),
                })
                .expect("didChange");
            trace.extend(merged_ambient_log(&drain(&client)));
        }

        // Only the rebuild arm is logged (finding 1) — a `hit@` line cannot
        // appear even if a bug turned one of these into an accidental cache
        // hit, so the assertion the test's name stands on is the rebuild
        // count alone: two edits in, two rebuilds out, none skipped.
        assert_eq!(
            trace.iter().filter(|e| e.contains("rebuilt@")).count(),
            2,
            "{trace:?}"
        );
    }

    /// #59: a reload that changes only `[types] defs` (no strictness change)
    /// still invalidates the merged-ambient cache, because the manual clear
    /// in `reload_config` does not depend on the host's revision moving —
    /// and, per the guard added there, the revision genuinely does not move
    /// in this scenario (`SetStrictness` is skipped when the value is
    /// unchanged), so this is the manual clear's effect in isolation, not
    /// the revision key's.
    #[test]
    fn a_defs_only_reload_invalidates_the_cache_without_bumping_the_revision() {
        let (dir, mut server, client) = test_server_with_trace(TraceValue::Messages);
        let root = dir.path();
        fs::write(root.join("luabox.toml"), MANIFEST_HEAD).expect("write manifest");
        let path = root.join("main.lua");
        let source = "---@type Widget\nlocal w = nil\nprint(w.gadget)\n";
        fs::write(&path, source).expect("write the document");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": source,
                    },
                }),
            })
            .expect("didOpen");
        drop(drain(&client));
        let before = server.host.snapshot().revision();

        // Add a `[types] defs` entry declaring `Widget` and reload — no
        // strictness change. Def files resolve under `<root>/defs/<name>.d.lua`
        // (`layout::resolve_project_defs`), so `defs = ["widget"]`.
        fs::create_dir_all(root.join("defs")).expect("defs dir");
        fs::write(
            root.join("defs/widget.d.lua"),
            "---@meta\n---@class Widget\n---@field gadget number\n",
        )
        .expect("write defs");
        fs::write(
            root.join("luabox.toml"),
            format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"widget\"]\n"),
        )
        .expect("rewrite manifest");
        server.reload_config().expect("reload");
        drop(drain(&client));

        let after = server.host.snapshot().revision();
        assert_eq!(
            before, after,
            "strictness did not change, so neither should the revision"
        );

        let hover = server.hover(&uri, lsp_types::Position::new(2, 10));
        let text = match hover.expect("hover").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        assert!(text.contains("Widget.gadget"), "{text}");
    }

    /// Round 4 review R11: a `[types] defs` declaration of `Widget` collides
    /// with an ordinary project file's own `---@class Widget`. The type
    /// merge (`env.rs`'s `merge_file_types`) seeds the ambient/defs classes
    /// first, so a same-named field on a later-merged project file loses the
    /// *type* collision but not its own `---@field` description — hover must
    /// render the defs field's type with whichever file's description
    /// `locate_field` actually names (here, the same defs file, since it is
    /// visited first), and goto-definition must jump there too, not to the
    /// project file `sema::locate_field` used to prefer by alphabetical
    /// accident.
    #[test]
    fn a_defs_declaration_wins_a_same_name_collision_over_a_project_files_own() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        fs::create_dir_all(root.join("defs")).expect("defs dir");
        fs::write(
            root.join("defs/widget.d.lua"),
            "---@meta\n---@class Widget\n---@field id string the id from defs\n",
        )
        .expect("write defs");
        // An unrelated project file re-declaring the identical class name
        // with a *different* field type and its own description — the
        // collision. Named so it would sort before `defs/widget.d.lua`
        // alphabetically, the exact ordering the previous `sort_unstable()`
        // used and got wrong.
        fs::write(
            root.join("a_widget.lua"),
            "---@class Widget\n---@field id number the id from a_widget\n",
        )
        .expect("write the colliding project file");
        let path = root.join("main.lua");
        let source = "---@type Widget\nlocal w = nil\nprint(w.id)\n";
        fs::write(&path, source).expect("write the document");
        fs::write(
            root.join("luabox.toml"),
            format!("{MANIFEST_HEAD}\n[types]\ndefs = [\"widget\"]\n"),
        )
        .expect("write manifest");
        // `reload_config` picks up `[types] defs` (the ambient/base layer,
        // read straight off disk); `bootstrap` is what actually loads every
        // `.lua` file — `defs/widget.d.lua` included (`DefFiles::Include`)
        // — into the host as a *project* file too, which is what
        // `sema::locate_field`'s cross-file search reads. Both are needed:
        // without `bootstrap`, neither colliding declaration is visible to
        // `locate_field` at all, and the test would pass vacuously with an
        // empty description on either side of the assertion.
        server.reload_config().expect("reload");
        server.bootstrap();
        drop(drain(&client));

        let uri = crate::uri::path_to_uri(&path);
        let hover = server.hover(&uri, lsp_types::Position::new(2, 8));
        let text = match hover.expect("hover").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        // The type is defs' (`string`), not the project file's (`number`).
        assert!(text.contains("Widget.id: string"), "{text}");
        assert!(!text.contains("Widget.id: number"), "{text}");
        // The description is whichever file `locate_field` actually named —
        // the defs file, since it is visited first — not the project file's.
        assert!(text.contains("the id from defs"), "{text}");
        assert!(!text.contains("the id from a_widget"), "{text}");

        // Goto-definition follows the same authority: it must land in the
        // defs file, not the project file whose type lost the collision.
        let location = server
            .definition(&uri, lsp_types::Position::new(2, 8))
            .expect("definition");
        assert!(
            location.uri.as_str().ends_with("defs/widget.d.lua"),
            "{location:?}"
        );
    }

    /// Finding 3: two *ordinary* project files (neither one is `current`,
    /// neither is a `.d.lua`) both declare `---@class Widget`. `sema.rs`'s
    /// `search_order` broke this tie with an alphabetical sort, documented
    /// as "not proven to match `merge_file_types`'s own first-wins order,
    /// which is keyed on `Project::files(db)` insertion order". This pins
    /// that it *does* match, for the file set every real session's `current`
    /// actually collides against — every file `bootstrap` loads at startup:
    /// `Project::files(db)`'s insertion order is exactly
    /// `luabox_manifest::layout::collect_lua_files`'s walk order (sorted by
    /// filename within each directory, always descending fully before the
    /// next sibling), which is the same order `Path`'s own component-wise
    /// `Ord` produces over the full paths — i.e. exactly what
    /// `rest.sort_unstable()` in `search_order` produces. `aa_widget.lua`
    /// sorts before `bb_widget.lua` both ways, so if the ordering actually
    /// disagreed with the merge, this test would show the *type* resolving
    /// to one file while hover's *description* and goto-definition named
    /// the other — the exact split finding 3 warns is possible.
    #[test]
    fn a_first_declared_ordinary_project_file_wins_a_same_name_collision_over_a_later_one() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        fs::write(root.join("luabox.toml"), MANIFEST_HEAD).expect("write manifest");
        fs::write(
            root.join("aa_widget.lua"),
            "---@class Widget\n---@field id string the id from aa_widget\n",
        )
        .expect("write the first-sorted colliding file");
        fs::write(
            root.join("bb_widget.lua"),
            "---@class Widget\n---@field id number the id from bb_widget\n",
        )
        .expect("write the second-sorted colliding file");
        let path = root.join("main.lua");
        let source = "---@type Widget\nlocal w = nil\nprint(w.id)\n";
        fs::write(&path, source).expect("write the document");
        // `bootstrap`, not `didOpen`, so `Project::files(db)` receives every
        // file in `collect_lua_files`'s sorted walk order — the order the
        // proof above depends on. `didOpen`-only files append in open order
        // instead (the gap `sema::locate_field`'s doc names as still open).
        server.bootstrap();
        drop(drain(&client));

        let uri = crate::uri::path_to_uri(&path);
        let hover = server.hover(&uri, lsp_types::Position::new(2, 8));
        let text = match hover.expect("hover").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        // The type merge's own first-wins decision.
        assert!(text.contains("Widget.id: string"), "{text}");
        assert!(!text.contains("Widget.id: number"), "{text}");
        // Navigation must agree with it — same file, same description.
        assert!(text.contains("the id from aa_widget"), "{text}");
        assert!(!text.contains("the id from bb_widget"), "{text}");

        let location = server
            .definition(&uri, lsp_types::Position::new(2, 8))
            .expect("definition");
        assert!(
            location.uri.as_str().ends_with("aa_widget.lua"),
            "{location:?}"
        );
    }

    /// N18: a `.d.lua`-named file is not automatically part of the project's
    /// *ambient* defs scope — that requires `[types] defs = [...]` naming it
    /// (`sema::locate_field`'s doc used to claim otherwise). With no such
    /// config, `defs/widget.d.lua` here is just an ordinary project file that
    /// happens to be named by convention, and `merge_file_types` (`env.rs`)
    /// has no notion of the `.d.lua` suffix at all for ordinary project
    /// files — collision resolution is pure first-loaded-wins, the same as
    /// the plain-file case pinned above. Before the fix, `sema::search_order`
    /// unconditionally visited every `*.d.lua` file ahead of `current` and
    /// every other project file, so the *type* resolved to `a_widget.lua`
    /// (the type merge's real answer, matching `luabox check`) while hover's
    /// *description* and goto-definition both named `defs/widget.d.lua`
    /// instead — three surfaces giving three different answers about which
    /// declaration won, off one cursor position.
    #[test]
    fn a_def_named_file_with_no_types_defs_config_is_an_ordinary_project_file_for_collisions() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        fs::write(root.join("luabox.toml"), MANIFEST_HEAD).expect("write manifest");
        fs::write(
            root.join("a_widget.lua"),
            "---@class Widget\n---@field id string the id from a_widget\n",
        )
        .expect("write the first-sorted colliding file");
        fs::create_dir_all(root.join("defs")).expect("mkdir defs");
        fs::write(
            root.join("defs/widget.d.lua"),
            "---@meta\n---@class Widget\n---@field id number the id from defs\n",
        )
        .expect("write the second-sorted colliding file");
        let path = root.join("main.lua");
        let source = "---@type Widget\nlocal w = nil\nprint(w.id)\n";
        fs::write(&path, source).expect("write the document");
        server.bootstrap();
        drop(drain(&client));

        let uri = crate::uri::path_to_uri(&path);
        let hover = server.hover(&uri, lsp_types::Position::new(2, 8));
        let text = match hover.expect("hover").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        // The type merge's own first-wins decision: `a_widget.lua` sorts
        // before `defs/widget.d.lua` (`a` < `defs`'s `d`), so it is loaded
        // first and wins the collision — exactly as the plain two-`.lua`-file
        // case above does, `.d.lua` naming notwithstanding.
        assert!(text.contains("Widget.id: string"), "{text}");
        assert!(!text.contains("Widget.id: number"), "{text}");
        // Description must agree with the type, not the other declaration.
        assert!(text.contains("the id from a_widget"), "{text}");
        assert!(!text.contains("the id from defs"), "{text}");

        let location = server
            .definition(&uri, lsp_types::Position::new(2, 8))
            .expect("definition");
        assert!(
            location.uri.as_str().ends_with("a_widget.lua"),
            "{location:?}"
        );
    }

    // === Watched-file deletion (N24) =======================================

    /// N24: a `require` target deleted mid-session must stop resolving.
    /// `watched_files_changed`'s `FileChangeType::DELETED` arm used to
    /// `continue` outright, never telling the host anything changed — the
    /// host kept the deleted file's last-known text (and everything derived
    /// from it) indefinitely, so hover kept answering from a file that no
    /// longer exists on disk. There is no `Change::RemoveFile` in
    /// `luabox-db` (`luabox_db::Change` has `SetFileText`/`SetOverlay`/
    /// `ClearOverlay`/`SetDialect`/`SetStrictness`, no delete variant) —
    /// overwriting the text to empty is the closest in-repo equivalent: an
    /// empty file exports nothing and declares nothing, so every surface
    /// that read something out of it starts reading nothing, the same
    /// externally-observable effect a real removal would have.
    #[test]
    fn a_deleted_require_target_stops_resolving() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let point_path = root.join("point.lua");
        fs::write(
            &point_path,
            "---@class Point\n---@field x number\nlocal P = {}\nreturn P\n",
        )
        .expect("write point.lua");
        let main_path = root.join("main.lua");
        let source = "local p = require(\"point\")\nprint(p.x)\n";
        fs::write(&main_path, source).expect("write main.lua");
        server.bootstrap();
        drop(drain(&client));

        let uri = crate::uri::path_to_uri(&main_path);
        let before = server.hover(&uri, lsp_types::Position::new(1, 8));
        let text_before = match before.expect("hover before deletion").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        assert!(text_before.contains("Point.x"), "{text_before}");

        // The file is gone from disk — deleting it for real, then telling
        // the server via `workspace/didChangeWatchedFiles`, exactly as a
        // client with file watching enabled would.
        fs::remove_file(&point_path).expect("delete point.lua");
        let point_uri = crate::uri::path_to_uri(&point_path);
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({
                    "changes": [{ "uri": point_uri.to_string(), "type": 3 }],
                }),
            })
            .expect("didChangeWatchedFiles");
        drop(drain(&client));

        let after = server.hover(&uri, lsp_types::Position::new(1, 8));
        let text_after = after.map(|h| match h.contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        });
        assert!(
            text_after.as_deref().is_none_or(|t| !t.contains("Point.x")),
            "a deleted require target must stop resolving: {text_after:?}"
        );
    }

    /// The one-variable control: a *non*-deleted watched-file change (a
    /// `Changed`/`Created` event) must still update resolution the way it
    /// always has — the deletion arm's fix must not have broken the other
    /// branch of the same handler.
    #[test]
    fn a_changed_require_targets_watched_file_event_still_updates_resolution() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let point_path = root.join("point.lua");
        fs::write(
            &point_path,
            "---@class Point\n---@field x number\nlocal P = {}\nreturn P\n",
        )
        .expect("write point.lua");
        let main_path = root.join("main.lua");
        let source = "local p = require(\"point\")\nprint(p.x)\n";
        fs::write(&main_path, source).expect("write main.lua");
        server.bootstrap();
        drop(drain(&client));

        // An external edit, not through the editor overlay — adds a field.
        fs::write(
            &point_path,
            "---@class Point\n---@field x number\n---@field y number\nlocal P = {}\nreturn P\n",
        )
        .expect("rewrite point.lua");
        let point_uri = crate::uri::path_to_uri(&point_path);
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({
                    "changes": [{ "uri": point_uri.to_string(), "type": 2 }],
                }),
            })
            .expect("didChangeWatchedFiles");
        drop(drain(&client));

        let main_uri = crate::uri::path_to_uri(&main_path);
        let hover = server.hover(&main_uri, lsp_types::Position::new(1, 8));
        let text = match hover.expect("hover").contents {
            lsp_types::HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        };
        assert!(text.contains("Point.x"), "{text}");
    }

    /// M58 (round 6 review, N24 half-fixed): resolution is correct
    /// immediately after a `DELETED` watched-file event — hover for the
    /// deleted class's own file already answers `null` right away, proven
    /// above. What was still missing: the event alone republished nothing
    /// for an *unrelated* already-open document whose diagnostics depended
    /// on the deleted file (a class inherited from it, here) — its Problems
    /// panel stayed clean until its own next keystroke, disagreeing with
    /// hover about the same instant. `base.lua` is deleted; `main.lua`
    /// (open, inheriting from `Base`) must be republished with the
    /// resulting diagnostic in the same batch, not on the next edit.
    #[test]
    fn a_deleted_watched_file_republishes_diagnostics_for_an_open_consumer() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let base_path = root.join("base.lua");
        fs::write(&base_path, "---@class Base\n---@field id number\n").expect("write base.lua");
        let main_path = root.join("main.lua");
        // A `---@param`, not a `---@type … = nil` binding: the latter is
        // itself an `LB0300` (`nil` does not satisfy `Sub`) so it is never
        // clean, and this test's whole subject is the transition from clean
        // to not-clean caused by the deletion alone.
        let source = "\
---@class Sub : Base

---@param s Sub
local function use(s) print(s.id) end

return use
";
        fs::write(&main_path, source).expect("write main.lua");
        server.bootstrap();
        drop(drain(&client));

        let main_uri = crate::uri::path_to_uri(&main_path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": main_uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": source,
                    },
                }),
            })
            .expect("didOpen");
        let opened = drain(&client);
        let before = published_diagnostics(&opened, &main_uri)
            .expect("didOpen publishes main.lua's diagnostics");
        assert!(
            before.as_array().is_some_and(Vec::is_empty),
            "clean before the deletion: {before:?}"
        );

        fs::remove_file(&base_path).expect("delete base.lua");
        let base_uri = crate::uri::path_to_uri(&base_path);
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({
                    "changes": [{ "uri": base_uri.to_string(), "type": 3 }],
                }),
            })
            .expect("didChangeWatchedFiles");
        let after = drain(&client);

        let republished = published_diagnostics(&after, &main_uri).expect(
            "main.lua must be republished by the DELETED event itself, not left \
             for the next keystroke",
        );
        assert!(
            republished.as_array().is_some_and(|d| !d.is_empty()),
            "s.id must now be flagged now that Base is gone: {republished:?}"
        );
    }

    /// Production readiness review, finding 4, end to end: checking one
    /// document can produce an `LB0318` about a class declared in *another*
    /// file. It must reach the user as a diagnostic on that other document,
    /// at that document's own line — not inside the checked document at a
    /// position clamped into its (shorter) text.
    ///
    /// `c.lua`'s declaration is padded well past `a.lua`'s length, so the
    /// line asserted below is one a conversion through `a.lua`'s index could
    /// not have produced.
    #[test]
    fn a_cross_file_cycle_publishes_its_diagnostic_on_the_declaring_document() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let a_path = root.join("a.lua");
        let c_path = root.join("c.lua");
        let a_source = "---@class A : B\n\n---@type B\nlocal v = nil\nlocal _ = v.whatever\n";
        let mut c_source = String::new();
        for i in 0..40 {
            let _ = writeln!(c_source, "-- padding line {i}");
        }
        c_source.push_str("---@class B : A\n---@field id number\n");
        fs::write(&a_path, a_source).expect("write a.lua");
        fs::write(&c_path, &c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        let a_uri = crate::uri::path_to_uri(&a_path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": a_uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": a_source,
                    },
                }),
            })
            .expect("didOpen");
        let published = drain(&client);

        let c_uri = crate::uri::path_to_uri(&c_path);
        let for_c = published_diagnostics(&published, &c_uri)
            .expect("the cycle's own declaring document is published too");
        let cyclic = for_c
            .as_array()
            .expect("an array")
            .iter()
            .find(|d| d["code"] == "LB0318")
            .unwrap_or_else(|| panic!("expected LB0318 on c.lua: {for_c:?}"));
        let declared_on = c_source
            .lines()
            .position(|line| line.starts_with("---@class B"))
            .expect("c.lua declares B");
        assert_eq!(cyclic["range"]["start"]["line"], json!(declared_on));

        let for_a = published_diagnostics(&published, &a_uri).expect("a.lua is published");
        let a_lines = u64::try_from(a_source.lines().count()).expect("a small line count");
        for diag in for_a.as_array().expect("an array") {
            // `A` is a member of the same cycle and IS declared in a.lua, so
            // a.lua carries its own `LB0318` — luals reports both
            // declarations of a mutual cycle and, as of round 12 R12-1, so
            // does this server. What may never appear here is `B`'s, whose
            // declaration lives in c.lua.
            assert!(
                !diag["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("`B`'s `---@class` ancestry")),
                "{diag:?}"
            );
            assert!(
                diag["range"]["start"]["line"].as_u64().expect("a line") < a_lines,
                "nothing may render at a clamped position: {diag:?}"
            );
        }
    }

    /// Production readiness review, finding 2: the CREATED/CHANGED mirror of
    /// the test above. M58 taught the DELETED arm to republish open
    /// consumers and stopped there, so the event that *restores* a
    /// dependency — a `git checkout` of a branch that has `base.lua`, an
    /// external edit that adds the missing `---@field` — left every open
    /// consumer showing the diagnostic the restore had just fixed, until
    /// that buffer's own next keystroke.
    ///
    /// Same shape as the deletion test, run backwards: `main.lua` is open
    /// and flagged because `base.lua` does not exist; `base.lua` is written
    /// and announced as CREATED; `main.lua`'s stale diagnostic must clear in
    /// that same batch.
    #[test]
    fn a_created_watched_file_republishes_diagnostics_for_an_open_consumer() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let base_path = root.join("base.lua");
        let main_path = root.join("main.lua");
        let source = "\
---@class Sub : Base

---@param s Sub
local function use(s) print(s.id) end

return use
";
        fs::write(&main_path, source).expect("write main.lua");
        server.bootstrap();
        drop(drain(&client));

        let main_uri = crate::uri::path_to_uri(&main_path);
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": main_uri.to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": source,
                    },
                }),
            })
            .expect("didOpen");
        let opened = drain(&client);
        let before = published_diagnostics(&opened, &main_uri)
            .expect("didOpen publishes main.lua's diagnostics");
        assert!(
            before.as_array().is_some_and(|d| !d.is_empty()),
            "s.id must be flagged while Base is absent: {before:?}"
        );

        fs::write(&base_path, "---@class Base\n---@field id number\n").expect("write base.lua");
        let base_uri = crate::uri::path_to_uri(&base_path);
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({
                    "changes": [{ "uri": base_uri.to_string(), "type": 1 }],
                }),
            })
            .expect("didChangeWatchedFiles");
        let after = drain(&client);

        let republished = published_diagnostics(&after, &main_uri).expect(
            "main.lua must be republished by the CREATED event itself, not left \
             for the next keystroke",
        );
        assert!(
            republished.as_array().is_some_and(Vec::is_empty),
            "s.id resolves now that Base exists: {republished:?}"
        );
    }

    /// H1: a batch of D deleted files must republish each open document
    /// exactly once (O total), not once per deleted file (D×O) — a branch
    /// switch or `git clean` deletes many files in a single
    /// `didChangeWatchedFiles` notification, and calling
    /// `republish_open_docs` (a fresh `Analysis` snapshot plus a full
    /// diagnostics pass per open document) from inside the per-event loop
    /// instead of once after it turns that single batch into a
    /// multiplicative, user-visible stall. Two deleted files, three open
    /// documents: republished-open-doc notifications must total 3, not 6.
    #[test]
    fn a_batch_of_deleted_files_republishes_open_docs_once_each_not_once_per_deletion() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();

        let del_paths: Vec<_> = ["del1.lua", "del2.lua"]
            .iter()
            .map(|name| {
                let path = root.join(name);
                fs::write(&path, "local x = 1\n").expect("write deleted file");
                path
            })
            .collect();

        let open_uris: Vec<_> = ["open1.lua", "open2.lua", "open3.lua"]
            .iter()
            .map(|name| {
                let path = root.join(name);
                let source = "local x = 1\n";
                fs::write(&path, source).expect("write open file");
                path
            })
            .collect();
        server.bootstrap();
        drop(drain(&client));

        let open_uris: Vec<_> = open_uris
            .into_iter()
            .map(|path| {
                let uri = crate::uri::path_to_uri(&path);
                server
                    .handle_notification(Notification {
                        method: DidOpenTextDocument::METHOD.to_string(),
                        params: json!({
                            "textDocument": {
                                "uri": uri.to_string(),
                                "languageId": "lua",
                                "version": 1,
                                "text": "local x = 1\n",
                            },
                        }),
                    })
                    .expect("didOpen");
                uri
            })
            .collect();
        drop(drain(&client));

        for path in &del_paths {
            fs::remove_file(path).expect("delete file");
        }
        let changes: Vec<Value> = del_paths
            .iter()
            .map(|path| json!({ "uri": crate::uri::path_to_uri(path).to_string(), "type": 3 }))
            .collect();
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({ "changes": changes }),
            })
            .expect("didChangeWatchedFiles");
        let after = drain(&client);

        let republish_count = after
            .iter()
            .filter(|m| match m {
                Message::Notification(not) => {
                    not.method == PublishDiagnostics::METHOD
                        && open_uris.iter().any(|u| not.params["uri"] == *u.as_str())
                }
                _ => false,
            })
            .count();
        assert_eq!(
            republish_count,
            open_uris.len(),
            "2 deleted files × 3 open docs must republish each open doc once (3), \
             not once per deletion (6): {after:?}"
        );
    }

    // === cross-file diagnostic bookkeeping (round 8 review, F4) ===========

    /// The `a.lua`/`c.lua` cycle fixture the finding-4 test above uses: `a`
    /// consumes `B` and so is the file whose pass produces the `LB0318`, and
    /// `B` is declared in `c`, so the diagnostic belongs to `c`'s document.
    /// `c` is padded well past `a`'s length for the same reason as there.
    fn cycle_pair(suffix: &str) -> (String, String) {
        let a = format!(
            "---@class A{suffix} : B{suffix}\n\n---@type B{suffix}\n\
             local v = nil\nlocal _ = v.whatever\n"
        );
        let mut c = String::new();
        for i in 0..40 {
            let _ = writeln!(c, "-- padding line {i}");
        }
        let _ = writeln!(c, "---@class B{suffix} : A{suffix}");
        let _ = writeln!(c, "---@field id number");
        (a, c)
    }

    /// `textDocument/didOpen` for `path` carrying `text`.
    fn did_open(server: &mut Server, path: &Path, text: &str) {
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": crate::uri::path_to_uri(path).to_string(),
                        "languageId": "lua",
                        "version": 1,
                        "text": text,
                    },
                }),
            })
            .expect("didOpen");
    }

    /// A whole-document `textDocument/didChange` for `path`.
    fn did_change(server: &mut Server, path: &Path, text: &str) {
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": crate::uri::path_to_uri(path).to_string(), "version": 2 },
                    "contentChanges": [{ "text": text }],
                }),
            })
            .expect("didChange");
    }

    /// A `textDocument/didClose` for `path`.
    fn did_close(server: &mut Server, path: &Path) {
        server
            .handle_notification(Notification {
                method: DidCloseTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": crate::uri::path_to_uri(path).to_string() },
                }),
            })
            .expect("didClose");
    }

    /// A `workspace/didChangeWatchedFiles` announcing `typ` for `path`.
    fn watched(server: &mut Server, path: &Path, typ: u8) {
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({
                    "changes": [{
                        "uri": crate::uri::path_to_uri(path).to_string(),
                        "type": typ,
                    }],
                }),
            })
            .expect("didChangeWatchedFiles");
    }

    /// The diagnostic codes of the last publish for `path` in `messages`.
    fn published_codes(messages: &[Message], path: &Path) -> Option<Vec<String>> {
        let uri = crate::uri::path_to_uri(path);
        published_diagnostics(messages, &uri).map(|diags| {
            diags
                .as_array()
                .expect("an array")
                .iter()
                .map(|d| d["code"].as_str().unwrap_or_default().to_string())
                .collect()
        })
    }

    /// F4, the **clear**: the group is produced by `a.lua`'s pass, so fixing
    /// `a.lua` means the next pass produces no group at all — and the code
    /// this pins replaced only ever republished the targets of groups it had
    /// *just produced*, so "no group" meant "nothing to republish" and
    /// `c.lua`'s panel kept a diagnostic about a cycle that no longer
    /// existed, until `c.lua` was itself touched.
    #[test]
    fn fixing_the_declaring_file_clears_its_cross_file_diagnostic_in_the_same_batch() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let (a_source, c_source) = cycle_pair("");
        let a_path = root.join("a.lua");
        let c_path = root.join("c.lua");
        fs::write(&a_path, &a_source).expect("write a.lua");
        fs::write(&c_path, &c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a_path, &a_source);
        let opened = drain(&client);
        assert!(
            published_codes(&opened, &c_path)
                .expect("c.lua is published")
                .contains(&"LB0318".to_string()),
            "the cycle must reach c.lua's panel first: {opened:?}"
        );

        // The fix: `A` no longer inherits `B`, so nothing is cyclic.
        did_change(&mut server, &a_path, "---@class A\n");
        let fixed = drain(&client);
        let codes = published_codes(&fixed, &c_path).expect(
            "c.lua must be republished by the edit that fixed the cycle, not \
             left showing it until c.lua is itself touched",
        );
        assert!(
            !codes.contains(&"LB0318".to_string()),
            "the cleared cycle must leave c.lua's panel: {codes:?}"
        );
    }

    /// F4, the **keystroke**: the group lives on `c.lua`'s document but is
    /// produced by `a.lua`'s pass, so publishing `c.lua` — which a keystroke
    /// in it does — used to replace its set with its own half alone and the
    /// diagnostic silently disappeared. Typing in a file must not delete a
    /// finding about it.
    #[test]
    fn editing_the_painted_on_file_keeps_the_cross_file_diagnostic_another_file_produced() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let (a_source, c_source) = cycle_pair("");
        let a_path = root.join("a.lua");
        let c_path = root.join("c.lua");
        fs::write(&a_path, &a_source).expect("write a.lua");
        fs::write(&c_path, &c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a_path, &a_source);
        drop(drain(&client));

        // Open `c.lua` and type in it. Neither its own pass nor the edit
        // touches `a.lua`'s finding, so it must still be there afterwards.
        did_open(&mut server, &c_path, &c_source);
        let opened = drain(&client);
        assert!(
            published_codes(&opened, &c_path)
                .expect("c.lua is published on open")
                .contains(&"LB0318".to_string()),
            "opening the painted-on file must not blank the finding: {opened:?}"
        );

        did_change(&mut server, &c_path, &format!("{c_source}-- a keystroke\n"));
        let typed = drain(&client);
        assert!(
            published_codes(&typed, &c_path)
                .expect("c.lua is published on the keystroke")
                .contains(&"LB0318".to_string()),
            "and neither must a keystroke in it: {typed:?}"
        );
    }

    /// F4, **two producers**: `c.lua` declares two cyclic classes, each
    /// completed by a different consumer. Both consumers' findings belong to
    /// `c.lua`'s document, so both must be on it — the code this pins let
    /// whichever consumer published last replace the other's.
    #[test]
    fn two_files_contributing_cross_file_diagnostics_to_one_target_both_show() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let (a1_source, c1) = cycle_pair("1");
        let (a2_source, c2) = cycle_pair("2");
        let c_source = format!("{c1}{c2}");
        let a1_path = root.join("a1.lua");
        let a2_path = root.join("a2.lua");
        let c_path = root.join("c.lua");
        fs::write(&a1_path, &a1_source).expect("write a1.lua");
        fs::write(&a2_path, &a2_source).expect("write a2.lua");
        fs::write(&c_path, &c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a1_path, &a1_source);
        did_open(&mut server, &a2_path, &a2_source);
        let opened = drain(&client);

        let codes = published_codes(&opened, &c_path).expect("c.lua is published");
        let cycles = codes.iter().filter(|c| *c == "LB0318").count();
        assert_eq!(
            cycles, 2,
            "both consumers' findings belong to c.lua's document; the second \
             publish must merge with the first, not replace it: {codes:?}"
        );
    }

    /// F4, **determinism**: a batch republish publishes as it walks the open
    /// documents, so the walk order is on the wire. Over a `HashMap` that
    /// order was hash order — an unchanged workspace emitted a differently
    /// ordered batch run to run, and (before the ledger) a different
    /// surviving cross-file group with it.
    #[test]
    fn a_batch_republish_walks_the_open_documents_in_path_order() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        // Opened in an order that is not the sorted one, so passing this
        // cannot be an accident of insertion order.
        let opened_in = ["e.lua", "a.lua", "d.lua", "b.lua", "c.lua"];
        let paths: Vec<PathBuf> = opened_in.iter().map(|name| root.join(name)).collect();
        for path in &paths {
            fs::write(path, "local x = 1\n").expect("write");
        }
        server.bootstrap();
        drop(drain(&client));
        for path in &paths {
            did_open(&mut server, path, "local x = 1\n");
        }
        drop(drain(&client));

        server
            .handle_notification(Notification {
                method: DidChangeConfiguration::METHOD.to_string(),
                params: json!({ "settings": {} }),
            })
            .expect("didChangeConfiguration");
        let batch = drain(&client);

        let published: Vec<String> = batch
            .iter()
            .filter_map(|m| match m {
                Message::Notification(not) if not.method == PublishDiagnostics::METHOD => {
                    Some(not.params["uri"].as_str().unwrap_or_default().to_string())
                }
                _ => None,
            })
            .collect();
        let mut sorted = paths.clone();
        sorted.sort();
        let expected: Vec<String> = sorted
            .iter()
            .map(|p| crate::uri::path_to_uri(p).to_string())
            .collect();
        assert_eq!(
            published, expected,
            "a republish batch must be in path order, not hash order"
        );
    }

    /// A hand-built diagnostic for the ledger tests below, distinguishable
    /// by `message` alone.
    fn ledger_diagnostic(message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            message: message.to_owned(),
            ..lsp_types::Diagnostic::default()
        }
    }

    /// The messages `target`'s document would currently show.
    fn ledger_messages(ledger: &ForeignLedger, target: &Path) -> Vec<String> {
        ledger
            .published_set(target)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    /// F4 (**two producers**) and R11-6a (**fix one, keep the other**), at
    /// the ledger rather than through the server.
    ///
    /// These two properties used to be pinned end to end on the cross-file
    /// `LB0318` fixtures below. They no longer can be: as of round 12 R12-1
    /// the declared-class cycle set is derived from the WORKSPACE, so it
    /// travels in [`ForeignLedger::workspace`] and no longer exercises
    /// [`ForeignLedger::contributions`] at all — measured, by deleting the
    /// whole contributor merge from `published_set`: the entire suite stayed
    /// green. The per-contributor mechanism is still live (every other
    /// cross-file ancestry finding — `LB0317`'s and `LB0319`'s
    /// `cross_file_class_decl_span` tier — travels in it), and pinning it on
    /// a real 200-class fixture costs a full editor check pass per
    /// keystroke of the test. So it is pinned here, on the map itself, where
    /// the mutation that would break it fails immediately.
    #[test]
    fn two_contributors_groups_for_one_target_both_appear_and_clear_independently() {
        let target = PathBuf::from("c.lua");
        let a1 = PathBuf::from("a1.lua");
        let a2 = PathBuf::from("a2.lua");
        let mut ledger = ForeignLedger::default();

        ledger.record(
            &a1,
            Vec::new(),
            BTreeMap::from([(target.clone(), vec![ledger_diagnostic("from a1")])]),
            WorkspaceScan::Skipped,
        );
        let affected = ledger.record(
            &a2,
            Vec::new(),
            BTreeMap::from([(target.clone(), vec![ledger_diagnostic("from a2")])]),
            WorkspaceScan::Skipped,
        );
        assert!(affected.contains(&target), "{affected:?}");
        assert_eq!(
            ledger_messages(&ledger, &target),
            vec!["from a1".to_owned(), "from a2".to_owned()],
            "the second contributor merges with the first, it does not replace it"
        );

        // a1 is re-checked and no longer produces anything: its group goes,
        // a2's stays. A `record` that stripped the target from every
        // contributor — or a `published_set` that read only the publishing
        // one — would take a2's finding down with it and leave a real
        // finding invisible until a2.lua is itself touched.
        let affected = ledger.record(&a1, Vec::new(), BTreeMap::new(), WorkspaceScan::Skipped);
        assert!(
            affected.contains(&target),
            "the target of a group that just went away is still republished: {affected:?}"
        );
        assert_eq!(
            ledger_messages(&ledger, &target),
            vec!["from a2".to_owned()]
        );
    }

    /// R12-1: the workspace half is REPLACED by each pass that computes it,
    /// never merged — and a pass that did not compute it leaves it alone.
    ///
    /// Both halves matter. Merging would put one cycle on a document once
    /// per file the session ever checked; treating "did not compute" as
    /// "computed empty" would wipe every cycle off every document the moment
    /// a publish for a path the analysis does not know went through
    /// ([`Server::publish_lua`]'s `None` arm, the one remaining
    /// [`WorkspaceScan::Skipped`] caller).
    #[test]
    fn the_workspace_half_is_replaced_by_a_pass_and_untouched_by_one_that_skips_it() {
        let target = PathBuf::from("c.lua");
        let stale = PathBuf::from("gone.lua");
        let a1 = PathBuf::from("a1.lua");
        let a2 = PathBuf::from("a2.lua");
        let mut ledger = ForeignLedger::default();

        ledger.record(
            &a1,
            Vec::new(),
            BTreeMap::new(),
            WorkspaceScan::Recomputed(BTreeMap::from([
                (target.clone(), vec![ledger_diagnostic("cycle")]),
                (stale.clone(), vec![ledger_diagnostic("other cycle")]),
            ])),
        );
        // a2's pass sees the same workspace minus `gone.lua`'s cycle.
        let affected = ledger.record(
            &a2,
            Vec::new(),
            BTreeMap::new(),
            WorkspaceScan::Recomputed(BTreeMap::from([(
                target.clone(),
                vec![ledger_diagnostic("cycle")],
            )])),
        );
        assert!(
            affected.contains(&stale),
            "a target that dropped out of the workspace answer is republished without it: \
             {affected:?}"
        );
        assert_eq!(ledger_messages(&ledger, &target), vec!["cycle".to_owned()]);
        assert!(ledger_messages(&ledger, &stale).is_empty());

        // A publish that computed nothing (the scratch-buffer didClose path)
        // must not read as "the workspace is clean".
        ledger.record(&a1, Vec::new(), BTreeMap::new(), WorkspaceScan::Skipped);
        assert_eq!(
            ledger_messages(&ledger, &target),
            vec!["cycle".to_owned()],
            "`Skipped` keeps the stored answer; `Recomputed(empty)` would have wiped it"
        );
    }

    /// R11-6a, the **fix-one-keep-other transition**, end to end. Two
    /// contributors, one target: what happens to the OTHER finding when one
    /// file is re-checked and its own goes away. Since R12-1 the cycle set
    /// is workspace-derived, so what this fixture exercises end to end is
    /// [`ForeignLedger::workspace`]'s wholesale replacement — the
    /// per-contributor half of the same property is pinned on the ledger
    /// directly, above.
    ///
    /// Proved by hand, the way the F7 gate test above was: `record` was
    /// temporarily taught to strip a stale target from **every** contributor
    /// rather than only from `source`'s own entry. This test went RED (0
    /// cycles on `c.lua` instead of 1) and the four F4 tests above all stayed
    /// GREEN under that same variant — which is exactly why this one had to
    /// exist.
    #[test]
    fn fixing_one_contributor_keeps_the_other_contributors_group_on_the_target() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let (a1_source, c1) = cycle_pair("1");
        let (a2_source, c2) = cycle_pair("2");
        let c_source = format!("{c1}{c2}");
        let a1_path = root.join("a1.lua");
        let a2_path = root.join("a2.lua");
        let c_path = root.join("c.lua");
        fs::write(&a1_path, &a1_source).expect("write a1.lua");
        fs::write(&a2_path, &a2_source).expect("write a2.lua");
        fs::write(&c_path, &c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a1_path, &a1_source);
        did_open(&mut server, &a2_path, &a2_source);
        let opened = drain(&client);
        assert_eq!(
            published_codes(&opened, &c_path)
                .expect("c.lua is published")
                .iter()
                .filter(|code| *code == "LB0318")
                .count(),
            2,
            "both contributors must be on c.lua before the fix, or the \
             transition below proves nothing: {opened:?}"
        );

        // Fix `a1.lua` alone: `A1` no longer inherits `B1`, so a1 contributes
        // nothing. `a2.lua` is untouched and its cycle is still real.
        did_change(&mut server, &a1_path, "---@class A1\n");
        let fixed = drain(&client);
        let codes = published_codes(&fixed, &c_path)
            .expect("c.lua must be republished by the edit that fixed a1's cycle");
        assert_eq!(
            codes.iter().filter(|code| *code == "LB0318").count(),
            1,
            "fixing one contributor must clear its group and keep the \
             other's, not clear the target: {codes:?}"
        );
    }

    /// Round 13 review, the per-keystroke workspace rebuild: the cycle pass
    /// is derived at most once per host revision, and a new revision derives
    /// a new one.
    ///
    /// Asserted on identity, not on a log line: the answer is a pure function
    /// of the analysis and the strictness, so two computations at one
    /// revision are indistinguishable by their contents and only `Rc::ptr_eq`
    /// can tell a cache hit from an honest recompute. Drop the revision check
    /// in `cycle_pass` and the first assertion fails; drop the store and the
    /// same one does.
    #[test]
    fn the_workspace_cycle_pass_is_derived_once_per_revision() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("main.lua");
        fs::write(&path, "---@class Ring : Ring\n").expect("write main.lua");
        server.bootstrap();
        drop(drain(&client));

        // Scoped: an outstanding `Analysis` blocks the host's next
        // `apply_change`, so the snapshot must not outlive the reads it is
        // for. The `Rc` may — it holds diagnostics, not the database.
        let (first, before) = {
            let snapshot = server.host.snapshot();
            let first = server.cycle_pass(&snapshot);
            let second = server.cycle_pass(&snapshot);
            assert!(
                Rc::ptr_eq(&first, &second),
                "a second read at the same revision must reuse the pass, not \
                 re-collect every `---@class` in the workspace"
            );
            (first, snapshot.revision())
        };

        // A keystroke bumps the revision, and the answer may genuinely have
        // moved with it — the cache must not outlive its key.
        did_open(&mut server, &path, "---@class Ring : Ring\n-- typed\n");
        drop(drain(&client));
        let after = server.host.snapshot();
        assert_ne!(after.revision(), before, "the edit landed");
        assert!(
            !Rc::ptr_eq(&first, &server.cycle_pass(&after)),
            "a new revision derives a new pass"
        );
    }

    /// R13-A: the close of a document with no disk backing must recompute
    /// the workspace half, not skip it — the cycle it was holding up is
    /// genuinely over the instant its overlay drops.
    ///
    /// The exact sequence, in order, because every step matters:
    ///
    /// 1. `a.lua` and `b.lua` declare a mutual `---@class` cycle; both open.
    ///    `b.lua`'s panel carries an `LB0318`.
    /// 2. `a.lua` is deleted on disk and announced. The watched-DELETE arm
    ///    blanks the DISK text, but the editor overlay still shadows it, so
    ///    `A` is still declared and the cycle is still real — asserted,
    ///    because if it cleared here the step below would prove nothing.
    /// 3. The user closes the orphaned tab. `read_to_string` fails, the
    ///    overlay drops, and now nothing in the workspace declares `A`.
    ///
    /// At step 3 the close handler passed `workspace: None` — "not
    /// recomputed" — so the stored answer stood: `b.lua` kept an error naming
    /// a class that no longer exists anywhere, on a clean open file, until a
    /// keystroke, a config reload or reopening `a.lua` happened to run
    /// another pass. RED at `d1a7381`: `b.lua` is not in the affected set at
    /// all, so the `expect` below fires.
    #[test]
    fn closing_a_deleted_buffer_clears_the_cycle_it_was_the_last_declaration_of() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let a_path = root.join("a.lua");
        let b_path = root.join("b.lua");
        let a_source = "---@class A : B\n";
        let b_source = "---@class B : A\n";
        fs::write(&a_path, a_source).expect("write a.lua");
        fs::write(&b_path, b_source).expect("write b.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a_path, a_source);
        did_open(&mut server, &b_path, b_source);
        let opened = drain(&client);
        assert!(
            published_codes(&opened, &b_path)
                .expect("b.lua is published")
                .contains(&"LB0318".to_string()),
            "the cycle must be on b.lua's panel first: {opened:?}"
        );

        fs::remove_file(&a_path).expect("delete a.lua");
        watched(&mut server, &a_path, 3);
        let deleted = drain(&client);
        assert!(
            published_codes(&deleted, &b_path)
                .expect("the DELETE republishes every open document")
                .contains(&"LB0318".to_string()),
            "the overlay still declares `A`, so the cycle is still real here — \
             if it cleared now, the close below would prove nothing: {deleted:?}"
        );

        did_close(&mut server, &a_path);
        let closed = drain(&client);
        let codes = published_codes(&closed, &b_path).expect(
            "b.lua must be republished by the close that removed the cycle's \
             last remaining declaration",
        );
        assert!(
            !codes.contains(&"LB0318".to_string()),
            "nothing declares `A` any more; the cycle must leave b.lua's panel \
             in this batch, not on some later keystroke: {codes:?}"
        );
    }

    /// R13-C: a cycle that exists ONLY because two files' declarations of one
    /// name union, reported by the editor, attributed per declaration.
    ///
    /// `a.lua` declares `A` plainly, `b.lua` reopens it as `A : B`, `c.lua`
    /// declares `B : A`. No single file contains a cycle; the union does. The
    /// server builds its own graph over its own file iteration, and until
    /// this test nothing exercised that path across files — the CLI's union
    /// tests could not, because they run the other surface's harvest.
    ///
    /// Attribution is the other half: `b.lua` and `c.lua` carry the finding
    /// because their OWN parent lists close the loop; `a.lua`'s plain
    /// `---@class A` does not and must stay clean, exactly as luals leaves it.
    #[test]
    fn the_editor_reports_a_cycle_formed_only_by_the_union_across_files() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let a_path = root.join("a.lua");
        let b_path = root.join("b.lua");
        let c_path = root.join("c.lua");
        let (a_source, b_source, c_source) =
            ("---@class A\n", "---@class A : B\n", "---@class B : A\n");
        fs::write(&a_path, a_source).expect("write a.lua");
        fs::write(&b_path, b_source).expect("write b.lua");
        fs::write(&c_path, c_source).expect("write c.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &a_path, a_source);
        let published = drain(&client);

        for (path, member) in [(&b_path, "`A`'s"), (&c_path, "`B`'s")] {
            let diags = published_diagnostics(&published, &crate::uri::path_to_uri(path))
                .unwrap_or_else(|| panic!("{} must be published", path.display()));
            let cyclic: Vec<&Value> = diags
                .as_array()
                .expect("an array")
                .iter()
                .filter(|d| d["code"] == "LB0318")
                .collect();
            assert_eq!(
                cyclic.len(),
                1,
                "one finding on the declaration that closes the loop: {diags:?}"
            );
            assert!(
                cyclic[0]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains(member)),
                "{} names its own class: {cyclic:?}",
                path.display()
            );
        }

        // The declaration that opened `A` without a parent is not a cycle
        // edge, so it draws nothing — the union finds the cycle, it does not
        // flatten the blame onto every declaration of a member.
        let for_a =
            published_codes(&published, &a_path).expect("a.lua is published, it was opened");
        assert!(
            !for_a.contains(&"LB0318".to_string()),
            "the plain `---@class A` is innocent: {for_a:?}"
        );
    }

    /// R11-4: `own` is the twin of `contributions` and must self-prune the
    /// same way. A didClose of a buffer with no disk backing records an empty
    /// own half — and an empty entry is indistinguishable from an absent one
    /// at `published_set`, so storing it buys nothing and grows the map by one
    /// entry per distinct file ever checked in a session.
    #[test]
    fn a_did_close_prunes_the_files_empty_own_half_from_the_ledger() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let path = root.join("scratch.lua");
        let source = "local x = \n";
        fs::write(&path, source).expect("write scratch.lua");
        server.bootstrap();
        drop(drain(&client));

        did_open(&mut server, &path, source);
        drop(drain(&client));
        assert!(
            server
                .foreign
                .own
                .get(&path)
                .is_some_and(|own| !own.is_empty()),
            "the syntax error must be recorded as a non-empty own half first, \
             or the prune below proves nothing"
        );

        // No disk backing left, so `close` takes the scratch-buffer path and
        // records an empty own half through the ledger.
        fs::remove_file(&path).expect("delete scratch.lua");
        server
            .handle_notification(Notification {
                method: DidCloseTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": crate::uri::path_to_uri(&path).to_string() },
                }),
            })
            .expect("didClose");

        assert!(
            !server.foreign.own.contains_key(&path),
            "an empty own half must be removed, not stored: {:?}",
            server.foreign.own.keys().collect::<Vec<_>>()
        );
    }

    /// R11-4, the other path `contributions` is pruned on: a watched DELETE
    /// empties the file's text, so its next pass has nothing to report and
    /// records an empty own half. Same leak, same prune.
    #[test]
    fn a_watched_delete_prunes_the_files_empty_own_half_from_the_ledger() {
        let (dir, mut server, client) = test_server();
        let root = dir.path();
        let path = root.join("orphan.lua");
        let uri = crate::uri::path_to_uri(&path);
        fs::write(&path, "local x = \n").expect("write orphan.lua");
        server.bootstrap();
        drop(drain(&client));

        // CHANGED, not open: publishes from disk and records the own half.
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({ "changes": [{ "uri": uri.to_string(), "type": 2 }] }),
            })
            .expect("didChangeWatchedFiles");
        drop(drain(&client));
        assert!(
            server
                .foreign
                .own
                .get(&path)
                .is_some_and(|own| !own.is_empty()),
            "the syntax error must be recorded as a non-empty own half first, \
             or the prune below proves nothing"
        );

        fs::remove_file(&path).expect("delete orphan.lua");
        server
            .handle_notification(Notification {
                method: DidChangeWatchedFiles::METHOD.to_string(),
                params: json!({ "changes": [{ "uri": uri.to_string(), "type": 3 }] }),
            })
            .expect("didChangeWatchedFiles");

        assert!(
            !server.foreign.own.contains_key(&path),
            "a deleted file must leave no empty own entry behind: {:?}",
            server.foreign.own.keys().collect::<Vec<_>>()
        );
    }

    /// The last `textDocument/publishDiagnostics` notification for `uri` in
    /// `messages`, if any.
    fn published_diagnostics(messages: &[Message], uri: &lsp_types::Uri) -> Option<Value> {
        messages.iter().rev().find_map(|m| match m {
            Message::Notification(not)
                if not.method == PublishDiagnostics::METHOD
                    && not.params["uri"] == *uri.as_str() =>
            {
                Some(not.params["diagnostics"].clone())
            }
            _ => None,
        })
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
    /// client end of the wire and the tempdir that must outlive both. The
    /// client is taken to support work-done progress; [`test_server_with`]
    /// drives the other direction.
    fn test_server() -> (TempDir, Server, Connection) {
        test_server_with(true)
    }

    /// The same, with the client's `window.workDoneProgress` capability set
    /// explicitly — every `$/progress` the server sends is gated on it.
    /// Trace defaults to `Off`, matching a real client that never sets it —
    /// [`test_server_with_trace`] is the one to reach for when a test
    /// actually asserts on `Self::merged_ambient`'s log.
    fn test_server_with(progress: bool) -> (TempDir, Server, Connection) {
        test_server_full(progress, TraceValue::Off)
    }

    /// [`test_server`] with the trace level set explicitly (N22) — for the
    /// handful of tests that assert on the "merged ambient rebuilt@" log,
    /// which is now silent unless a client opts in.
    fn test_server_with_trace(trace: TraceValue) -> (TempDir, Server, Connection) {
        test_server_full(true, trace)
    }

    fn test_server_full(progress: bool, trace: TraceValue) -> (TempDir, Server, Connection) {
        let dir = TempDir::new().expect("tempdir");
        let (server_end, client) = Connection::memory();
        let server = Server::new(server_end, dir.path().to_path_buf(), progress, trace);
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
        logged(messages).into_iter().map(|(_, m)| m).collect()
    }

    /// Log-pane messages at one severity. `log_messages` erases the level,
    /// so a test that cares which pane the client paints — WARNING is a
    /// visible complaint, LOG is not — has to key on it.
    fn log_messages_at(messages: &[Message], typ: MessageType) -> Vec<String> {
        logged(messages)
            .into_iter()
            .filter(|(t, _)| *t == typ)
            .map(|(_, m)| m)
            .collect()
    }

    /// Every `window/logMessage` the server has sent, severity kept.
    fn logged(messages: &[Message]) -> Vec<(MessageType, String)> {
        messages
            .iter()
            .filter_map(|m| match m {
                Message::Notification(not) if not.method == super::LogMessage::METHOD => Some((
                    serde_json::from_value(not.params["type"].clone())
                        .expect("`window/logMessage` carries a `type`"),
                    not.params["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )),
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

    // === didChange for a document the client never opened =================
    //
    // #78. `didChange` carries *edits*, not state: a ranged change is only
    // meaningful against the buffer it indexes into, and only an open
    // document has one. Without a `didOpen` there is nothing to splice into
    // but the empty string, or — for a file `bootstrap` indexed — a disk
    // text the client never agreed to. Either way the result is a document
    // invented out of a fragment, stored as the overlay and published as
    // diagnostics. The client is never told; it just sees nonsense in the
    // problems pane for a file it believes is fine.

    /// The one edit shape that carries no base text of its own is dropped,
    /// and the client is told why — no overlay written, no diagnostics for
    /// the fragment.
    #[test]
    fn a_ranged_did_change_for_an_unopened_document_is_logged_and_dropped() {
        let (dir, mut server, client) = test_server();
        // Never written to disk, never opened, never indexed: the host has
        // no text for it, which is precisely the state the guard is about.
        let path = dir.path().join("never_opened.lua");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [{
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 0 },
                        },
                        "text": "local x = ",
                    }],
                }),
            })
            .expect("a didChange for an unknown document is not fatal");

        let messages = drain(&client);
        assert!(
            published_diagnostics(&messages, &uri).is_none(),
            "an edit with no base text must not publish diagnostics for the \
             fragment it would have spliced: {messages:?}"
        );
        assert!(
            server.host.snapshot().file_text(&path).is_none(),
            "and it must not invent the document in the host either"
        );
        let logged = log_messages(&messages);
        assert!(
            logged
                .iter()
                .any(|m| m.contains("textDocument/didChange") && m.contains("never_opened.lua")),
            "{logged:?}"
        );
    }

    /// An empty batch carries no edit at all, so it is a legal no-op rather
    /// than the "no base text" case the ranged-and-unopened guard above logs
    /// a warning for — nothing to splice, nothing to warn about.
    #[test]
    fn an_empty_did_change_batch_on_an_unopened_document_is_silent() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("never_opened.lua");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [],
                }),
            })
            .expect("an empty didChange batch is not fatal");

        let messages = drain(&client);
        assert!(
            published_diagnostics(&messages, &uri).is_none(),
            "an empty batch has nothing to publish: {messages:?}"
        );
        assert!(
            server.host.snapshot().file_text(&path).is_none(),
            "and it must not invent the document in the host either"
        );
        assert!(
            log_messages(&messages).is_empty(),
            "a legal no-op must not be logged as though it were: {messages:?}"
        );
    }

    /// The other shape carries the whole document, so it needs no base text
    /// — a client that syncs in full mode, or replaces the buffer wholesale,
    /// still works on a document the server has not seen. The guard is about
    /// missing base text, not about the notification's provenance.
    #[test]
    fn a_full_text_did_change_for_an_unopened_document_is_applied() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("never_opened.lua");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [{ "text": "local x = 1\n" }],
                }),
            })
            .expect("didChange");

        let messages = drain(&client);
        assert!(
            published_diagnostics(&messages, &uri).is_some(),
            "a full-replace change is self-contained: {messages:?}"
        );
        assert_eq!(
            server.host.snapshot().file_text(&path).as_deref(),
            Some("local x = 1\n"),
        );
    }

    /// A ranged edit that *follows* a full replace in the same batch has a
    /// base text — the one the replace just established — so the batch is
    /// applied whole. The guard keys on the first change, not on "any
    /// change has a range".
    #[test]
    fn a_full_replace_then_a_ranged_edit_for_an_unopened_document_is_applied() {
        let (dir, mut server, _client) = test_server();
        let path = dir.path().join("never_opened.lua");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [
                        { "text": "local x = 1\n" },
                        {
                            "range": {
                                "start": { "line": 0, "character": 10 },
                                "end": { "line": 0, "character": 11 },
                            },
                            "text": "2",
                        },
                    ],
                }),
            })
            .expect("didChange");

        assert_eq!(
            server.host.snapshot().file_text(&path).as_deref(),
            Some("local x = 2\n"),
        );
    }

    /// The mirror shape of the test above: a full replace *following* a
    /// ranged edit does not rescue the batch. The gate is on the first
    /// change alone — a batch that begins ranged already has nothing to
    /// splice into, and a later full replace in it does not change that: an
    /// off-spec client that lost sync once is not trusted to have found it
    /// again mid-batch.
    #[test]
    fn a_batch_that_begins_ranged_is_dropped_even_if_it_ends_in_a_full_replace() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("never_opened.lua");
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [
                        {
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 0, "character": 0 },
                            },
                            "text": "local x = ",
                        },
                        { "text": "local x = 1\n" },
                    ],
                }),
            })
            .expect("didChange");

        let messages = drain(&client);
        assert!(
            published_diagnostics(&messages, &uri).is_none(),
            "a batch that begins ranged is dropped whole, full replace or \
             not: {messages:?}"
        );
        assert!(
            server.host.snapshot().file_text(&path).is_none(),
            "and it must not invent the document in the host either"
        );
        let logged = log_messages_at(&messages, MessageType::WARNING);
        assert!(
            logged
                .iter()
                .any(|m| m.contains("textDocument/didChange") && m.contains("never_opened.lua")),
            "{logged:?}"
        );
    }

    /// The guard's question is "has the client opened this?", not "does the
    /// host hold text?". `bootstrap` indexes every `.lua` file under the
    /// root, so `file_text` answers `Some(disk text)` for all of them — an
    /// indexed file reaches #78 by the same route (a `didChange` with no
    /// `didOpen`) and stays there: the invented overlay shadows disk, and
    /// with no `didOpen` there is no `didClose` to drop it, so the errors
    /// outlive the session's every attempt to correct them.
    #[test]
    fn a_ranged_did_change_for_an_indexed_but_unopened_document_is_dropped() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("indexed.lua");
        fs::write(&path, "local x = 1\n").expect("write");
        server.bootstrap();
        // The index pass publishes for what it read; only what the
        // `didChange` below produces is under test.
        drain(&client);
        let uri = crate::uri::path_to_uri(&path);
        server
            .handle_notification(Notification {
                method: DidChangeTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": { "uri": uri.to_string(), "version": 2 },
                    "contentChanges": [{
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 0 },
                        },
                        "text": "((( ",
                    }],
                }),
            })
            .expect("a didChange for a document that was never opened is not fatal");

        let messages = drain(&client);
        assert!(
            published_diagnostics(&messages, &uri).is_none(),
            "the index is not a buffer the client edits against: {messages:?}"
        );
        assert_eq!(
            server.host.snapshot().file_text(&path).as_deref(),
            Some("local x = 1\n"),
            "the indexed text must stay exactly as the walk read it"
        );
        let logged = log_messages_at(&messages, MessageType::WARNING);
        assert!(
            logged
                .iter()
                .any(|m| m.contains("textDocument/didChange") && m.contains("indexed.lua")),
            "{logged:?}"
        );
    }

    /// #78: a file the walk found but `bootstrap` cannot read leaves the
    /// index quietly incomplete — every cross-file answer that file would
    /// have contributed to is silently wrong. `collect_lua_files` already
    /// reports its own failure; this is the per-file half of the same rule.
    #[test]
    #[cfg(unix)]
    fn a_file_bootstrap_cannot_read_is_logged() {
        let (dir, mut server, client) = test_server();
        // A dangling symlink: the walk sees a `.lua` entry (it is not a
        // directory), `read_to_string` then fails on the missing target.
        std::os::unix::fs::symlink("nowhere.lua", dir.path().join("broken.lua")).expect("symlink");
        server.bootstrap();

        // The same read `bootstrap` just failed, so the assertion pins the
        // OS error itself rather than a paraphrase of it: drop `{err}` from
        // the format string and this test goes red.
        let os_error = fs::read_to_string(dir.path().join("broken.lua"))
            .expect_err("the symlink target does not exist")
            .to_string();
        // At WARNING, not LOG: a hole in the index makes later cross-file
        // answers wrong, which is a complaint, not a trace line.
        let logged = log_messages_at(&drain(&client), MessageType::WARNING);
        assert!(
            logged
                .iter()
                .any(|m| m.contains("broken.lua") && m.contains(&os_error)),
            "{logged:?} (expected the path and `{os_error}`)"
        );
    }

    /// The lint quick-fix *pairing*, not just its precondition.
    ///
    /// `code_actions` matches a fix back to the diagnostic that produced it
    /// by comparing suggestion span + replacement, then re-`convert`s that
    /// diagnostic under `LINT_SOURCE`. Nothing asserted that the result was
    /// the diagnostic the client was actually shown, so a future non-rule
    /// finding riding the same `lint_source` engine while carrying a
    /// machine-applicable fix would pass every band test and still hand the
    /// editor a `diagnostics` entry that matches nothing in the problems
    /// pane. Driven over a *mixed* set: `LB0022` (control-flow legality, not
    /// a lint rule, toolchain source) alongside the fixable `LB0501`.
    #[test]
    fn a_quickfix_references_the_exact_diagnostic_that_was_published() {
        let (dir, mut server, client) = test_server();
        let path = dir.path().join("main.lua");
        let source = "break\nlocal unused = 1\n";
        fs::write(&path, source).expect("write the document");
        // Through `path_to_uri`, not `format!("file://…")`: a hand-built URI
        // keeps Windows' backslashes and drive colon, and the server then
        // publishes under a different (normalized) URI than the one opened.
        let uri = crate::uri::path_to_uri(&path);
        let uri_text = uri.to_string();
        server
            .handle_notification(Notification {
                method: DidOpenTextDocument::METHOD.to_string(),
                params: json!({
                    "textDocument": {
                        "uri": uri_text,
                        "languageId": "lua",
                        "version": 1,
                        "text": source,
                    },
                }),
            })
            .expect("didOpen");

        let published = drain(&client)
            .iter()
            .find_map(|message| match message {
                Message::Notification(not) if not.method == PublishDiagnostics::METHOD => {
                    serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(
                        not.params.clone(),
                    )
                    .ok()
                    .map(|params| params.diagnostics)
                }
                _ => None,
            })
            .expect("the server published diagnostics for the open document");
        let code_of = |diag: &lsp_types::Diagnostic| match diag.code.clone() {
            Some(lsp_types::NumberOrString::String(code)) => code,
            _ => String::new(),
        };
        assert!(
            published.iter().any(|d| code_of(d) == "LB0022"),
            "the set must be mixed: {published:?}"
        );
        let published_lint = published
            .iter()
            .find(|d| code_of(d) == "LB0501")
            .expect("the fixable lint was published");

        let actions = server
            .code_actions(
                &uri,
                Range {
                    start: Position::new(1, 6),
                    end: Position::new(1, 6),
                },
            )
            .expect("code actions");
        let quickfix = actions
            .iter()
            .find_map(|action| match action {
                CodeActionOrCommand::CodeAction(action)
                    if action.kind.as_ref() == Some(&super::CodeActionKind::QUICKFIX) =>
                {
                    Some(action)
                }
                _ => None,
            })
            .expect("a quickfix for the unused local");
        assert_eq!(
            quickfix.diagnostics.as_deref(),
            Some(std::slice::from_ref(published_lint)),
            "the action must reference the published diagnostic byte for byte"
        );
    }
    /// The startup rock harvest recurses over every vendored source on rayon
    /// WORKERS, and this path had never been measured against a worker stack
    /// (Shockwave round 4). `deep_pipeline.rs` proves the CLI's pipeline
    /// survives depth 195 — but only because `real_main` pins the pool, which
    /// no library caller does.
    ///
    /// `test_server` deliberately does NOT call `pin_worker_stacks`: it goes
    /// straight to `Server::new`, the way an embedder that owns its own
    /// transport reaches the harvest, so what this exercises is rayon's
    /// unconfigured 2 MiB default. The shape is `deep_pipeline`'s — 32 files
    /// so work-stealing genuinely spreads them across workers rather than
    /// running everything on the calling thread, at the depth reference Lua
    /// accepts. A worker overflowing aborts the process, so "this test
    /// returned" is the whole assertion.
    #[test]
    fn a_deep_rock_tree_survives_the_startup_harvest() {
        const DEPTH: usize = 195;
        const FILES: usize = 32;
        let dir = TempDir::new().expect("tempdir");
        write(dir.path(), "luabox.toml", MANIFEST_HEAD);
        let nest = format!("{}{}", "{".repeat(DEPTH), "}".repeat(DEPTH));
        for n in 0..FILES {
            write(
                dir.path(),
                &format!("lua_modules/share/lua/5.4/deep_{n:02}.lua"),
                &format!(
                    "---@class Deep{n:02}\nlocal Deep = {{}}\nlocal x = {nest}\nreturn Deep\n"
                ),
            );
        }
        let (server_end, _client) = Connection::memory();
        let server = Server::new(server_end, dir.path().to_path_buf(), false, TraceValue::Off);
        assert_eq!(
            server.rocks.types().len(),
            FILES,
            "every deep rock module must have been harvested"
        );
    }
    /// A server on a worker thread with the client end **here**, so a test
    /// can answer `window/workDoneProgress/create` the way a real editor
    /// does.
    ///
    /// [`test_server`]'s client is inert, and cannot be used for a test about
    /// any pause after the first: one unanswered create now mutes progress
    /// for the whole session ([`Server::create_unanswered`]), so a create
    /// that goes unanswered on the way in means there is no second create to
    /// assert anything about. The thread runs `Server::new` → `bootstrap` →
    /// `main_loop`, which is exactly what [`run`] does after the handshake.
    struct AnsweringClient {
        client: Connection,
        server: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
        /// Everything read off the wire so far, in arrival order.
        seen: Vec<Message>,
        _dir: TempDir,
    }

    impl AnsweringClient {
        fn start() -> Self {
            let dir = TempDir::new().expect("tempdir");
            let root = dir.path().to_path_buf();
            let (server_end, client) = Connection::memory();
            let server = std::thread::spawn(move || {
                let mut server = Server::new(server_end, root, true, TraceValue::Off);
                server.bootstrap();
                server.main_loop()
            });
            Self {
                client,
                server: Some(server),
                seen: Vec::new(),
                _dir: dir,
            }
        }

        /// Read up to and including the next create, and accept it. Hands
        /// back the `(request id, token)` pair the create carried.
        fn accept_create(&mut self) -> (String, String) {
            let create = loop {
                let msg = self
                    .client
                    .receiver
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("a pause announces itself");
                self.seen.push(msg.clone());
                if let Message::Request(request) = msg
                    && request.method == super::WorkDoneProgressCreate::METHOD
                {
                    break request;
                }
            };
            self.client
                .sender
                .send(Message::Response(lsp_server::Response::new_ok(
                    create.id.clone(),
                    Value::Null,
                )))
                .expect("the server is still reading");
            (
                create.id.to_string(),
                create.params["token"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        }

        fn notify(&self, method: &str, params: Value) {
            self.client
                .sender
                .send(Message::Notification(Notification::new(
                    method.to_owned(),
                    params,
                )))
                .expect("the server is still reading");
        }

        fn reload(&self) {
            self.notify(
                super::DidChangeConfiguration::METHOD,
                json!({ "settings": {} }),
            );
        }

        /// End the session and collect everything still on the wire.
        fn shut_down(&mut self) {
            shut_down(&self.client, 77);
            let server = self.server.take().expect("shut down once");
            server
                .join()
                .expect("the server thread does not panic")
                .expect("a clean shutdown");
            self.seen.extend(drain(&self.client));
        }

        /// Every `$/progress` seen so far, as params.
        fn progress(&self) -> Vec<Value> {
            self.seen
                .iter()
                .filter_map(|message| match message {
                    Message::Notification(not) if not.method == super::Progress::METHOD => {
                        Some(not.params.clone())
                    }
                    _ => None,
                })
                .collect()
        }
    }

    /// The config reload re-harvests the whole rock tree on the main loop —
    /// the startup cost, landing mid-session (Shockwave round 4 measured a
    /// ~0.9 s stall on every `luabox.toml` save). The work is unchanged; what
    /// is new is that the client is told a pause is happening, so it reads as
    /// progress rather than as a hang.
    #[test]
    fn a_config_reload_announces_itself_with_a_progress_token() {
        let mut client = AnsweringClient::start();
        // The two startup tokens first: a client that ignored them would be
        // told nothing about the reload either.
        client.accept_create();
        client.accept_create();

        client.reload();
        client.accept_create();
        client.shut_down();

        let progress = client.progress();
        assert!(
            progress.iter().any(|params| {
                params["value"]["kind"] == "begin"
                    && params["value"]["title"] == "Reloading luabox configuration"
            }),
            "{progress:?}"
        );
        assert!(
            progress
                .iter()
                .any(|params| params["value"]["kind"] == "end"),
            "{progress:?}"
        );
    }

    /// Every `$/progress` notification the client end received, as params.
    fn progress_notifications(client: &Connection) -> Vec<Value> {
        drain(client)
            .iter()
            .filter_map(|message| match message {
                Message::Notification(not) if not.method == super::Progress::METHOD => {
                    Some(not.params.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// The startup rock harvest is synchronous and runs before the main loop,
    /// so it is a stretch of protocol silence the client cannot attribute to
    /// anything — Shockwave round 5 captured 746 ms of it between the
    /// `initialize` response and the bootstrap token, which covered an index
    /// that took 0.1 ms. The harvest now carries its own token.
    ///
    /// `Server::new` is exactly where `run` reaches it, and by then
    /// `initialize_finish` has answered the handshake, so the server-initiated
    /// `window/workDoneProgress/create` this sends is protocol-legal.
    #[test]
    fn the_startup_harvest_announces_itself_with_a_progress_token() {
        let (_dir, _server, client) = test_server();
        let progress = progress_notifications(&client);
        assert!(
            progress.iter().any(|params| {
                params["value"]["kind"] == "begin"
                    && params["value"]["title"] == super::STARTUP_HARVEST_PROGRESS.1
            }),
            "the startup harvest must open a token: {progress:?}"
        );
        assert!(
            progress
                .iter()
                .any(|params| params["value"]["kind"] == "end"),
            "…and close it: {progress:?}"
        );
    }

    /// Every `window/workDoneProgress/create` among messages already read off
    /// the wire, as `(request id, token)` pairs.
    fn progress_creates_of(messages: &[Message]) -> Vec<(String, String)> {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::Request(request)
                    if request.method == super::WorkDoneProgressCreate::METHOD =>
                {
                    Some((
                        request.id.to_string(),
                        request.params["token"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                    ))
                }
                _ => None,
            })
            .collect()
    }

    /// Two consecutive reloads must not reuse either identifier. Both were
    /// compile-time constants derived from the token *name*, so three reloads
    /// sent three `window/workDoneProgress/create` requests sharing one id and
    /// one token (Shockwave round 6) — and both are required to be unique:
    /// JSON-RPC ids among outstanding requests, LSP tokens by the
    /// server-generated-token rule.
    ///
    /// Asserted on the pair, not on the format. What the counter has to buy is
    /// distinctness; `luabox/reload-3` is one spelling of it.
    #[test]
    fn consecutive_reloads_do_not_reuse_a_progress_id_or_token() {
        let mut client = AnsweringClient::start();
        // The two startup creates, answered, so the reloads' creates are sent
        // at all — and dropped, so what is counted below is the reloads'.
        client.accept_create();
        client.accept_create();

        let mut seen: Vec<(String, String)> = Vec::new();
        for _ in 0..3 {
            client.reload();
            seen.push(client.accept_create());
        }
        client.shut_down();

        assert_eq!(seen.len(), 3, "one create per reload: {seen:?}");
        let ids: std::collections::HashSet<&str> = seen.iter().map(|(id, _)| id.as_str()).collect();
        let tokens: std::collections::HashSet<&str> =
            seen.iter().map(|(_, token)| token.as_str()).collect();
        assert_eq!(ids.len(), 3, "request ids must be distinct: {seen:?}");
        assert_eq!(
            tokens.len(),
            3,
            "progress tokens must be distinct: {seen:?}"
        );
        // …and each still says which pause it is announcing.
        assert!(
            tokens
                .iter()
                .all(|token| token.starts_with(super::RELOAD_HARVEST_PROGRESS.0)),
            "a reload must announce itself under the reload kind: {seen:?}"
        );
    }

    /// The other direction, on both harvest paths: a client that does not
    /// advertise `window.workDoneProgress` must receive **zero** progress
    /// traffic. The reload path read no capability at all before round 5 — it
    /// announced itself to every client — and the startup path would have
    /// inherited that bug the moment it grew a token.
    #[test]
    fn a_client_without_the_capability_receives_no_progress_traffic() {
        let (_dir, mut server, client) = test_server_with(false);
        assert!(
            progress_notifications(&client).is_empty(),
            "startup harvest sent progress to a client that never asked"
        );

        server
            .handle_notification(Notification {
                method: super::DidChangeConfiguration::METHOD.to_string(),
                params: json!({ "settings": {} }),
            })
            .expect("a configuration change is not fatal");
        assert!(
            progress_notifications(&client).is_empty(),
            "config reload sent progress to a client that never asked"
        );

        server.bootstrap();
        assert!(
            progress_notifications(&client).is_empty(),
            "bootstrap sent progress to a client that never asked"
        );
    }

    /// The pin's `Err` arm is not an accident — it is the arm production takes
    /// on the `luabox lsp` path, where `real_main` has already built the global
    /// pool by the time `run` calls this. Calling it repeatedly, and after a
    /// pool exists, must be a no-op rather than a panic or an abort.
    ///
    /// This asserts the `Err` arm and nothing about the stack size — it
    /// cannot, because the global pool of this binary may have been built by
    /// any earlier test. What isolates the pin itself is
    /// `tests/pinned_stack.rs`, which owns its own test binary for exactly
    /// that reason.
    #[test]
    fn pinning_worker_stacks_is_idempotent_and_never_fatal() {
        use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};

        super::pin_worker_stacks();
        super::pin_worker_stacks();
        // The pool is usable afterwards either way — whether this call built
        // it or found one already built by another test in this binary.
        let sum: usize = (0..1024_usize).into_par_iter().sum();
        assert_eq!(sum, 1024 * 1023 / 2);
    }

    /// The token belongs to the client until it says otherwise:
    /// `window/workDoneProgress/create` is a *request*, and a `$/progress`
    /// under a token the client has not acknowledged is a notification it may
    /// drop. Round 7 captured the create at t=0.0053 and the `begin` at
    /// t=0.0054 with no response in between, and `main_loop` discarded every
    /// response, so the answer was never read.
    ///
    /// This drives `Server::new` (whose startup harvest is the session's first
    /// announced pause) on a worker thread and plays the client here, so the
    /// assertion is about message *order on the wire*: nothing under the token
    /// before the create, nothing at all while the response is outstanding,
    /// and the `begin` only after it is sent.
    #[test]
    fn a_progress_begin_waits_for_the_create_response() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let (server_end, client) = Connection::memory();
        let server =
            std::thread::spawn(move || drop(Server::new(server_end, root, true, TraceValue::Off)));

        // Read up to the create request. A `$/progress` in this prefix would
        // be a token being used before it was asked for.
        let mut before = Vec::new();
        let create = loop {
            let msg = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the startup harvest announces itself");
            if let Message::Request(request) = &msg
                && request.method == super::WorkDoneProgressCreate::METHOD
            {
                break request.clone();
            }
            before.push(msg);
        };
        assert!(
            !before.iter().any(is_progress),
            "a token was reported under before it was created: {before:?}"
        );

        // Nothing may arrive while the create is outstanding. The window is
        // *derived* from the bound rather than written out: a literal 60 ms
        // silently stops proving anything the moment
        // `PROGRESS_CREATE_TIMEOUT` is lowered to 50 ms — the wait would
        // already have expired and the `begin` already have been sent, and
        // this would still pass. A quarter of the bound is comfortably inside
        // it however the constant moves.
        let inside_the_wait = super::PROGRESS_CREATE_TIMEOUT / 4;
        assert!(
            client.receiver.recv_timeout(inside_the_wait).is_err(),
            "the server reported under the token before the client created it"
        );

        client
            .sender
            .send(Message::Response(lsp_server::Response::new_ok(
                create.id,
                Value::Null,
            )))
            .expect("the server is still reading");

        // …and now it speaks.
        let begin = loop {
            let msg = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the begin follows the create response");
            if is_progress(&msg) {
                break msg;
            }
        };
        let Message::Notification(begin) = begin else {
            panic!("progress is a notification")
        };
        assert_eq!(begin.params["value"]["kind"], "begin", "{begin:?}");
        server.join().expect("the server thread does not panic");
    }

    /// Whether a message is a `$/progress` notification.
    fn is_progress(message: &Message) -> bool {
        matches!(message, Message::Notification(not) if not.method == super::Progress::METHOD)
    }

    /// A client that speaks while a `window/workDoneProgress/create` is
    /// outstanding must not lose what it said, **and must not have it
    /// reordered**. `await_progress_create` takes those messages off the wire
    /// to find the response, and `main_loop` drains the queue before reading
    /// anything new.
    ///
    /// Two `didOpen`s, not one. With a single message the test could not tell
    /// a queue from a one-slot buffer, and the commit that introduced it
    /// claimed arrival order — which is a property of a `VecDeque` drained
    /// from the front and would survive a change to `pop_back` or to a `Vec`
    /// used as a stack completely unremarked. The assertion is that the
    /// server publishes for `a.lua` before `b.lua`, in the order they were
    /// sent.
    #[test]
    fn notifications_sent_during_the_create_wait_are_handled_in_arrival_order() {
        let dir = TempDir::new().expect("tempdir");
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        let first = src.join("a.lua");
        let second = src.join("b.lua");
        fs::write(&first, "local a = 1\n").expect("write");
        fs::write(&second, "local b = 2\n").expect("write");
        let root = dir.path().to_path_buf();
        let (server_end, client) = Connection::memory();

        let server = std::thread::spawn(move || {
            let mut server = Server::new(server_end, root, true, TraceValue::Off);
            server.main_loop()
        });

        let create = loop {
            let msg = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the startup harvest announces itself");
            if let Message::Request(request) = &msg
                && request.method == super::WorkDoneProgressCreate::METHOD
            {
                break request.clone();
            }
        };
        // Both spoken into the window where the server is *not* in
        // `main_loop`, so both go on the queue.
        let first_uri = crate::uri::path_to_uri(&first);
        let second_uri = crate::uri::path_to_uri(&second);
        for (uri, text) in [
            (&first_uri, "local a = 1\n"),
            (&second_uri, "local b = 2\n"),
        ] {
            client
                .sender
                .send(Message::Notification(Notification {
                    method: DidOpenTextDocument::METHOD.to_string(),
                    params: json!({
                        "textDocument": {
                            "uri": uri.as_str(),
                            "languageId": "lua",
                            "version": 1,
                            "text": text,
                        }
                    }),
                }))
                .expect("the server is still reading");
        }
        client
            .sender
            .send(Message::Response(lsp_server::Response::new_ok(
                create.id,
                Value::Null,
            )))
            .expect("the server is still reading");

        let mut published = Vec::new();
        while published.len() < 2 {
            let msg = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("both buffered didOpens are handled once the loop starts");
            if let Message::Notification(not) = &msg
                && not.method == PublishDiagnostics::METHOD
            {
                published.push(not.params["uri"].clone());
            }
        }
        assert_eq!(
            published,
            vec![
                Value::from(first_uri.as_str()),
                Value::from(second_uri.as_str())
            ],
            "the queue must drain in arrival order"
        );
        shut_down(&client, 99);
        server
            .join()
            .expect("the server thread does not panic")
            .expect("a clean shutdown");
    }

    /// Send the ordered `shutdown` → `exit` pair a clean session ends with.
    fn shut_down(client: &Connection, id: i32) {
        client
            .sender
            .send(Message::Request(Request {
                id: RequestId::from(id),
                method: super::Shutdown::METHOD.to_string(),
                params: Value::Null,
            }))
            .expect("the server is still reading");
        client
            .sender
            .send(Message::Notification(Notification {
                method: super::Exit::METHOD.to_string(),
                params: Value::Null,
            }))
            .expect("the server is still reading");
    }

    /// Wait for a server thread to finish, failing rather than hanging.
    ///
    /// The bound matters: the bug these tests cover made
    /// `Connection::handle_shutdown` wait 30 s for an `exit` it was already
    /// holding, and over a memory transport — where `exit` does not close the
    /// channel the way a closed stdin does — that is a 30 s block rather than
    /// a fast failure. A `join()` would sit through it and then pass or fail
    /// on the exit code alone; this fails on the *hang*.
    fn join_within(
        handle: std::thread::JoinHandle<anyhow::Result<()>>,
        done: &std::sync::mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        done.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the server ends the session promptly rather than waiting out a timeout");
        handle.join().expect("the server thread does not panic")
    }

    /// A `shutdown`/`exit` pair sent while a `window/workDoneProgress/create`
    /// is outstanding must still end the session cleanly.
    ///
    /// This is the round-8 regression. `await_progress_create` drained both
    /// messages onto [`Server::pending`], and `Connection::handle_shutdown`
    /// reads the *channel* for the `exit` that follows the request it answers
    /// — it cannot see the queue. So the server answered the shutdown, waited
    /// out `handle_shutdown`'s own 30 s bound for a notification it was
    /// already holding, and exited 1. A session without the progress
    /// capability exited 0, which is what made it a regression rather than a
    /// standing wart.
    ///
    /// Both windows are covered, because both call the same helper: the
    /// startup harvest (before `main_loop` is entered at all) and the config
    /// reload (from inside it).
    #[test]
    fn a_shutdown_during_the_startup_create_wait_exits_cleanly() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let (server_end, client) = Connection::memory();
        let (done, finished) = std::sync::mpsc::channel();

        let server = std::thread::spawn(move || {
            let mut server = Server::new(server_end, root, true, TraceValue::Off);
            let outcome = server.main_loop();
            let _ = done.send(());
            outcome
        });

        await_create(&client);
        // Answered with a shutdown instead of a response — the sequence an
        // editor sends when the user closes the window during the harvest.
        shut_down(&client, 1);

        join_within(server, &finished).expect("a clean shutdown, not exit 1");
        assert!(
            drain(&client).iter().any(|message| {
                matches!(message, Message::Response(response) if response.id == RequestId::from(1))
            }),
            "the shutdown request must still be answered"
        );
    }

    /// The same, on the reload path — the window Shockwave reproduced it in.
    /// Here the server is inside `main_loop` when the create goes out.
    #[test]
    fn a_shutdown_during_the_reload_create_wait_exits_cleanly() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let (server_end, client) = Connection::memory();
        let (done, finished) = std::sync::mpsc::channel();

        let server = std::thread::spawn(move || {
            let mut server = Server::new(server_end, root, true, TraceValue::Off);
            let outcome = server.main_loop();
            let _ = done.send(());
            outcome
        });

        // Get past the startup token first, so the create being waited for
        // below is the reload's.
        let startup = await_create(&client);
        client
            .sender
            .send(Message::Response(lsp_server::Response::new_ok(
                startup.id,
                Value::Null,
            )))
            .expect("the server is still reading");
        client
            .sender
            .send(Message::Notification(Notification {
                method: super::DidChangeConfiguration::METHOD.to_string(),
                params: json!({ "settings": {} }),
            }))
            .expect("the server is still reading");

        await_create(&client);
        shut_down(&client, 2);

        join_within(server, &finished).expect("a clean shutdown, not exit 1");
        assert!(
            drain(&client).iter().any(|message| {
                matches!(message, Message::Response(response) if response.id == RequestId::from(2))
            }),
            "the shutdown request must still be answered"
        );
    }

    /// Read up to and including the next `window/workDoneProgress/create`.
    fn await_create(client: &Connection) -> lsp_server::Request {
        loop {
            let msg = client
                .receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("a pause announces itself");
            if let Message::Request(request) = &msg
                && request.method == super::WorkDoneProgressCreate::METHOD
            {
                return request.clone();
            }
        }
    }

    /// A client that answers the create with an **error** has refused the
    /// token, and nothing may be reported under it.
    ///
    /// `await_progress_create` matched on the response id and never looked at
    /// `response.error`, so a client replying `-32601` still received the
    /// `begin`, the per-file `report`s and the `end` — the exact traffic the
    /// wait was added to prevent, under a token the client had just declined.
    /// Refusal now takes the `progress: false` path.
    #[test]
    fn a_refused_progress_token_is_never_reported_under() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let (server_end, client) = Connection::memory();
        // Files for the bootstrap index to report on, so a server that
        // ignored the refusal would have something to say.
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");
        for i in 0..5 {
            fs::write(src.join(format!("f{i}.lua")), "local x = 1\n").expect("write");
        }

        let refuser = std::thread::spawn(move || {
            // Refuse every create this session sends.
            while let Ok(message) = client.receiver.recv() {
                if let Message::Request(request) = &message
                    && request.method == super::WorkDoneProgressCreate::METHOD
                {
                    let refusal = lsp_server::Response::new_err(
                        request.id.clone(),
                        ErrorCode::MethodNotFound as i32,
                        "no work-done progress here".to_string(),
                    );
                    if client.sender.send(Message::Response(refusal)).is_err() {
                        break;
                    }
                }
                if is_progress(&message) {
                    return true;
                }
            }
            false
        });

        let mut server = Server::new(server_end, root, true, TraceValue::Off);
        server.bootstrap();
        drop(server);
        assert!(
            !refuser.join().expect("the client thread does not panic"),
            "a refused token was reported under"
        );
    }

    /// A client that answers *nothing* is not a client that refuses. The one
    /// create it was asked for still takes the degraded path — the token is
    /// used regardless, which is the behaviour this whole mechanism replaced
    /// and so is no worse than it — and after that the session goes quiet:
    /// **no further create is sent at all**.
    ///
    /// Round 9 (B2) is why the second half is not "asked without waiting".
    /// Skipping only the wait kept the latency win and lost the guarantee:
    /// the create went out, the client's late `-32601` landed in
    /// `main_loop`'s discard arm, and `$/progress` went out under a token it
    /// had refused. The semantics are disclosed rather than hidden — one
    /// timeout mutes progress for the session — and the alternative was a
    /// refusal check any slow client could step around.
    #[test]
    fn one_unanswered_create_mutes_progress_for_the_session() {
        let (_dir, mut server, client) = test_server();
        // `Server::new`'s startup harvest has already paid the one wait.
        assert!(server.create_unanswered.get(), "the first wait timed out");
        // One drain: it consumes the wire, so the create and the progress it
        // carried have to be read out of the same batch.
        let startup = drain(&client);
        assert_eq!(
            progress_creates_of(&startup).len(),
            1,
            "exactly one create was asked: {startup:?}"
        );
        assert!(
            startup.iter().any(is_progress),
            "the degraded path still reports under the token it asked for"
        );

        let before = std::time::Instant::now();
        server.bootstrap();
        assert!(
            before.elapsed() < super::PROGRESS_CREATE_TIMEOUT,
            "a second create was waited out after the first went unanswered"
        );
        let after = drain(&client);
        assert!(
            progress_creates_of(&after).is_empty(),
            "a second create was SENT after the first went unanswered — a \
             late refusal of it would land in main_loop's discard arm: \
             {after:?}"
        );
        assert!(
            !after.iter().any(is_progress),
            "…and reported under: {after:?}"
        );
    }

    /// …and with the capability, all three paths do speak — to a client that
    /// answers, which after round 9 is the only kind that hears more than the
    /// first.
    #[test]
    fn a_client_with_the_capability_hears_every_announced_pause() {
        let mut client = AnsweringClient::start();
        let (_, harvest) = client.accept_create();
        let (_, bootstrap) = client.accept_create();
        client.reload();
        let (_, reload) = client.accept_create();
        client.shut_down();

        let tokens: Vec<String> = client
            .progress()
            .iter()
            .map(|params| params["token"].as_str().unwrap_or_default().to_owned())
            .collect();
        for (name, token) in [
            ("startup", &harvest),
            ("bootstrap", &bootstrap),
            ("reload", &reload),
        ] {
            assert!(
                tokens.iter().any(|seen| seen == token),
                "{name} announced nothing under `{token}`: {tokens:?}"
            );
        }
    }
}
