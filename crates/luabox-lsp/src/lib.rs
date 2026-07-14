//! Language server over `luabox-db` — the **Frontend** bounded context
//! (SPEC.md §8, §16).
//!
//! rust-analyzer's architecture in miniature: a synchronous mainloop over
//! [`lsp_server::Connection`] (stdio in production, in-memory in tests), one
//! [`AnalysisHost`](luabox_db::AnalysisHost) fed by document lifecycle
//! notifications, and per-request [`Analysis`](luabox_db::Analysis)
//! snapshots. No async runtime.
//!
//! # Error contract
//!
//! This is a driver crate: its public entry points ([`run`], [`run_stdio`])
//! and internal handlers return [`anyhow::Result`] deliberately. Errors here
//! are transport- and mainloop-level failures that abort the server rather
//! than values a caller inspects and recovers from, so a single opaque error
//! type with rich context is the right contract — a bespoke public error enum
//! would buy no caller anything. Library crates in the workspace keep their
//! typed errors; the buck stops at this application boundary.
//!
//! # Tranche 1 features (this crate today)
//!
//! - **Streamed diagnostics** — parse errors, dialect legality, and type
//!   diagnostics pushed after every open/change/close.
//! - **Hover** — binding types from `---@type`/`---@param`, function
//!   signatures from `@param`/`@return`, class fields, with LuaCATS doc text.
//! - **Goto definition** — locals/upvalues via HIR name resolution, class
//!   fields to their `---@field` site, functions to their declaration,
//!   `require("mod")` to the module file.
//! - **Completion** — `.`/`:` member completion on class-typed receivers;
//!   scope-visible locals, file globals, and keywords elsewhere.
//! - **Document symbols** — functions (nested, with containers), top-level
//!   locals, `---@class` declarations.
//! - **Formatting** — whole-document and range (MVP: range formats the whole
//!   document, see [`fmt`]) via the canonical formatters; parse errors yield
//!   no edits, never an error.
//! - **Semantic tokens** — full-document, standard-types-only legend, for
//!   `.lua` (HIR-resolved locals vs globals, LuaCATS doc comments; see
//!   [`semantic_tokens`]).
//! - **Inlay hints** — the rich table inference's binding types rendered
//!   after unannotated declarations (see [`inlay_hints`]).
//! - **Signature help** — the callee's resolved signature(s) while the
//!   cursor sits inside a call's argument list, with the active parameter
//!   and `---@overload` alternates (see [`signature_help`]).
//!
//! The remaining SPEC §8 surface (find-refs, rename, code actions, call
//! hierarchy, TCP transport) is P4 polish.

mod call_hierarchy;
mod code_action;
mod completion;
mod diagnostics;
mod document_highlight;
mod fmt;
mod folding;
mod goto_def;
mod goto_impl;
mod goto_type;
mod hover;
mod inlay_hints;
mod line_index;
mod references;
mod rename;
mod selection_range;
mod sema;
mod semantic_tokens;
mod server;
mod signature_help;
mod symbols;
mod uri;

pub use line_index::LineIndex;
pub use server::{run, run_stdio};
pub use uri::{path_to_uri, uri_to_path};
