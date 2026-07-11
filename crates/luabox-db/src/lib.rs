//! Incremental analysis database — the **Semantics** context's query core
//! (SPEC.md §8, §16).
//!
//! This crate is luabox's salsa layer: it wraps the shipped producers
//! ([`luabox_syntax`], [`luabox_hir`], [`luabox_types`]) in memoized queries so
//! that editing one file re-analyses only what actually depends on it. It is
//! the shared engine behind `check`, `lint`, `fmt`, and the LSP — SPEC.md §16
//! names the salsa DB as the Semantics boundary contract.
//!
//! # Layers
//!
//! - **Inputs** — [`SourceFile`] (path, text, dialect) and [`Project`]
//!   (strictness, file list): the leaves of the graph, mutated via
//!   [`salsa::Setter`].
//! - **Queries** — [`parse`], [`annotations`], [`type_env`], [`lower`],
//!   [`diagnostics`], [`project_diagnostics`]: `#[salsa::tracked]` functions,
//!   memoized per revision.
//! - **Values** — [`ParsedModule`], [`Annotations`], [`Diagnostics`],
//!   [`TypeEnvHandle`]: `Arc`-backed, salsa-compatible wrappers around producer
//!   outputs (rowan trees are not `PartialEq`/`Update` on their own).
//! - **VFS** — [`Vfs`]/[`FileId`]: path interning plus a disk/overlay content
//!   store (editor buffers shadow disk).
//! - **Boundary** — [`AnalysisHost`] (mutable world + `apply_change`) and
//!   [`Analysis`] (immutable snapshot with `diagnostics`/`parse`/…). The LSP
//!   (P1, ticket #14) consumes exactly these two types and nothing deeper.
//!
//! # Salsa idiom
//!
//! Built on `salsa` 0.27 (the rust-analyzer-lineage crate) with the current
//! attribute idioms: `#[salsa::input]`, `#[salsa::tracked]`, `#[salsa::db]`.
//! Those macros generate `unsafe impl`s in this crate, and the value wrappers
//! hand-implement [`salsa::Update`]; the crate therefore opts out of the
//! workspace's `deny(unsafe_code)` at the salsa boundary. All hand-written
//! `unsafe` is confined to the value wrappers ([`ParsedModule`] et al.) and
//! documented there.
#![allow(
    unsafe_code,
    reason = "salsa's #[input]/#[tracked]/#[db] macros generate `unsafe impl Update`, \
              and the Arc-backed value wrappers hand-implement it; this crate is the \
              salsa boundary and cannot use deny(unsafe_code)."
)]

mod db;
mod host;
mod input;
mod query;
mod value;
mod vfs;

pub use db::{Db, RootDatabase};
pub use host::{Analysis, AnalysisHost, Change};
pub use input::{Project, SourceFile};
pub use query::{
    annotations, binding_types, diagnostics, lower, module_export, module_surface_checked,
    outgoing_calls, parse, project_diagnostics, type_env,
};
pub use value::{
    Annotations, BindingTypes, Diagnostics, LoweredHandle, ModuleExport, ModuleSurfaceChecked,
    OutgoingCalls, ParsedModule, TypeEnvHandle,
};
pub use vfs::{FileId, Vfs};

// Re-export the upstream vocabulary consumers configure the host with, so a
// caller can depend on `luabox-db` alone for the boundary.
pub use luabox_diag::Diagnostic;
pub use luabox_syntax::lua::Dialect;
pub use luabox_types::Strictness;
