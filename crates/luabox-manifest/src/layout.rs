//! Project layout: where the project is, what counts as its source, and
//! where its ambient definition packages come from (SPEC.md §3, §5, §16).
//!
//! Every project-aware frontend — `check`, `lint`, `fmt`, `build`, `doc`, the
//! watcher and the LSP — begins by answering the same three questions, and
//! they must all answer them identically or the editor and CI disagree about
//! which files exist. This module is the single answer:
//!
//! * **Where** — [`find_manifest_dir`] walks up to the nearest `luabox.toml`
//!   (cargo-style), [`read_manifest`] reads and validates it, and
//!   [`discover_manifest`] combines the two with a manifest-less `None`.
//! * **What** — [`is_project_source`] is the path-level predicate ("is this a
//!   first-party `.lua` file?") and [`collect_lua_files`] is the walk that
//!   enumerates them in deterministic order. `lua_modules/` (the vendored
//!   rock tree, [`VENDOR_DIR`]), dot-directories and the `[build] out`
//!   directory are excluded by both.
//! * **Which types** — [`resolve_project_defs`] and [`resolve_dep_defs`]
//!   locate the `*.d.lua` ambient definition files the project and its direct
//!   dependencies contribute (#108, the luals `workspace.library` model), and
//!   [`collect_rock_sources`] enumerates the *installed rock sources* of a
//!   luarocks tree whose LuaCATS annotations Semantics harvests for their
//!   type surfaces (#30).
//!
//! Layout is classification by *path*: Distribution never parses syntax
//! (SPEC.md §16), so nothing here reads a Lua file's contents to decide what
//! it is — [`collect_rock_sources`] hands every rock source's text back
//! unjudged, and whether a given one carries usable annotations is Semantics'
//! call. Diagnostics are the frontend's job too — an unresolvable `[types]
//! defs` entry comes back as a name, not an `LB1002`.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::ManifestError;
use crate::model::{Dependency, Manifest};

/// The project-local dependency tree `luarocks install --tree lua_modules`
/// writes. Excluded from every first-party source walk (see
/// [`is_project_source`]).
pub const VENDOR_DIR: &str = "lua_modules";

/// The manifest file name every project is rooted by.
const MANIFEST_FILE: &str = "luabox.toml";

/// Whether a source walk yields `*.d.lua` definition files.
///
/// `*.d.lua` files are `---@meta` definition files — ambient *type surfaces*,
/// never project source — so `check`, `build` and `doc` [`Exclude`] them and
/// consume them through [`resolve_project_defs`] instead. `fmt` and `lint`
/// [`Include`] them: they format and lint those files like any other.
///
/// [`Exclude`]: DefFiles::Exclude
/// [`Include`]: DefFiles::Include
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefFiles {
    /// Yield `*.d.lua` files alongside ordinary sources (`fmt`, `lint`).
    Include,
    /// Skip `*.d.lua` files (`check`, `build`, `doc`).
    Exclude,
}

/// One resolved ambient definition file: the text, plus the label diagnostics
/// name it by.
///
/// The label's *shape* is the caller's convention and is deliberately not
/// uniform: [`resolve_project_defs`] labels a project def by its root-relative
/// path (`defs/love.d.lua`), while [`resolve_dep_defs`] prefixes the
/// dependency name (`geometry/defs/geometry.d.lua`) because the path alone
/// would be ambiguous across packages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefSource {
    /// How diagnostics name this file.
    pub label: String,
    /// The file's contents.
    pub text: String,
}

/// A project's layout could not be read from disk.
///
/// Deliberately not a `luabox-diag` diagnostic: Distribution owns the manifest
/// API, not diagnostic rendering (see [`crate::error`]). The frontend renders
/// these — the messages are the ones `luabox` has always printed.
#[derive(Debug)]
pub enum LayoutError {
    /// A file could not be read.
    ReadFile { path: PathBuf, source: io::Error },
    /// A directory could not be listed.
    ReadDir { path: PathBuf, source: io::Error },
    /// A manifest was read but failed validation.
    InvalidManifest {
        path: PathBuf,
        errors: Vec<ManifestError>,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadFile { path, .. } => write!(f, "cannot read `{}`", path.display()),
            Self::ReadDir { path, .. } => write!(f, "cannot read directory `{}`", path.display()),
            Self::InvalidManifest { path, errors } => {
                writeln!(f, "invalid `{}`:", path.display())?;
                let rendered = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                f.write_str(&rendered)
            }
        }
    }
}

