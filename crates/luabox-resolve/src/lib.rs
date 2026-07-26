//! Project manifest — **Distribution** bounded context (SPEC.md §5, §16).
//!
//! v1 is a pure static toolchain: luabox *consumes* a rock tree
//! (`lua_modules/`, materialized by the user's own luarocks) and never
//! produces one. What survives of Distribution is therefore the manifest
//! layer, not the package graph — no solver, no providers, no lockfile, no
//! registry bridge.
//!
//! This crate owns `luabox.toml` end to end: typed model, validation, and
//! comment-preserving round-tripping live in [`manifest`]. Per SPEC.md §16,
//! Distribution "never parses syntax" — `edition`/`target` are validated as
//! plain strings against a local allow-list, not via `luabox-syntax`.

pub mod manifest;

pub use manifest::{Manifest, ManifestError};
