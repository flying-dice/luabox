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
//! seam: path/workspace deps resolve from disk, git deps via the
//! [`GitProvider`] (`git` CLI, sha-pinned); the registry provider (#20)
//! plugs in behind the same trait. Results carry a deterministic
//! [`Lockfile`] (`luabox.lock`), and failures render cargo-style conflict
//! reports via `Display`.
//!
//! Status: manifest parsing/editing, solver, lockfile, and git fetching
//! landed. Registry client and luarocks bridge are P2 follow-ups (#19/#20).

pub mod advisory;
pub mod git_provider;
mod http;
pub mod lockfile;
pub mod luarocks;
pub mod manifest;
pub mod provider;
pub mod registry;
mod report;
mod semver_ranges;
pub mod solver;

pub use git_provider::{GitCheckout, GitProvider};
pub use lockfile::{
    LOCKFILE_NAME, LOCKFILE_VERSION, LockedPackage, LockedSource, Lockfile, LockfileError,
};
pub use luarocks::{LUAROCKS_PREFIX, LuaRocksProvider};
pub use manifest::{Manifest, ManifestError};
pub use provider::{
    GitReference, PackageId, PackageMeta, PackageProvider, PathProvider, ProviderError, Source,
    StackedProvider, StaticProvider,
};
pub use registry::{IndexDep, IndexEntry, REGISTRY_ENV, Registry, RegistryError, RegistryProvider};
pub use solver::{Resolution, ResolveError, ResolvedPackage, resolve, verify_resolution};