impl std::error::Error for LayoutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadFile { source, .. } | Self::ReadDir { source, .. } => Some(source),
            Self::InvalidManifest { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------

/// Walk up from `cwd` (cargo-style) to the nearest directory containing a
/// `luabox.toml` file. Returns that directory (the project root), or `None`
/// if neither `cwd` nor any ancestor has one. Does not read the manifest.
#[must_use]
pub fn find_manifest_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        if current.join(MANIFEST_FILE).is_file() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

/// Read and parse `<root>/luabox.toml`, with the shared read-error and
/// parse-error rendering every command relies on:
///
/// * an unreadable file → [`LayoutError::ReadFile`], naming the path;
/// * a manifest that fails to parse → [`LayoutError::InvalidManifest`], naming
///   the path, followed by one rendered parse error per line.
pub fn read_manifest(root: &Path) -> Result<Manifest, LayoutError> {
    let path = root.join(MANIFEST_FILE);
    let text = fs::read_to_string(&path).map_err(|source| LayoutError::ReadFile {
        path: path.clone(),
        source,
    })?;
    Manifest::parse(&text).map_err(|errors| LayoutError::InvalidManifest { path, errors })
}

/// Discover the project for a command that supports a manifest-less default:
/// the root and parsed manifest of the nearest `luabox.toml`, or `None` when
/// there is none in `cwd` or any parent. A malformed manifest that *is*
/// present is still an error (via [`read_manifest`]).
pub fn discover_manifest(cwd: &Path) -> Result<Option<(PathBuf, Manifest)>, LayoutError> {
    match find_manifest_dir(cwd) {
        Some(root) => {
            let manifest = read_manifest(&root)?;
            Ok(Some((root, manifest)))
        }
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------
// Source classification
// ---------------------------------------------------------------------

/// Whether `path` lies in the project's first-party tree: not under the build
/// output directory `out_dir`, and — for a path under `root` — with no
/// dot-directory/dot-file component and no [`VENDOR_DIR`] component at any
/// depth. Says nothing about the file's kind; [`is_project_source`] adds that.
///
/// A path *outside* `root` is judged on `out_dir` alone: the component rules
/// are relative to the project, and there is no relative path to apply them to.
#[must_use]
pub fn is_in_project_tree(path: &Path, root: &Path, out_dir: Option<&Path>) -> bool {
    if out_dir.is_some_and(|out| path.starts_with(out)) {
        return false;
    }
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    !rel.components().any(|component| {
        let name = component.as_os_str();
        // A vendored rock tree is never first-party, at any depth: a
        // workspace member has its own, and a rock may vendor one in turn.
        name == OsStr::new(VENDOR_DIR)
            || matches!(component, Component::Normal(_))
                && name.to_str().is_some_and(|s| s.starts_with('.'))
    })
}

/// Whether `path` is a first-party project *source* file: a `.lua` file
/// ([`is_in_project_tree`]) — the path-level form of the rule
/// [`collect_lua_files`] walks.
///
/// `*.d.lua` files count as source here: whether a given command wants them is
/// a [`DefFiles`] choice made per walk, not a property of the path.
#[must_use]
pub fn is_project_source(path: &Path, root: &Path, out_dir: Option<&Path>) -> bool {
    path.extension().and_then(OsStr::to_str) == Some("lua")
        && is_in_project_tree(path, root, out_dir)
}

/// Whether `path` is a vendored rock source the toolchain *reads*: a `.lua`
/// file under the project's own `lua_modules/share/lua/<X.Y>/` tree — the
/// path-level form of what [`collect_rock_sources`] walks for the type
/// harvest (#30).
///
/// Any version directory counts, not just the one the manifest currently
/// selects: the caller that needs this rule (`luabox check --watch`) cannot
/// know the manifest's answer without re-reading it, and a rerun is exactly
/// how it finds out. Only the project root's own tree qualifies — a nested
/// `lua_modules/` inside a rock is never read, matching the harvest.
#[must_use]
pub fn is_rock_source(path: &Path, root: &Path) -> bool {
    if path.extension().and_then(OsStr::to_str) != Some("lua") {
        return false;
    }
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    let mut parts = rel.components();
    parts.next() == Some(Component::Normal(OsStr::new(VENDOR_DIR)))
        && parts.next() == Some(Component::Normal(OsStr::new("share")))
        && parts.next() == Some(Component::Normal(OsStr::new("lua")))
        && matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next().is_some()
}

/// All project-source `*.lua` files under `root` ([`is_project_source`]), in
/// deterministic order — entries sorted by file name at each directory level,
/// walked depth-first.
///
/// Excluded directories are pruned rather than filtered per file, so a large
/// `lua_modules/` tree (whatever `luarocks install --tree lua_modules`
/// materialized) costs nothing to skip. Walking into it made `luabox check`
/// typecheck rock sources against the project's own strictness — which fails
/// on any rock that is not trivially typed, and took `luabox build` down with
/// it.
///
/// Symlinked directories are NOT descended ([`is_real_dir`]) — the same guard,
/// for the same reason, as the two sibling walks ([`collect_rock_lua`],
/// [`collect_d_lua`]). A link back up the tree (`src/loop -> <root>`) used to
/// be followed by `Path::is_dir()`, so the walk re-collected every source once
/// per level until the kernel's symlink budget ran out: 41 copies of one
/// `src/main.lua` on Linux, and 41 diagnostics for one mistake. Termination by
/// `ELOOP` is termination by filesystem accident; this is termination by
/// design. A symlinked *file* is still taken — only the cycle vector is
/// closed.
pub fn collect_lua_files(
    root: &Path,
    out_dir: Option<&Path>,
    defs: DefFiles,
) -> Result<Vec<PathBuf>, LayoutError> {
    let mut lua = Vec::new();
    walk(root, root, out_dir, defs, &mut lua)?;
    Ok(lua)
}

fn walk(
    dir: &Path,
    root: &Path,
    out_dir: Option<&Path>,
    defs: DefFiles,
    lua: &mut Vec<PathBuf>,
) -> Result<(), LayoutError> {
    let read_error = |source| LayoutError::ReadDir {
        path: dir.to_path_buf(),
        source,
    };
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(read_error)?
        .collect::<Result<_, _>>()
        .map_err(read_error)?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if is_real_dir(&entry) {
            if is_in_project_tree(&path, root, out_dir) {
                walk(&path, root, out_dir, defs, lua)?;
            }
        } else if is_project_source(&path, root, out_dir)
            && !(defs == DefFiles::Exclude && is_def_file(&path))
        {
            lua.push(path);
        }
    }
    Ok(())
}

/// Whether `path` is a `*.d.lua` `---@meta` definition file.
///
/// Public because this is the **one owner** of the `.d.lua` convention, and it
/// has consumers outside this crate: `luabox-lsp`'s `locate_field` resolves a
/// member's declaration by visiting defs files before ordinary project files,
/// because the type merge lets a defs declaration win a same-name collision —
/// so navigation and the merge must agree on which files those are. A second
/// copy of the rule has no compiler-enforced link to this one: loosen the
/// convention here (a second suffix, a directory-based rule) and the copy
/// keeps the old behaviour, breaking that precedence with no build failure —
/// only a wrong hover result someone eventually notices.
#[must_use]
pub fn is_def_file(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.ends_with(".d.lua"))
}

/// Root-relative path with forward slashes — stable output across platforms.
/// A path outside `root` is rendered whole rather than erroring.
#[must_use]
pub fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------
// Ambient definition packages
// ---------------------------------------------------------------------

/// Resolve `[types] defs` entries against the project-local `defs/`
/// directory: each name loads `defs/<name>.d.lua` *and/or* every `*.d.lua`
/// under `defs/<name>/`, sorted (SPEC.md §3 — registry-distributed packages
/// are P2+).
///
/// Returns the resolved files — labelled by root-relative path — plus the
/// names that resolved to nothing at all, in declaration order. Reporting an
/// unresolved name is the frontend's call: `luabox check` raises `LB1002`,
/// `lint` and the LSP quietly do without those globals.
pub fn resolve_project_defs(root: &Path, names: &[String]) -> (Vec<DefSource>, Vec<String>) {
    let defs_dir = root.join("defs");
    let mut defs = Vec::new();
    let mut unresolved = Vec::new();
    for name in names {
        let found = load_defs_package(&defs_dir, name, &mut defs, &|file| display_rel(file, root));
        if !found {
            unresolved.push(name.clone());
        }
    }
    (defs, unresolved)
}

/// Resolve the def files each DIRECT dependency contributes to the consuming
/// project's ambient scope (#108, the luals `workspace.library` model).
///
/// For each direct dependency (`[dependencies]` + `[dev-dependencies]`) in
/// alphabetical name order — the deterministic collision-winner order — locate
/// its package root (a path dependency in place at its `path`, every other
/// kind under `lua_modules/<name>/` — the rock tree the user materializes with
/// luarocks; luabox only reads it), read that dependency's *own* `[types]
/// defs`, and load those files from the dependency's `defs/` directory.
///
/// A dependency with no manifest on disk (not materialized, or a source kind
/// whose root cannot be located here) or no `[types] defs` simply contributes
/// nothing. Resolution is one level deep only: a dependency's *own*
/// dependencies' defs do not transit.
#[must_use]
pub fn resolve_dep_defs(root: &Path, manifest: &Manifest) -> Vec<DefSource> {
    // `[dependencies]` and `[dev-dependencies]` are each `BTreeMap`s (already
    // name-sorted); merge them into one name-sorted list so the winner order
    // is a single alphabetical sweep across both.
    let mut deps: Vec<(&String, &Dependency)> = manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
        .collect();
    deps.sort_by(|a, b| a.0.cmp(b.0));

    let mut out = Vec::new();
    for (name, dep) in deps {
        let dep_root = match dep {
            Dependency::Path(p) => root.join(p.path.replace('\\', "/")),
            _ => root.join(VENDOR_DIR).join(name),
        };
        let Ok(dep_manifest) = read_manifest(&dep_root) else {
            continue;
        };
        let defs_dir = dep_root.join("defs");
        for def_name in &dep_manifest.types.defs {
            load_defs_package(&defs_dir, def_name, &mut out, &|file| {
                dep_def_label(name, file, &dep_root)
            });
        }
    }
    out
}

/// Load one `[types] defs` entry from `defs_dir` — `<name>.d.lua` and/or every
/// `*.d.lua` under `<name>/`, sorted — labelling each file with `label`.
/// Returns whether the entry resolved to at least one readable file.
fn load_defs_package(
    defs_dir: &Path,
    name: &str,
    out: &mut Vec<DefSource>,
    label: &dyn Fn(&Path) -> String,
) -> bool {
    let mut found = false;
    let single = defs_dir.join(format!("{name}.d.lua"));
    if single.is_file()
        && let Ok(text) = fs::read_to_string(&single)
    {
        out.push(DefSource {
            label: label(&single),
            text,
        });
        found = true;
    }
    let dir = defs_dir.join(name);
    if dir.is_dir() {
        let mut files = Vec::new();
        collect_d_lua(&dir, &mut files);
        // Deliberately `PathBuf`'s component-wise `Ord` — the *opposite* rule
        // to the raw-byte sort its sibling rock walk uses (`collect_rock_
        // sources`), and safe here for the reason that one is not: `defs/` is
        // never listed by `resolve_candidates`, so there is no `require`
        // resolution order for this order to contradict. All it has to be is
        // deterministic, and `load_defs_package` is the single shared
        // consumer for both front-ends, so `check` and the server pick the
        // same `LB0307` winner. Do not "fix" this one to match the other.
        files.sort();
        for file in files {
            if let Ok(text) = fs::read_to_string(&file) {
                out.push(DefSource {
                    label: label(&file),
                    text,
                });
                found = true;
            }
        }
    }
    found
}

/// A readable, deterministic label for a dependency-contributed def file: the
/// dependency name plus the file's path within the dependency
/// (`<dep>/defs/<name>.d.lua`), forward-slashed for cross-platform stability.
fn dep_def_label(dep_name: &str, file: &Path, dep_root: &Path) -> String {
    format!("{dep_name}/{}", display_rel(file, dep_root))
}

// ---------------------------------------------------------------------
// Installed rock sources (the luarocks-tree type harvest, #30)
// ---------------------------------------------------------------------

/// One Lua source file installed in a luarocks tree, as the type harvest sees
/// it (#30).
///
/// Not a [`DefSource`]: a def file is an ambient `---@meta` *declaration*
/// package named by a manifest, while this is ordinary vendored code that also
/// happens to carry annotations, and it answers to a `require` name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RockSource {
    /// The dotted `require` name this file answers to — its path under the
    /// version directory, `/` → `.`, with a trailing `init` dropped
    /// (`pl/tablex.lua` → `pl.tablex`, `pl/init.lua` → `pl`), i.e. the name
    /// `luabox_bundle::resolve_module` maps back to this same file.
    pub module: String,
    /// How diagnostics and debug notes name this file: its root-relative
    /// path, forward-slashed.
    pub label: String,
    /// The file on disk — the identity a resolved `require` is matched against.
    pub path: PathBuf,
    /// The file's contents, handed back unjudged (see the module docs).
    pub text: String,
}

