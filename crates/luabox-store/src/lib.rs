//! Content-addressed store — the **Distribution** bounded context (SPEC.md §6,
//! §16), alongside the resolver (`luabox-resolve`).
//!
//! This crate is the storage layer for luabox's package manager: a global,
//! content-addressed cache (the CLI mounts it at `~/.luabox/store`) that
//! **hard-links** files into projects instead of copying them, on the
//! bun/pnpm model. Install speed is a feature — linking a tree is a handful of
//! `link(2)` calls, not a byte copy per file.
//!
//! It is deliberately **library-first and network-free**. The resolver (#17)
//! produces package sets with checksums; the registry client (#20) fetches
//! bytes; this crate is the storage both sit on. It never opens a socket and
//! never parses Lua.
//!
//! # Layout
//!
//! Given a store root, the on-disk shape is:
//!
//! ```text
//! <root>/
//!   objects/sha256/<2-char prefix>/<remaining 62 chars>   # per-file CAS
//!   packages/<name>/<version>/<tree-hash>.json            # tree manifests
//!   tmp/                                                    # atomic staging
//!   gc.lock                                                # advisory gc lock
//! ```
//!
//! ## Why per-file CAS (extracted trees), not one blob per archive
//!
//! Each *file* is hashed and stored once under `objects/`; a [`TreeManifest`]
//! lists the `path -> object` entries that reconstitute a package. This is
//! pnpm's model, and it is chosen deliberately:
//!
//! - **Dedup across versions.** Consecutive releases of a package usually
//!   change a handful of files. Per-file CAS stores only the changed files
//!   again; the rest are shared objects. A per-archive store would re-store the
//!   entire tree for every version.
//! - **Hard links need real files.** You can only hard-link a concrete file
//!   into a project. Extracted per-file objects *are* those files; a packed
//!   archive would have to be exploded first, defeating the point.
//!
//! # Read-only objects (and what that means for your project files)
//!
//! Stored objects are made **read-only** to protect the CAS from accidental
//! mutation — pnpm does the same. A hard-linked project file is the *same
//! inode* as its object, so it inherits that read-only bit: installed files are
//! read-only by default. Code that must mutate an installed file materializes
//! with [`LinkMode::Copy`], which produces an independent, writable copy.
//!
//! On Windows, `remove_file` refuses read-only files, so garbage collection
//! clears the attribute before deleting.
//!
//! # Concurrency
//!
//! - **Interning is atomic and write-once.** [`Store::put_tree`] stages each
//!   object in `tmp/` (same volume) and `rename`s it into place. Because the
//!   address is the content hash, two processes interning the same tree write
//!   byte-identical objects and the rename is idempotent — a concurrent double
//!   `put` cannot corrupt an object.
//! - **`gc` takes a best-effort advisory lock** and never deletes objects
//!   newer than a grace window, so it cannot race an in-flight install. See
//!   [`Store::gc_with_options`] for the full model and its limits.
//!
//! # Integration points
//!
//! - **#20 (registry fetch):** fetches and verifies archive bytes, then hands
//!   an extracted tree directory to [`Store::put_tree`]; the returned
//!   [`TreeManifest`] (checksummed) is what the lockfile records.
//! - **#21 (install):** reads a [`TreeManifest`] (from the resolver, or via
//!   [`Store::read_package_manifest`]) and calls [`Store::materialize`] to
//!   link the tree into `node_modules`-equivalent project storage.

mod hash;
mod json;
mod lock;
mod manifest;
mod object;
mod store;

pub use hash::{hash_bytes, hash_file, hash_reader};
pub use manifest::{FileEntry, TreeManifest};
pub use store::{
    CorruptEntry, CorruptKind, GcOptions, GcReport, LinkMode, MaterializeReport, Store, StoreStats,
};
