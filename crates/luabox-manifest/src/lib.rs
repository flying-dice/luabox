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
//! Per SPEC.md §16, Distribution "never parses syntax": `edition`/`target`
//! are validated as plain strings against a local allow-list rather than via
//! `luabox-syntax`, and the layout walk classifies files by path alone.

pub mod error;
pub mod layout;
pub mod model;
mod parse;
