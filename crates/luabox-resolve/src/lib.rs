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
//! Resolution ([`resolve`]) is PubGrub over the [`provider::PackageProvider`]
//! seam: path/workspace deps resolve from disk today; registry (#20) and
//! git (#21) providers plug in behind the same trait. Results carry a
//! deterministic [`Lockfile`] (`luabox.lock`), and failures render
//! cargo-style conflict reports via `Display`.
//!
//! Status: manifest parsing, solver, and lockfile landed. Registry client
//! and luarocks bridge are P2 follow-ups (#20/#21).

pub mod lockfile;
pub mod manifest;
pub mod provider;
mod report;
mod semver_ranges;
pub mod solver;

pub use lockfile::{
    LOCKFILE_NAME, LOCKFILE_VERSION, LockedPackage, LockedSource, Lockfile, LockfileError,
};
pub use manifest::{Manifest, ManifestError};
pub use provider::{
    GitReference, PackageId, PackageMeta, PackageProvider, PathProvider, ProviderError, Source,
    StackedProvider, StaticProvider,
};
pub use solver::{Resolution, ResolveError, ResolvedPackage, resolve, verify_resolution};