/// The `lua_modules/share/lua/<version_dir>/` directory a luarocks tree
/// installs pure-Lua modules into — where `require` resolution looks
/// (`luabox_bundle::resolve_candidates`) and therefore where the type harvest
/// reads. `version_dir` is `luabox_bundle::rocks_version_dir`'s answer for the
/// dialect in play (`"5.4"`, `"5.1"` for LuaJIT).
#[must_use]
pub fn rock_tree_dir(root: &Path, version_dir: &str) -> PathBuf {
    root.join(VENDOR_DIR)
        .join("share")
        .join("lua")
        .join(version_dir)
}

/// Every `*.lua` file installed under [`rock_tree_dir`], read, in the order
/// `require` would try them — the collision-winner order for the type harvest
/// (#30). See the sort inside for why that is not `Vec<PathBuf>::sort`.
///
/// Empty when the project has no luarocks tree for this version directory,
/// which is the scope guard: the harvest requires the versioned
/// `share/lua/<X.Y>/` layout, and the flat `lua_modules/<name>/` layout keeps
/// its existing `[dependencies]` + `[types] defs` path untouched.
///
/// Nothing here fails: an unreadable directory or file simply contributes
/// nothing. A vendored tree is not the project's code, and a permissions
/// problem inside it must never turn into a diagnostic about the project.
#[must_use]
pub fn collect_rock_sources(root: &Path, version_dir: &str) -> Vec<RockSource> {
    let base = rock_tree_dir(root, version_dir);
    if !base.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_rock_lua(&base, &mut files);
    // Ordered by the paths' raw bytes, NOT by `Path`'s own `Ord`.
    //
    // This order is a contract, not a tidiness: the harvest is first-wins per
    // module name (`luabox_types::RockSurfaces`), so whichever of `pl.lua` and
    // `pl/init.lua` comes first here is the file the editor calls `pl` — and
    // `luabox_bundle::resolve_candidates` tries the flat `<rel>.lua` BEFORE
    // `<rel>/init.lua`, which is the file `luabox check` calls `pl`. The two
    // must agree or the same source gets opposite verdicts in CI and in the
    // editor (Shockwave round 3 measured exactly that).
    //
    // `Path: Ord` compares component-wise, so it ranks `pl` against `pl.lua`
    // and puts the DIRECTORY first — inverting the contract. Byte order gets
    // it right for free and on every platform: the separator is `/` (0x2F) on
    // Unix and `\` (0x5C) on Windows, both above `.` (0x2E), so `pl.lua`
    // precedes `pl<sep>init.lua` either way. `OsStr: Ord` is that byte
    // comparison, and needs no lossy `String` round-trip to reach it.
    files.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    files
        .into_iter()
        .filter_map(|path| {
            let module = rock_module_name(&path, &base)?;
            let text = fs::read_to_string(&path).ok()?;
            Some(RockSource {
                module,
                label: display_rel(&path, root),
                path,
                text,
            })
        })
        .collect()
}

