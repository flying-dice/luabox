//! Project manifest — **Distribution** bounded context (SPEC.md §5, §16).
//!
//! v1 is a pure static toolchain: luabox *consumes* a rock tree
//! (`lua_modules/`, materialized by the user's own luarocks) and never
//! produces one. What survives of Distribution is therefore the manifest
//! layer, not the package graph — no solver, no providers, no lockfile, no
//! registry bridge.
//!
//! This crate owns `luabox.toml` end to end: typed model, validation, and
//! comment-preserving round-tripping live in [`manifest`].
//! [`project::effective_manifest`] is the one resolvable *view* of a project:
//! the manifest plus the identity checks (name/version present) every
//! project-level consumer needs. Per SPEC.md §16, Distribution "never parses
//! syntax" — `edition`/`target` are validated as plain strings against a local
//! allow-list, not via `luabox-syntax`. The dialect **compatibility** model
//! ([`dialect`]) is the one place this crate reasons over
//! `luabox_syntax::Dialect` as a family (family sets + lowerability, #5) — it
//! classifies dialects, it does not parse them.

pub mod dialect;
pub mod manifest;
pub mod project;

pub use dialect::{DialectSet, lowerable};
pub use manifest::{Manifest, ManifestError};
pub use project::{ProjectError, effective_manifest};
