//! Dependency resolution — **Distribution** bounded context (SPEC.md §6, §16).
//!
//! Full-semver PubGrub solver, transparent luarocks.org bridge (the registry,
//! pnpm/bun-style), deterministic text lockfile (`luabox.lock`). Boundary
//! contract: the package-graph API; knows nothing of syntax/types.
//!
//! This crate owns `luabox.toml` end to end: typed model, validation, and
//! comment-preserving round-tripping live in [`manifest`]. It also reads the
//! project's `*.rockspec` (the package manifest, pnpm-style — name, version,
//! and registry dependencies) via [`luarocks::rockspec`] and merges it with
//! `luabox.toml`'s tool config + path/git sources ([`project::effective_manifest`]).
//! Per SPEC.md §16, Distribution "never parses syntax" — `edition`/`target`
//! are validated as plain strings against a local allow-list, not via
//! `luabox-syntax`. The dialect **compatibility** model ([`dialect`]) is the
//! one place the resolver reasons over `luabox_syntax::Dialect` as a family
//! (family sets + lowerability, #5) — it classifies dialects, it does not parse
//! them.
//!
//! Resolution ([`resolve`]) is PubGrub over the [`provider::PackageProvider`]
//! seam: path/workspace deps resolve from disk, git deps via the
//! [`GitProvider`] (`git` CLI, sha-pinned), and bare registry deps via the
//! [`LuaRocksProvider`] (luarocks.org). Results carry a deterministic
//! [`Lockfile`] (`luabox.lock`), and failures render cargo-style conflict
//! reports via `Display`.

pub mod dialect;
pub mod git_provider;
mod http;
pub mod lockfile;
pub mod luarocks;
pub mod manifest;
pub mod project;
pub mod provider;
mod report;
mod semver_ranges;
pub mod solver;
pub mod url_provider;

pub use dialect::{DialectSet, lowerable};
pub use git_provider::{GitCheckout, GitProvider};
pub use lockfile::{
    LOCKFILE_NAME, LOCKFILE_VERSION, LockedPackage, LockedSource, Lockfile, LockfileError,
};
pub use luarocks::{LuaRocksProvider, RockSummary};
pub use manifest::{Manifest, ManifestError};
pub use project::{ProjectError, effective_manifest};
pub use provider::{
    GitReference, PackageId, PackageMeta, PackageProvider, PathProvider, ProviderError, Source,
    StackedProvider, StaticProvider,
};
pub use solver::{Resolution, ResolveError, ResolvedPackage, resolve, verify_resolution};
pub use url_provider::UrlProvider;