/// Collect every `*.lua` file under `dir`, recursively. Unlike
/// [`collect_d_lua`] this takes *all* Lua files: a rock's annotations live in
/// its ordinary sources, which is the whole point of #30.
///
/// Symlinked directories are NOT descended ([`is_real_dir`]): a rock tree is
/// third-party content walked unconditionally on every `check`/`lint`/
/// `build`, and a link back up the tree (`pkg/up -> ..`) would otherwise
/// recurse until the accumulated path tripped `ENAMETOOLONG` — termination by
/// filesystem accident rather than by design. A symlinked *file* is still
/// taken: only the cycle vector is closed.
fn collect_rock_lua(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_real_dir(&entry) {
            collect_rock_lua(&path, out);
        } else if path.extension().and_then(OsStr::to_str) == Some("lua") {
            out.push(path);
        }
    }
}

/// True for a directory entry that is a directory in its own right — not a
/// symlink to one. `Path::is_dir()` follows links; `entry.file_type()` is the
/// symlink-aware (and cheaper — no extra stat) test the walks above need to
/// stay cycle-free without a visited set.
fn is_real_dir(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
}

/// The dotted `require` name a rock source answers to: its path relative to
/// the version directory `base`, without the `.lua` extension, `/` → `.`, and
/// with a trailing `init` segment dropped (`pl/init.lua` → `pl`) — the inverse
/// of `luabox_bundle::resolve_candidates`' luarocks-tree mapping.
///
/// `None` for a path outside `base` or one whose components are not UTF-8: a
/// name that cannot be spelled cannot be `require`d either.
fn rock_module_name(path: &Path, base: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let mut segments: Vec<&str> = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(name) => segments.push(name.to_str()?),
            _ => return None,
        }
    }
    let last = segments.pop()?;
    let stem = last.strip_suffix(".lua")?;
    // `pl/init.lua` is `require "pl"`; a bare `init.lua` at the tree root has
    // no parent to name it, so it keeps its own.
    let names_its_parent = stem == "init" && !segments.is_empty();
    if !names_its_parent {
        segments.push(stem);
    }
    Some(segments.join("."))
}

