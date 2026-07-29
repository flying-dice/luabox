//! Project manifest and layout — **Distribution** bounded context
//! (SPEC.md §5, §16).
//!
//! v1 is a pure static toolchain: luabox *consumes* a rock tree
//! (`lua_modules/`, materialized by the user's own luarocks) and never
//! produces one. What survives of Distribution is therefore the manifest
//! layer, not the package graph — no solver, no providers, no lockfile, no
//! registry bridge.
//!
//! This crate owns `luabox.toml` end to end — the typed model ([`model`]),
//! its validation ([`model::Manifest::parse`]) and its errors ([`error`]) —
//! plus the project layout every frontend shares ([`layout`]): where the
//! project root is, which files count as first-party source, and where
//! ambient definition packages come from.
//!
//! It also publishes that contract in machine-readable form: [`schema`] is a
//! complete JSON Schema for `luabox.toml`, printed by `luabox schema` for
//! editors, validators and LLM coding assistants. Both it and the parser are
//! rendered from one declarative key table (`contract`), so the document
//! those tools read cannot describe a manifest the toolchain would reject —
//! or reject one it accepts.
//!
//! Per SPEC.md §16, Distribution "never parses syntax": `edition`/`target`
//! are validated as plain strings against a local allow-list rather than via
//! `luabox-syntax`, and the layout walk classifies files by path alone.

// The half of `contract` that carries prose, defaults and examples is read by
// the schema renderer, which is test-only — the binary prints the generated
// file with `include_str!` and needs no JSON writer. A non-test build
// therefore sees those fields as unread; they are not.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "read by the test-only schema renderer")
)]
mod contract;
pub mod error;
pub mod layout;
pub mod model;
mod parse;
pub mod schema;
