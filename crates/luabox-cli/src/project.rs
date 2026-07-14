//! Shared project discovery: the walk-up-to-`luabox.toml` + read + parse
//! step that every project-aware command begins with.
//!
//! Discovery has two contracts, and commands pick the one that fits:
//!
//! * [`discover_manifest`] returns `None` when there is no `luabox.toml` in
//!   `cwd` or any ancestor, letting the command fall back to its own
//!   manifest-less default (`check`, `lint`, `fmt`, `test`, `run`, `bench`
//!   each root a default project at `cwd`).
//! * [`discover_required`] (and [`require_root`], which stops at the root
//!   without reading) instead errors — dependency and audit commands have
//!   nothing to resolve without a manifest.
//!
//! Both share one manifest reader so the read-error (`cannot read ...`) and
//! parse-error (`invalid ...:\n<rendered>`) messages, and the
//! no-manifest bail, stay byte-identical across every command. The *view*
//! each command builds on top of `(root, Manifest)` — its edition/target
//! validation, its `Project` struct — stays in the command, because those
//! differ (some commands don't parse the edition at all; the ones that do
//! word the error differently).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use luabox_resolve::manifest::Manifest;

/// Walk up from `cwd` (cargo-style) to the nearest directory containing a
/// `luabox.toml` file. Returns that directory (the project root), or `None`
/// if neither `cwd` nor any ancestor has one. Does not read the manifest.
pub(crate) fn find_manifest_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        if current.join("luabox.toml").is_file() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

/// Read and parse `<root>/luabox.toml`, with the shared read-error and
/// parse-error rendering every command relies on:
///
/// * an unreadable file → a "cannot read" error naming the path;
/// * a manifest that fails to parse → an "invalid" error naming the path,
///   followed by one rendered parse error per line.
pub(crate) fn read_manifest(root: &Path) -> anyhow::Result<Manifest> {
    let manifest_path = root.join("luabox.toml");
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read `{}`", manifest_path.display()))?;
    Manifest::parse(&text).map_err(|errors| {
        let rendered = errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::anyhow!("invalid `{}`:\n{rendered}", manifest_path.display())
    })
}

/// Discover the project for a command that supports a manifest-less default:
/// the root and parsed manifest of the nearest `luabox.toml`, or `None` when
/// there is none in `cwd` or any parent. A malformed manifest that *is*
/// present is still an error (via [`read_manifest`]).
pub(crate) fn discover_manifest(cwd: &Path) -> anyhow::Result<Option<(PathBuf, Manifest)>> {
    match find_manifest_dir(cwd) {
        Some(root) => {
            let manifest = read_manifest(&root)?;
            Ok(Some((root, manifest)))
        }
        None => Ok(None),
    }
}

/// The project root for a command that requires a manifest, without reading
/// it: the nearest `luabox.toml`'s directory, or the shared no-manifest
/// error. Used by `audit`, which only needs the root to locate the lockfile.
pub(crate) fn require_root(cwd: &Path) -> anyhow::Result<PathBuf> {
    find_manifest_dir(cwd).ok_or_else(|| no_manifest_error(cwd))
}

/// Discover the project for a command that requires a manifest: the root and
/// parsed manifest of the nearest `luabox.toml`, or the shared no-manifest
/// error. Used by the dependency commands.
pub(crate) fn discover_required(cwd: &Path) -> anyhow::Result<(PathBuf, Manifest)> {
    let root = require_root(cwd)?;
    let manifest = read_manifest(&root)?;
    Ok((root, manifest))
}

/// The message reported when a manifest-requiring command is run outside any
/// project — shared so `audit` and the dependency commands stay identical.
fn no_manifest_error(cwd: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "no `luabox.toml` found in `{}` or any parent directory — run `luabox init` first",
        cwd.display()
    )
}