/// Collect every `*.d.lua` file under `dir`, recursively. An unreadable
/// directory contributes nothing rather than failing: definition packages are
/// an additive layer, and a missing one is already reported by its caller as
/// an unresolved name. Symlinked directories are not descended — same cycle
/// guard as [`collect_rock_lua`], same rationale.
pub fn collect_d_lua(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_real_dir(&entry) {
            collect_d_lua(&path, out);
        } else if is_def_file(&path) {
            out.push(path);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;

    const MINIMAL_MANIFEST: &str = "\
[package]
name = \"fixture\"
version = \"0.1.0\"
edition = \"5.4\"
";

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }

    /// A `luabox.toml` body with the given extra tables appended.
    fn manifest_text(extra: &str) -> String {
        format!("{MINIMAL_MANIFEST}{extra}")
    }

    fn manifest_of(text: &str) -> Manifest {
        Manifest::parse(text).expect("manifest parses")
    }

    /// The root-relative, forward-slashed paths of a walk result.
    fn rel_all(files: &[PathBuf], root: &Path) -> Vec<String> {
        files.iter().map(|p| display_rel(p, root)).collect()
    }

    // --- discovery --------------------------------------------------------

    #[test]
    fn find_manifest_dir_returns_the_directory_holding_the_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        assert_eq!(
            find_manifest_dir(tmp.path()).expect("found"),
            tmp.path().to_path_buf()
        );
    }

    #[test]
    fn find_manifest_dir_walks_up_from_a_nested_subdirectory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        let nested = tmp.path().join("src").join("deep").join("deeper");
        fs::create_dir_all(&nested).expect("mkdir");

        assert_eq!(
            find_manifest_dir(&nested).expect("found by walking up"),
            tmp.path().to_path_buf()
        );
    }

    #[test]
    fn find_manifest_dir_stops_at_the_nearest_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        write(tmp.path(), "inner/luabox.toml", MINIMAL_MANIFEST);
        let inner = tmp.path().join("inner");

        assert_eq!(find_manifest_dir(&inner).expect("found"), inner);
    }

    #[test]
    fn find_manifest_dir_ignores_a_directory_named_luabox_toml() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(tmp.path().join("proj").join("luabox.toml")).expect("mkdir");
        // `is_file()` — a *directory* by that name is not a manifest, and the
        // walk continues past it rather than claiming a bogus root.
        assert!(find_manifest_dir(&tmp.path().join("proj")).is_none());
    }

    #[test]
    fn read_manifest_parses_a_valid_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        let manifest = read_manifest(tmp.path()).expect("parses");
        assert_eq!(manifest.package.name, "fixture");
        assert_eq!(manifest.package.edition, crate::model::DialectId::Lua54);
    }

    #[test]
    fn read_manifest_reports_an_unreadable_manifest_by_path_with_the_io_cause() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = read_manifest(tmp.path()).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("cannot read"), "{rendered}");
        assert!(rendered.contains("luabox.toml"), "{rendered}");
        // The io failure is the error's `source`, so a frontend printing a
        // `Caused by:` chain still says *why* it could not be read.
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn read_manifest_reports_a_malformed_manifest_with_the_rendered_parse_errors() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "[package]\nname = 42\n");
        let error = read_manifest(tmp.path()).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.starts_with("invalid `"), "{rendered}");
        assert!(rendered.contains("luabox.toml"), "{rendered}");
        // The rendered parse errors follow on their own line(s).
        assert!(rendered.contains('\n'), "{rendered}");
        // A validation failure has no underlying cause to chain to — the
        // errors *are* the message.
        assert!(std::error::Error::source(&error).is_none());
    }

    #[test]
    fn discover_manifest_yields_none_when_no_ancestor_has_a_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let nested = tmp.path().join("a").join("b");
        fs::create_dir_all(&nested).expect("mkdir");
        // A tempdir's ancestors are real directories, so this only holds
        // while no ancestor happens to carry a manifest — true for /tmp.
        assert!(discover_manifest(&nested).expect("no error").is_none());
    }

    #[test]
    fn discover_manifest_yields_the_root_and_parsed_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", MINIMAL_MANIFEST);
        write(tmp.path(), "src/main.lua", "return 0\n");

        let (root, manifest) = discover_manifest(&tmp.path().join("src"))
            .expect("no error")
            .expect("found");
        assert_eq!(root, tmp.path().to_path_buf());
        assert_eq!(manifest.package.name, "fixture");
    }

    #[test]
    fn discover_manifest_propagates_a_malformed_present_manifest_as_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "this is not toml = = =\n");
        let error = discover_manifest(tmp.path()).unwrap_err().to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    // --- the source walk --------------------------------------------------

    #[test]
    fn collect_lua_files_walks_depth_first_in_name_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "z.lua", "");
        write(tmp.path(), "a.lua", "");
        write(tmp.path(), "b/inner.lua", "");
        write(tmp.path(), "b/a_inner.lua", "");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(
            rel_all(&files, tmp.path()),
            ["a.lua", "b/a_inner.lua", "b/inner.lua", "z.lua"]
        );
    }

    #[test]
    fn collect_lua_files_skips_dot_directories_dot_files_and_non_lua_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        write(tmp.path(), "README.md", "");
        write(tmp.path(), "src/notes.txt", "");
        write(tmp.path(), ".git/hooks.lua", "");
        write(tmp.path(), ".hidden.lua", "");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(rel_all(&files, tmp.path()), ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_the_build_output_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        write(tmp.path(), "dist/src/main.lua", "");

        let out = tmp.path().join("dist");
        let files = collect_lua_files(tmp.path(), Some(&out), DefFiles::Include).expect("walk");
        assert_eq!(rel_all(&files, tmp.path()), ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_the_vendored_lua_modules_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        // A real `luarocks install --tree lua_modules` layout: Lua modules
        // under share/lua/<X.Y>/, C modules under lib/lua/<X.Y>/, rock
        // metadata under lib/luarocks/.
        write(tmp.path(), "lua_modules/share/lua/5.4/pl/tablex.lua", "");
        write(tmp.path(), "lua_modules/share/lua/5.4/pl/init.lua", "");
        write(
            tmp.path(),
            "lua_modules/lib/luarocks/rocks-5.4/pl/spec.lua",
            "",
        );
        // …and the older flat layout, which is skipped just the same.
        write(tmp.path(), "lua_modules/pkg/src/init.lua", "");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(rel_all(&files, tmp.path()), ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_skips_a_nested_lua_modules_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "packages/core/src/main.lua", "");
        // A vendored tree can nest — a workspace member has its own, and a
        // rock may vendor one in turn. Every depth is skipped.
        write(tmp.path(), "packages/core/lua_modules/dep/init.lua", "");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.1/x/lua_modules/inner.lua",
            "",
        );

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(rel_all(&files, tmp.path()), ["packages/core/src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_keeps_a_file_merely_named_lua_modules() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The exclusion is by *directory* component; a source file that
        // happens to be called `lua_modules.lua` is first-party.
        write(tmp.path(), "src/lua_modules.lua", "");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(rel_all(&files, tmp.path()), ["src/lua_modules.lua"]);
    }

    #[test]
    fn collect_lua_files_excludes_d_lua_only_when_asked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/love.d.lua", "");
        write(tmp.path(), "src/main.lua", "");

        let included = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(
            rel_all(&included, tmp.path()),
            ["defs/love.d.lua", "src/main.lua"]
        );

        let excluded = collect_lua_files(tmp.path(), None, DefFiles::Exclude).expect("walk");
        assert_eq!(rel_all(&excluded, tmp.path()), ["src/main.lua"]);
    }

    #[test]
    fn collect_lua_files_reports_an_unreadable_directory_by_path() {
        let missing = Path::new("definitely-not-a-directory-xyzzy");
        let error = collect_lua_files(missing, None, DefFiles::Include).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("cannot read directory"), "{rendered}");
        assert!(std::error::Error::source(&error).is_some());
    }

    /// A symlink cycle in the project's own tree (`src/loop -> <root>`) must
    /// be a non-event, exactly as it is for the two rock/defs walks: the
    /// walk skips symlinked directories by design rather than spiralling
    /// until the kernel's symlink budget runs out. Before the guard,
    /// `path.is_dir()` followed the link and the walk re-collected
    /// `src/main.lua` once per level — 41 copies of one file on Linux
    /// (`MAXSYMLINKS`), and 41 diagnostics for one mistake.
    #[cfg(unix)]
    #[test]
    fn collect_lua_files_does_not_descend_symlinked_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "return 1\n");
        // The cycle: src/loop points back at the project root.
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("src/loop")).expect("symlink dir");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(
            rel_all(&files, tmp.path()),
            ["src/main.lua"],
            "the cycle contributes nothing and the file is reported once"
        );
    }

    /// The other half of the sibling contract: only the cycle vector is
    /// closed, so a symlinked *file* is still project source.
    #[cfg(unix)]
    #[test]
    fn collect_lua_files_still_takes_a_symlinked_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/real.lua", "return 1\n");
        write(tmp.path(), "elsewhere.lua.txt", "return 2\n");
        std::os::unix::fs::symlink(
            tmp.path().join("elsewhere.lua.txt"),
            tmp.path().join("src/alias.lua"),
        )
        .expect("symlink file");

        let files = collect_lua_files(tmp.path(), None, DefFiles::Include).expect("walk");
        assert_eq!(
            rel_all(&files, tmp.path()),
            ["src/alias.lua", "src/real.lua"]
        );
    }

    #[test]
    fn every_walked_file_satisfies_the_path_level_predicate() {
        // The walk prunes directories; `is_project_source` judges one path.
        // They are one rule, so the walk must never yield a path the
        // predicate rejects — this is what lets `watch` reuse the predicate.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "src/main.lua", "");
        write(tmp.path(), "defs/love.d.lua", "");
        write(tmp.path(), "lua_modules/dep/init.lua", "");
        write(tmp.path(), ".hidden/x.lua", "");
        write(tmp.path(), "dist/out.lua", "");

        let out = tmp.path().join("dist");
        let files = collect_lua_files(tmp.path(), Some(&out), DefFiles::Include).expect("walk");
        assert_eq!(
            rel_all(&files, tmp.path()),
            ["defs/love.d.lua", "src/main.lua"]
        );
        assert!(
            files
                .iter()
                .all(|p| is_project_source(p, tmp.path(), Some(&out)))
        );
    }

    // --- the path-level predicate ----------------------------------------

    #[test]
    fn is_project_source_accepts_a_lua_file_under_the_root() {
        let root = Path::new("/proj");
        assert!(is_project_source(
            Path::new("/proj/src/foo.lua"),
            root,
            None
        ));
        assert!(is_project_source(
            Path::new("/proj/defs/love.d.lua"),
            root,
            None
        ));
    }

    #[test]
    fn is_project_source_rejects_every_non_lua_file() {
        let root = Path::new("/proj");
        assert!(!is_project_source(Path::new("/proj/README.md"), root, None));
        assert!(!is_project_source(
            Path::new("/proj/luabox.toml"),
            root,
            None
        ));
        assert!(!is_project_source(Path::new("/proj/src/noext"), root, None));
    }

    #[test]
    fn is_project_source_rejects_hidden_vendored_and_emitted_paths() {
        let root = Path::new("/proj");
        let out = Path::new("/proj/dist");
        assert!(!is_project_source(
            Path::new("/proj/.git/x.lua"),
            root,
            None
        ));
        assert!(!is_project_source(
            Path::new("/proj/.hidden.lua"),
            root,
            None
        ));
        assert!(!is_project_source(
            Path::new("/proj/lua_modules/dep/init.lua"),
            root,
            None
        ));
        assert!(!is_project_source(
            Path::new("/proj/src/lua_modules/inner.lua"),
            root,
            None
        ));
        assert!(!is_project_source(
            Path::new("/proj/dist/bundle.lua"),
            root,
            Some(out)
        ));
        // A same-named file elsewhere is unaffected.
        assert!(is_project_source(
            Path::new("/proj/src/dist.lua"),
            root,
            Some(out)
        ));
    }

    #[test]
    fn a_path_outside_the_root_is_judged_on_its_own_name() {
        // `strip_prefix` fails for a path outside the root, so the component
        // rules have nothing to apply to and the extension decides.
        let root = Path::new("/proj");
        assert!(is_project_source(Path::new("/elsewhere/a.lua"), root, None));
        assert!(!is_project_source(Path::new("/elsewhere/a.md"), root, None));
    }

    #[test]
    fn is_in_project_tree_accepts_a_non_lua_file_the_source_predicate_rejects() {
        // The manifest is in the tree but is not source — `watch` needs both
        // halves separately.
        let root = Path::new("/proj");
        assert!(is_in_project_tree(
            Path::new("/proj/luabox.toml"),
            root,
            None
        ));
        assert!(!is_in_project_tree(
            Path::new("/proj/.git/luabox.toml"),
            root,
            None
        ));
    }

    #[test]
    fn display_rel_strips_the_root_and_normalizes_to_forward_slashes() {
        let root = Path::new("/proj");
        assert_eq!(
            display_rel(&root.join("src").join("a.lua"), root),
            "src/a.lua"
        );
        // A path outside the root is returned as-is rather than erroring.
        assert_eq!(
            display_rel(Path::new("/elsewhere/b.lua"), root),
            "/elsewhere/b.lua"
        );
    }

    // --- `[types] defs` resolution ----------------------------------------

    #[test]
    fn a_defs_entry_resolves_to_a_single_d_lua_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "defs/mylib.d.lua",
            "---@meta\n---@class MyLib\nmylib = {}\n",
        );
        let (defs, unresolved) = resolve_project_defs(tmp.path(), &["mylib".to_owned()]);
        assert!(unresolved.is_empty(), "{unresolved:?}");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].label, "defs/mylib.d.lua");
        assert!(defs[0].text.contains("---@class MyLib"));
    }

    #[test]
    fn a_defs_entry_resolves_to_every_d_lua_file_under_a_directory_sorted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/pack/z.d.lua", "---@meta\n");
        write(tmp.path(), "defs/pack/a.d.lua", "---@meta\n");
        write(tmp.path(), "defs/pack/nested/m.d.lua", "---@meta\n");
        // Not a definition file — never picked up.
        write(tmp.path(), "defs/pack/plain.lua", "return 0\n");

        let (defs, unresolved) = resolve_project_defs(tmp.path(), &["pack".to_owned()]);
        assert!(unresolved.is_empty(), "{unresolved:?}");
        let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "defs/pack/a.d.lua",
                "defs/pack/nested/m.d.lua",
                "defs/pack/z.d.lua"
            ]
        );
    }

    #[test]
    fn a_single_file_and_a_directory_of_the_same_name_both_contribute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/both.d.lua", "---@meta\n");
        write(tmp.path(), "defs/both/extra.d.lua", "---@meta\n");
        let (defs, unresolved) = resolve_project_defs(tmp.path(), &["both".to_owned()]);
        assert!(unresolved.is_empty(), "{unresolved:?}");
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn a_defs_entry_matching_neither_a_file_nor_a_directory_is_reported_unresolved() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (defs, unresolved) =
            resolve_project_defs(tmp.path(), &["ghost".to_owned(), "spook".to_owned()]);
        assert!(defs.is_empty());
        // Declaration order, so the frontend's diagnostics come out stable.
        assert_eq!(unresolved, ["ghost", "spook"]);
    }

    // --- dependency-contributed defs (#108) -------------------------------

    #[test]
    fn a_path_dependency_contributes_its_own_defs_to_the_consumer() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "vendor/geometry/luabox.toml",
            &manifest_text("\n[types]\ndefs = [\"geometry\"]\n"),
        );
        write(
            tmp.path(),
            "vendor/geometry/defs/geometry.d.lua",
            "---@meta\n---@class geometry.Shape\n",
        );

        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\ngeometry = { path = \"vendor/geometry\" }\n",
        ));
        let defs = resolve_dep_defs(tmp.path(), &manifest);
        assert_eq!(defs.len(), 1);
        // The label is dependency-prefixed and forward-slashed.
        assert_eq!(defs[0].label, "geometry/defs/geometry.d.lua");
        assert!(defs[0].text.contains("geometry.Shape"));
    }

    #[test]
    fn a_non_path_dependency_is_read_from_the_lua_modules_rock_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/lpeg/luabox.toml",
            &manifest_text("\n[types]\ndefs = [\"lpeg\"]\n"),
        );
        write(
            tmp.path(),
            "lua_modules/lpeg/defs/lpeg.d.lua",
            "---@meta\nlpeg = {}\n",
        );

        let manifest = manifest_of(&manifest_text("\n[dependencies]\nlpeg = \"1.0\"\n"));
        let defs = resolve_dep_defs(tmp.path(), &manifest);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].label, "lpeg/defs/lpeg.d.lua");
    }

    #[test]
    fn dependency_defs_are_ordered_alphabetically_across_both_dependency_tables() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for name in ["alpha", "zeta"] {
            write(
                tmp.path(),
                &format!("vendor/{name}/luabox.toml"),
                &manifest_text(&format!("\n[types]\ndefs = [\"{name}\"]\n")),
            );
            write(
                tmp.path(),
                &format!("vendor/{name}/defs/{name}.d.lua"),
                "---@meta\n",
            );
        }

        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\nzeta = { path = \"vendor/zeta\" }\n\
             \n[dev-dependencies]\nalpha = { path = \"vendor/alpha\" }\n",
        ));
        let labels: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.label)
            .collect();
        assert_eq!(labels, ["alpha/defs/alpha.d.lua", "zeta/defs/zeta.d.lua"]);
    }

    #[test]
    fn a_dependency_directory_defs_package_contributes_every_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "vendor/pack/luabox.toml",
            &manifest_text("\n[types]\ndefs = [\"api\"]\n"),
        );
        write(tmp.path(), "vendor/pack/defs/api/b.d.lua", "---@meta\n");
        write(tmp.path(), "vendor/pack/defs/api/a.d.lua", "---@meta\n");

        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\npack = { path = \"vendor/pack\" }\n",
        ));
        let labels: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.label)
            .collect();
        assert_eq!(labels, ["pack/defs/api/a.d.lua", "pack/defs/api/b.d.lua"]);
    }

    #[test]
    fn a_dependency_that_is_not_materialized_contributes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\nmissing = \"1.0\"\nalso-missing = { path = \"nowhere\" }\n",
        ));
        assert!(resolve_dep_defs(tmp.path(), &manifest).is_empty());
    }

    #[test]
    fn a_dependency_with_an_unparseable_manifest_contributes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "vendor/broken/luabox.toml", "= = =\n");
        write(tmp.path(), "vendor/broken/defs/broken.d.lua", "---@meta\n");

        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\nbroken = { path = \"vendor/broken\" }\n",
        ));
        assert!(resolve_dep_defs(tmp.path(), &manifest).is_empty());
    }

    #[test]
    fn dependency_defs_resolution_is_one_level_deep_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "vendor/mid/luabox.toml",
            &manifest_text(
                "\n[types]\ndefs = [\"mid\"]\n\n[dependencies]\ndeep = { path = \"../deep\" }\n",
            ),
        );
        write(tmp.path(), "vendor/mid/defs/mid.d.lua", "---@meta\n");
        write(
            tmp.path(),
            "vendor/deep/luabox.toml",
            &manifest_text("\n[types]\ndefs = [\"deep\"]\n"),
        );
        write(tmp.path(), "vendor/deep/defs/deep.d.lua", "---@meta\n");

        let manifest = manifest_of(&manifest_text(
            "\n[dependencies]\nmid = { path = \"vendor/mid\" }\n",
        ));
        let labels: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.label)
            .collect();
        // `deep` is a transitive dependency: its defs do not transit.
        assert_eq!(labels, ["mid/defs/mid.d.lua"]);
    }

    #[test]
    fn dep_def_label_prefixes_the_dependency_name_and_forward_slashes_the_rest() {
        let dep_root = Path::new("/tmp/proj/vendor/geometry");
        let file = dep_root.join("defs").join("geometry.d.lua");
        assert_eq!(
            dep_def_label("geometry", &file, dep_root),
            "geometry/defs/geometry.d.lua"
        );
    }

    #[test]
    fn dep_def_label_falls_back_to_the_whole_path_when_it_is_not_under_the_dep_root() {
        let label = dep_def_label("dep", Path::new("/elsewhere/x.d.lua"), Path::new("/root"));
        assert_eq!(label, "dep//elsewhere/x.d.lua");
    }

    // --- installed rock sources (#30) -------------------------------------

    #[test]
    fn rock_tree_dir_names_the_versioned_share_directory() {
        assert_eq!(
            rock_tree_dir(Path::new("/proj"), "5.4"),
            Path::new("/proj/lua_modules/share/lua/5.4")
        );
    }

    #[test]
    fn collect_rock_sources_yields_module_names_labels_and_text_in_require_candidate_order() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl/tablex.lua",
            "---@meta-ish\nreturn 1\n",
        );
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl/init.lua",
            "return 2\n",
        );
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/inifile.lua",
            "return 3\n",
        );

        let rocks = collect_rock_sources(tmp.path(), "5.4");
        let named: Vec<(&str, &str)> = rocks
            .iter()
            .map(|r| (r.module.as_str(), r.label.as_str()))
            .collect();
        // Byte-sorted — the deterministic first-wins order the harvest relies
        // on, and the order `resolve_candidates` tries.
        assert_eq!(
            named,
            [
                ("inifile", "lua_modules/share/lua/5.4/inifile.lua"),
                ("pl", "lua_modules/share/lua/5.4/pl/init.lua"),
                ("pl.tablex", "lua_modules/share/lua/5.4/pl/tablex.lua"),
            ]
        );
        assert_eq!(rocks[0].text, "return 3\n");
        assert_eq!(
            rocks[0].path,
            tmp.path().join("lua_modules/share/lua/5.4/inifile.lua")
        );
    }

    /// The collision the whole sort exists for (Shockwave round 3).
    ///
    /// `pl.lua` and `pl/init.lua` both answer to the module `pl`, and the
    /// harvest is first-wins, so whichever comes back first here is the file
    /// the editor calls `pl`. `luabox_bundle::resolve_candidates` tries the
    /// flat `<rel>.lua` first, which is the file `luabox check` calls `pl`.
    /// `Vec<PathBuf>::sort` ranked the DIRECTORY first (`Path: Ord` is
    /// component-wise, and `pl` < `pl.lua`), so `check` and the LSP resolved
    /// the same `require` to different files and gave opposite verdicts on the
    /// same source.
    ///
    /// The older test above never caught it: its tree has no flat `pl.lua` to
    /// collide with `pl/init.lua`.
    #[test]
    fn a_flat_module_beats_its_init_form_the_way_require_would() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl/init.lua",
            "return \"init\"\n",
        );
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl.lua",
            "return \"flat\"\n",
        );
        // A second collision one level down, so the rule is shown to be about
        // the `<rel>.lua` / `<rel>/init.lua` pair, not about the tree root.
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl/tablex/init.lua",
            "return \"deep-init\"\n",
        );
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pl/tablex.lua",
            "return \"deep-flat\"\n",
        );

        let rocks = collect_rock_sources(tmp.path(), "5.4");
        // Every module named, in order: the flat form of each colliding pair
        // comes first, so first-wins picks it.
        let named: Vec<(&str, &str)> = rocks
            .iter()
            .map(|r| (r.module.as_str(), r.text.trim()))
            .collect();
        assert_eq!(
            named,
            [
                ("pl", "return \"flat\""),
                ("pl", "return \"init\""),
                ("pl.tablex", "return \"deep-flat\""),
                ("pl.tablex", "return \"deep-init\""),
            ]
        );
    }

    /// A symlink cycle in a rock tree (`pkg/up -> ..`) must be a non-event:
    /// the walk skips symlinked directories by design, not by running the
    /// path into `ENAMETOOLONG`. A symlinked *file* is still harvested.
    #[cfg(unix)]
    #[test]
    fn collect_rock_sources_does_not_descend_symlinked_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/pkg/real.lua",
            "---@class pkg.T\nreturn 1\n",
        );
        write(tmp.path(), "elsewhere/linked.lua", "return 2\n");
        let pkg = tmp.path().join("lua_modules/share/lua/5.4/pkg");
        // The cycle: pkg/up -> the version dir's parent.
        std::os::unix::fs::symlink("..", pkg.join("up")).expect("symlink dir");
        // A symlinked file, which must still be taken.
        std::os::unix::fs::symlink(
            tmp.path().join("elsewhere/linked.lua"),
            pkg.join("alias.lua"),
        )
        .expect("symlink file");

        let rocks = collect_rock_sources(tmp.path(), "5.4");
        let modules: Vec<&str> = rocks.iter().map(|r| r.module.as_str()).collect();
        assert_eq!(
            modules,
            ["pkg.alias", "pkg.real"],
            "the cycle contributes nothing, the linked file still counts"
        );
    }

    /// Same guard on the defs walk — `collect_d_lua` shares the shape.
    #[cfg(unix)]
    #[test]
    fn collect_d_lua_does_not_descend_symlinked_directories() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/real.d.lua", "---@meta\n");
        std::os::unix::fs::symlink("..", tmp.path().join("defs/up")).expect("symlink");

        let mut out = Vec::new();
        collect_d_lua(&tmp.path().join("defs"), &mut out);
        assert_eq!(out.len(), 1, "one real def file, no cycle traversal");
    }

    #[test]
    fn collect_rock_sources_is_empty_without_a_versioned_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The flat layout keeps its existing defs-based path: no harvest.
        write(tmp.path(), "lua_modules/pkg/src/init.lua", "return 1\n");
        // …and so does a tree installed for another interpreter version.
        write(
            tmp.path(),
            "lua_modules/share/lua/5.1/legacy.lua",
            "return 1\n",
        );
        assert!(collect_rock_sources(tmp.path(), "5.4").is_empty());
    }

    #[test]
    fn collect_rock_sources_ignores_non_lua_files_and_c_modules() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "lua_modules/share/lua/5.4/ok.lua", "return 1\n");
        write(tmp.path(), "lua_modules/share/lua/5.4/README.md", "hi\n");
        write(tmp.path(), "lua_modules/lib/lua/5.4/lfs.so", "binary\n");
        write(
            tmp.path(),
            "lua_modules/lib/luarocks/rocks-5.4/pl/spec.lua",
            "return 1\n",
        );

        let modules: Vec<String> = collect_rock_sources(tmp.path(), "5.4")
            .into_iter()
            .map(|r| r.module)
            .collect();
        assert_eq!(modules, ["ok"]);
    }

    #[test]
    fn collect_rock_sources_takes_a_deeply_nested_module() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/a/b/c/d.lua",
            "return 1\n",
        );
        let rocks = collect_rock_sources(tmp.path(), "5.4");
        assert_eq!(rocks.len(), 1);
        assert_eq!(rocks[0].module, "a.b.c.d");
    }

    #[test]
    fn a_bare_init_lua_at_the_tree_root_keeps_its_own_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/init.lua",
            "return 1\n",
        );
        let rocks = collect_rock_sources(tmp.path(), "5.4");
        // Nothing to name it after — the `init` drop needs a parent segment.
        assert_eq!(rocks[0].module, "init");
    }

    #[test]
    fn rock_module_name_rejects_a_path_outside_the_tree_or_a_non_lua_file() {
        let base = Path::new("/proj/lua_modules/share/lua/5.4");
        assert!(rock_module_name(Path::new("/elsewhere/x.lua"), base).is_none());
        assert!(rock_module_name(&base.join("notes.txt"), base).is_none());
        // The base itself has no trailing file segment to name.
        assert!(rock_module_name(base, base).is_none());
    }

    #[test]
    fn collect_rock_lua_on_a_missing_directory_yields_nothing() {
        let mut found = Vec::new();
        collect_rock_lua(Path::new("no-such-rock-tree-xyzzy"), &mut found);
        assert!(found.is_empty());
    }

    #[test]
    fn collect_d_lua_recurses_and_takes_only_d_lua_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "a.d.lua", "");
        write(tmp.path(), "plain.lua", "");
        write(tmp.path(), "notes.txt", "");
        write(tmp.path(), "nested/b.d.lua", "");

        let mut found = Vec::new();
        collect_d_lua(tmp.path(), &mut found);
        found.sort();
        assert_eq!(rel_all(&found, tmp.path()), ["a.d.lua", "nested/b.d.lua"]);
    }

    #[test]
    fn collect_d_lua_on_a_missing_directory_yields_nothing() {
        let mut found = Vec::new();
        collect_d_lua(Path::new("no-such-directory-xyzzy"), &mut found);
        assert!(found.is_empty());
    }
}
