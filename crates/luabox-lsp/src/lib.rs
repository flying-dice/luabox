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
//! # Features (this crate today)
//!
//! The list below is the dispatch table in `server.rs` written out; if a
//! request is handled there it is claimed here, and nowhere else.
//!
//! - **Streamed diagnostics** — parse errors, dialect legality, type and lint
//!   diagnostics pushed after every open/change/close.
//! - **Hover** — binding types from `---@type`/`---@param`, function
//!   signatures from `@param`/`@return`, class fields, with LuaCATS doc text.
//! - **Goto definition / type definition / implementation** — locals and
//!   upvalues via HIR name resolution, class fields to their `---@field`
//!   site, functions to their declaration, `require("mod")` to the module
//!   file; type-definition and implementation resolve against the
//!   workspace-global class graph (see [`goto_type_definition`],
//!   [`goto_implementation`]).
//! - **Find references + rename** — every use of a binding, class or field
//!   across the workspace ([`references`]), and `prepare`-gated rename
//!   driving the same resolution ([`rename`]).
//! - **Completion** — `.`/`:` member completion on class-typed receivers;
//!   scope-visible locals, file globals, and keywords elsewhere; plus
//!   tsc-style **auto-require imports** that insert the `require` line with
//!   the item.
//! - **Code actions** — machine-applicable lint quick-fixes, plus the
//!   type-driven refactors in [`code_action`] (add-missing-field,
//!   annotate-local-from-inference, generate-class-from-literal,
//!   dot/colon conversion).
//! - **Call hierarchy** — `prepare`, incoming and outgoing calls
//!   ([`call_hierarchy`]).
//! - **Document + workspace symbols** — functions (nested, with containers),
//!   top-level locals, `---@class` declarations ([`symbols`]); the flat
//!   `workspace/symbol` counterpart adds the workspace-global `---@alias` and
//!   `---@enum` declarations, which no single file's tree owns.
//! - **Document highlight, folding ranges, selection ranges** — read/write
//!   occurrence highlighting and the two structural range families.
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
//! # Handler naming
//!
//! Each handler module exposes one entry point per LSP request it serves,
//! named for that request's method: the `textDocument/`, `workspace/` and
//! `callHierarchy/` prefix dropped, converted to snake case, pluralised when
//! the reply is a list. So `hover::hover`, `goto_definition::definition`,
//! `folding::folding_ranges`, `call_hierarchy::incoming_calls`. `Server`'s
//! wrapper for each carries that same name, which is what makes the dispatch
//! table in `server.rs` readable as the method list it is. Where a module's
//! name and its entry point's coincide the stutter stands — it is the price of
//! having one rule rather than a judgement call per module.
//!
//! Of the request handlers SPEC §8 asks for, all are dispatched. What is
//! still outstanding there is transport and trimmings: **TCP** (stdio is the
//! only transport — [`run_stdio`]; `luabox lsp --stdio` accepts the flag
//! editors pass and ignores it, because there is nothing to switch away
//! from), plus postfix snippets, on-type formatting, the extract/inline and
//! sort-requires code actions, and the persistent mmap index cache. None of
//! those is claimed above; if it is not in the list, it is not handled.

mod call_hierarchy;
mod code_action;
mod completion;
mod diagnostics;
mod document_highlight;
mod fmt;
mod folding;
mod goto_definition;
mod goto_implementation;
mod goto_type_definition;
mod hover;
mod inlay_hints;
mod line_index;
mod references;
mod rename;
mod requires;
mod selection_range;
mod sema;
mod semantic_tokens;
mod server;
mod signature_help;
mod symbols;
mod uri;

pub use line_index::LineIndex;
pub use server::{PINNED_STACK_BYTES, run, run_stdio};
pub use uri::{path_to_uri, uri_to_path};
