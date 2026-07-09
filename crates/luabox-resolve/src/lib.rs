//! Dependency resolution — **Distribution** bounded context (SPEC.md §6, §16).
//!
//! Full-semver PubGrub solver, first-party registry client, transparent
//! LuaRocks bridge, deterministic text lockfile (`luabox.lock`).
//! Boundary contract: the package-graph API; knows nothing of syntax/types.
//!
//! This crate owns `luabox.toml` end to end: typed model, validation, and
//! comment-preserving round-tripping live in [`manifest`]. Per SPEC.md §16,
//! Distribution "never parses syntax" — `edition`/`target` are validated as
//! plain strings against a local allow-list, not via `luabox-syntax`.
//!
//! Status: manifest parsing landed — P0/P1. PubGrub solver, registry
//! client, luarocks bridge, and lockfile are placeholder — P2.

pub mod manifest;

pub use manifest::{Manifest, ManifestError};
