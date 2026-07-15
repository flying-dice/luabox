//! `luabox add/remove/install/update/vendor` — dependency management
//! (SPEC.md §4, §6).
//!
//! # The two manifests
//!
//! luabox follows the pnpm/bun model: the project's **rockspec**
//! (`*.rockspec`) is the package manifest — it owns the package name,
//! version, and *registry* dependencies (resolved from luarocks.org).
//! `luabox.toml` is tool config plus the *source* dependencies a rockspec
//! cannot express (`path`/`git`/`workspace`). Both are discovered together
//! and fused into one resolvable view by
//! [`luabox_resolve::effective_manifest`].
//!
//! # Command semantics
//!
//! - **add** — comment-preserving `luabox.toml` edit
//!   (`Manifest::set_dependency_entry`) for `--path <dir>` /
//!   `--git <url> [--rev|--tag|--branch]` sources, then an install. A bare
//!   registry spec (`luabox add penlight@1.14`) now belongs in the rockspec;
//!   editing the rockspec arrives in a later wave, so it errors with that
//!   guidance for now.
//! - **remove** — comment-preserving edit (`Manifest::remove_dependency`)
//!   plus a re-install, so `luabox.lock` and `lua_modules/` drop the entry.
//! - **install** — resolve (respecting an existing `luabox.lock` for
//!   minimal churn) over `PathProvider` + `GitProvider` +
//!   `LuaRocksProvider`, write the lockfile, and materialize every non-path
//!   package from the content-addressed store into `lua_modules/<name>/`.
//!   Idempotent: when the lockfile and `lua_modules` are already current it
//!   prints `up to date` and does no work.
//! - **update `[pkg]`** — re-resolve ignoring the lockfile (or just
//!   dropping `pkg`'s pin), re-fetch mutable git refs, rewrite the lock,
//!   re-materialize.
//! - **vendor** — materialize every non-path package as writable *copies*
//!   into `vendor/<name>/`, ready to commit.
//!
//! # Layout and store integration
//!
//! Fetched packages are interned into the global content-addressed store
//! (`LUABOX_STORE` env override, default `~/.luabox/store`) and hard-linked
//! into `<project>/lua_modules/<name>/` — the require-path convention the
//! bundler will consume (`require "name"` ↔ `lua_modules/name/…`; build and
//! require-resolution integration lands with the bundler, SPEC.md §7).
//!
//! Path and workspace dependencies are **not** copied: they are used in
//! place from their source directories (the lockfile records them as
//! `path+…` for the graph, nothing is materialized).

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use luabox_resolve::luarocks::rockspec::{self, Rockspec};
use luabox_resolve::manifest::{Dependency, GitDependency, PathDependency};
use luabox_resolve::{
    GitProvider, LOCKFILE_NAME, Lockfile, LuaRocksProvider, Manifest, PackageId, PathProvider,
    Resolution, ResolveError, Source, StackedProvider, effective_manifest, resolve,
};
use luabox_store::{LinkMode, Store};

/// Directory (under the project root) that installed packages land in.
const MODULES_DIR: &str = "lua_modules";

/// Directory that `luabox vendor` fills with committable copies.
const VENDOR_DIR: &str = "vendor";

/// Parsed arguments of `luabox add`.
pub struct AddOptions {
    /// `name[@version]`.
    pub package: String,
    /// Target `[dev-dependencies]` instead of `[dependencies]`.
    pub dev: bool,
    /// Add as a path dependency rooted at this directory.
    pub path: Option<String>,
    /// Add as a git dependency at this URL.
    pub git: Option<String>,
    pub rev: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
}

/// `luabox add`: manifest edit + install.
pub fn add(cwd: &Path, opts: &AddOptions) -> anyhow::Result<()> {
    let mut project = discover(cwd)?;
    let (name, version) = split_spec(&opts.package)?;

    let dep = if let Some(url) = &opts.git {
        Dependency::Git(GitDependency {
            git: url.clone(),
            rev: opts.rev.clone(),
            tag: opts.tag.clone(),
            branch: opts.branch.clone(),
            version: version.clone(),
        })
    } else if let Some(path) = &opts.path {
        Dependency::Path(PathDependency {
            // Manifest paths are conventionally forward-slashed.
            path: path.replace('\\', "/"),
            version: version.clone(),
        })
    } else {
        // A bare registry spec is a luarocks.org dependency, and those live
        // in the project's rockspec now (pnpm-style, SPEC.md §6). Editing the
        // rockspec from `luabox add` lands in a later wave; until then,
        // declare it in the rockspec by hand.
        bail!(
            "`{name}` is a registry (luarocks.org) dependency — declare it in your \
             project's rockspec `dependencies` for now (rockspec editing from \
             `luabox add` arrives in a later wave). `--path <dir>` and \
             `--git <url>` dependencies are written to `luabox.toml` and work today"
        );
    };

    project.manifest.set_dependency_entry(&name, &dep, opts.dev);
    fs::write(&project.manifest_path, project.manifest.to_string())
        .with_context(|| format!("cannot write `{}`", project.manifest_path.display()))?;
    let table = if opts.dev {
        "dev-dependencies"
    } else {
        "dependencies"
    };
    println!("added `{name}` to [{table}]");

    sync(&project, &LockUse::Full, false)
}

/// `luabox remove`: manifest edit + re-install (lockfile updated).
pub fn remove(cwd: &Path, package: &str) -> anyhow::Result<()> {
    let mut project = discover(cwd)?;
    if !project.manifest.remove_dependency(package) {
        bail!(
            "no dependency named `{package}` in `{}`",
            project.manifest_path.display()
        );
    }
    fs::write(&project.manifest_path, project.manifest.to_string())
        .with_context(|| format!("cannot write `{}`", project.manifest_path.display()))?;
    println!("removed `{package}`");

    sync(&project, &LockUse::Full, false)
}

/// `luabox install`: resolve against the existing lockfile (minimal churn),
/// write `luabox.lock`, materialize into `lua_modules/`.
pub fn install(cwd: &Path) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    sync(&project, &LockUse::Full, false)
}

/// `luabox update [pkg]`: re-pin git dependencies to their repo's latest
/// GitHub release tag, then re-resolve ignoring the lock (or just `pkg`'s pin),
/// refreshing mutable git references.
///
/// The re-pin is the GUI "Update" button's clean call: a **tag**-pinned git dep
/// is rewritten (`Manifest::set_dependency_entry`, comment-preserving) to the
/// latest release tag of its GitHub repo. A dep pinned by `rev`/`branch` is
/// left untouched — switching its pin kind silently would be surprising — and
/// says so. Non-git deps are untouched by the re-pin and follow the ordinary
/// re-resolve.
pub fn update(cwd: &Path, package: Option<&str>) -> anyhow::Result<()> {
    let mut project = discover(cwd)?;
    let token = crate::github::token();
    repin_git_tags(&mut project, package, token.as_deref())?;
    match package {
        None => sync(&project, &LockUse::Ignore, true),
        Some(name) => sync(&project, &LockUse::Without(name.to_owned()), true),
    }
}

/// A git dependency and which table it lives in — the unit `repin_git_tags`
/// rewrites.
struct GitTarget {
    name: String,
    dev: bool,
    git: GitDependency,
}

/// Re-pins tag-pinned git dependencies (all of them, or just `only` when named)
/// to their GitHub repo's latest release tag, writing `luabox.toml` in place.
/// Prints one line per dependency it considered. Returns the names re-pinned.
fn repin_git_tags(
    project: &mut Project,
    only: Option<&str>,
    token: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut targets = Vec::new();
    for (dev, table) in [
        (false, &project.manifest.dependencies),
        (true, &project.manifest.dev_dependencies),
    ] {
        for (name, dep) in table {
            if only.is_some_and(|wanted| wanted != name) {
                continue;
            }
            if let Dependency::Git(git) = dep {
                targets.push(GitTarget {
                    name: name.clone(),
                    dev,
                    git: git.clone(),
                });
            }
        }
    }

    let mut repinned = Vec::new();
    for target in targets {
        let Some(tag) = &target.git.tag else {
            println!(
                "leaving `{}`: pinned by {}, not a tag — re-pin manually to change it",
                target.name,
                if target.git.rev.is_some() {
                    "rev"
                } else {
                    "branch"
                }
            );
            continue;
        };
        let Some(repo) = crate::github::parse_github_repo(&target.git.git) else {
            println!(
                "leaving `{}`: `{}` is not a GitHub repo — cannot look up its latest release",
                target.name, target.git.git
            );
            continue;
        };
        let Some(latest) = crate::github::latest_release_tag(&repo, token)? else {
            println!(
                "leaving `{}`: `{repo}` has no releases or tags",
                target.name
            );
            continue;
        };
        if &latest == tag {
            println!("`{}` is already at the latest release {tag}", target.name);
            continue;
        }

        let updated = GitDependency {
            tag: Some(latest.clone()),
            ..target.git.clone()
        };
        project
            .manifest
            .set_dependency_entry(&target.name, &Dependency::Git(updated), target.dev);
        println!("re-pinned `{}` {tag} -> {latest}", target.name);
        repinned.push(target.name);
    }

    if !repinned.is_empty() {
        fs::write(&project.manifest_path, project.manifest.to_string())
            .with_context(|| format!("cannot write `{}`", project.manifest_path.display()))?;
    }
    Ok(repinned)
}

/// `luabox vendor`: writable copies of every non-path dependency under
/// `vendor/<name>/`.
pub fn vendor(cwd: &Path) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let manifest = project.resolved_manifest()?;

    let store = Store::open(store_root()?);
    let git = GitProvider::new(store.root().join("git"));
    let paths = PathProvider::new();
    let luarocks = LuaRocksProvider::from_env(store.root().join("luarocks"));
    let lock = read_lockfile(&project, &LockUse::Full)?;
    let providers: Vec<&dyn luabox_resolve::PackageProvider> = vec![&paths, &git, &luarocks];
    let stacked = StackedProvider::new(providers);
    let resolution = run_resolve(&manifest, &project.root, &stacked, lock.as_ref())?;

    let vendor_dir = project.root.join(VENDOR_DIR);
    let vendored = materialize(&resolution, &store, &git, &luarocks, &vendor_dir, LinkMode::Copy)?;
    if vendored.is_empty() {
        println!(
            "nothing to vendor: every dependency is a path/workspace dependency (used in place)"
        );
    } else {
        println!("vendored {} package(s) into {VENDOR_DIR}/", vendored.len());
        for name in &vendored {
            println!("  {name} -> {VENDOR_DIR}/{name}/");
        }
        println!(
            "to build against the vendored copies, point luabox.toml at them:\n  \
             <name> = {{ path = \"{VENDOR_DIR}/<name>\" }}"
        );
    }
    Ok(())
}

// --- shared machinery ------------------------------------------------------

/// How much of an existing `luabox.lock` a resolve should respect.
enum LockUse {
    /// Keep every still-satisfying pin (install semantics).
    Full,
    /// Ignore the lockfile entirely (`luabox update`).
    Ignore,
    /// Keep every pin except this package's (`luabox update <pkg>`).
    Without(String),
}

/// A discovered project: nearest ancestor directory holding `luabox.toml`,
/// plus its rockspec package manifest when one is present.
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    /// The editable `luabox.toml` (tool config + path/git sources).
    pub(crate) manifest: Manifest,
    /// The project's parsed `*.rockspec` (package name/version + registry
    /// deps), when the root has exactly one.
    pub(crate) rockspec: Option<Rockspec>,
}

impl Project {
    /// The single [`Manifest`] the resolver consumes: `luabox.toml` fused with
    /// the rockspec's name/version/registry deps
    /// ([`luabox_resolve::effective_manifest`]). A registry dep left in
    /// `luabox.toml`, or a name declared in both manifests, surfaces here as a
    /// hard error.
    pub(crate) fn resolved_manifest(&self) -> anyhow::Result<Manifest> {
        effective_manifest(&self.manifest, self.rockspec.as_ref()).map_err(|e| anyhow!("{e}"))
    }
}

/// Resolve + lock + materialize — the shared body of install/add/remove/
/// update. Prints `up to date` and does nothing when a `Full` sync finds
/// the lockfile byte-identical and `lua_modules` complete.
fn sync(project: &Project, lock_use: &LockUse, refresh_git: bool) -> anyhow::Result<()> {
    let manifest = project.resolved_manifest()?;

    let lock_path = project.root.join(LOCKFILE_NAME);
    let existing_text = fs::read_to_string(&lock_path).ok();
    let lock = read_lockfile(project, lock_use)?;

    let store = Store::open(store_root()?);
    let git = GitProvider::new(store.root().join("git")).with_refresh(refresh_git);
    let paths = PathProvider::new();
    let luarocks = LuaRocksProvider::from_env(store.root().join("luarocks"));
    let providers: Vec<&dyn luabox_resolve::PackageProvider> = vec![&paths, &git, &luarocks];
    let stacked = StackedProvider::new(providers);
    let resolution = run_resolve(&manifest, &project.root, &stacked, lock.as_ref())?;

    let new_text = resolution.lockfile.to_toml_string();
    let modules_dir = project.root.join(MODULES_DIR);
    let lock_current = existing_text.as_deref() == Some(new_text.as_str());
    if matches!(lock_use, LockUse::Full)
        && lock_current
        && modules_complete(&resolution, &modules_dir)
    {
        println!("up to date");
        return Ok(());
    }

    if !lock_current {
        fs::write(&lock_path, &new_text)
            .with_context(|| format!("cannot write `{}`", lock_path.display()))?;
    }

    let installed = materialize(&resolution, &store, &git, &luarocks, &modules_dir, LinkMode::Auto)?;
    prune_stale_modules(&resolution, &modules_dir)?;

    let path_count = resolution
        .packages
        .iter()
        .filter(|p| matches!(p.source, Source::Path { .. }))
        .count();
    let mut parts = vec![format!(
        "locked {} package(s) in {LOCKFILE_NAME}",
        resolution.packages.len()
    )];
    if !installed.is_empty() {
        parts.push(format!("installed {} into {MODULES_DIR}/", installed.len()));
    }
    if path_count > 0 {
        parts.push(format!("{path_count} path dependency(ies) used in place"));
    }
    println!("{}", parts.join("; "));
    Ok(())
}

/// Loads `luabox.lock` per `lock_use`. A missing lockfile is fine; an
/// unreadable one is an error (silently re-resolving would churn pins).
fn read_lockfile(project: &Project, lock_use: &LockUse) -> anyhow::Result<Option<Lockfile>> {
    if matches!(lock_use, LockUse::Ignore) {
        return Ok(None);
    }
    let lock_path = project.root.join(LOCKFILE_NAME);
    let Ok(text) = fs::read_to_string(&lock_path) else {
        return Ok(None);
    };
    let mut lock = Lockfile::parse(&text)
        .with_context(|| format!("cannot parse `{}`", lock_path.display()))?;
    if let LockUse::Without(name) = lock_use {
        lock.packages.retain(|p| &p.name != name);
    }
    Ok(Some(lock))
}

/// Runs the PubGrub resolve, translating failures into command errors
/// (conflict reports render via `ResolveError`'s `Display`).
fn run_resolve(
    manifest: &Manifest,
    root: &Path,
    provider: &dyn luabox_resolve::PackageProvider,
    lock: Option<&Lockfile>,
) -> anyhow::Result<Resolution> {
    resolve(manifest, root, provider, lock).map_err(|e| match &e {
        ResolveError::Provider(
            luabox_resolve::ProviderError::UnknownPackage { .. }
            | luabox_resolve::ProviderError::UnsupportedSource { .. },
        ) => anyhow!(
            "{e}\nnote: registry dependencies resolve from luarocks.org (declared in \
             the project's rockspec); check that it provides this rock (SPEC.md §6)"
        ),
        _ => anyhow!("{e}"),
    })
}

/// Interns every non-path package into the store and links it into
/// `dest_root/<name>/`. Path/workspace packages are used in place and
/// skipped. Registry packages are fetched from luarocks.org (or its mirror)
/// as a laid-out module tree. Returns the names materialized.
fn materialize(
    resolution: &Resolution,
    store: &Store,
    git: &GitProvider,
    luarocks: &LuaRocksProvider,
    dest_root: &Path,
    mode: LinkMode,
) -> anyhow::Result<Vec<String>> {
    let mut installed = Vec::new();
    for package in &resolution.packages {
        let tree_dir = match &package.source {
            Source::Path { .. } => continue,
            Source::Git { url, reference } => {
                git.checkout(url, reference)
                    .map_err(|e| anyhow!("fetching `{}`: {e}", package.name))?
                    .dir
            }
            // A registry package is a luarocks.org rock: fetch its laid-out
            // module tree (SPEC.md §6).
            Source::Registry => luarocks
                .fetch(&PackageId::registry(&package.name), &package.version)
                .map_err(|e| anyhow!("fetching `{}`: {e}", package.name))?,
        };
        let tree = store
            .put_tree(&tree_dir)
            .with_context(|| format!("interning `{}` into the store", package.name))?;
        store
            .write_package_manifest(&package.name, &package.version.to_string(), &tree)
            .with_context(|| format!("indexing `{}` in the store", package.name))?;
        let dest = dest_root.join(&package.name);
        remove_all_force(&dest).with_context(|| format!("clearing `{}`", dest.display()))?;
        store
            .materialize(&tree, &dest, mode)
            .with_context(|| format!("materializing `{}`", package.name))?;
        installed.push(package.name.clone());
    }
    Ok(installed)
}

/// Whether every non-path package already has a `lua_modules/<name>/`
/// directory (the cheap idempotency check behind `up to date`).
fn modules_complete(resolution: &Resolution, modules_dir: &Path) -> bool {
    resolution
        .packages
        .iter()
        .filter(|p| !matches!(p.source, Source::Path { .. }))
        .all(|p| modules_dir.join(&p.name).is_dir())
}

/// Drops `lua_modules/` entries that no resolved package claims (e.g.
/// after `luabox remove`). Scoped names (`@org/pkg`) keep their top-level
/// `@org` directory as long as any package still lives under it.
fn prune_stale_modules(resolution: &Resolution, modules_dir: &Path) -> anyhow::Result<()> {
    let expected_top: BTreeSet<&str> = resolution
        .packages
        .iter()
        .filter(|p| !matches!(p.source, Source::Path { .. }))
        .filter_map(|p| p.name.split('/').next())
        .collect();
    let entries = match fs::read_dir(modules_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading `{}`", modules_dir.display()));
        }
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if !expected_top.contains(name.to_string_lossy().as_ref()) {
            remove_all_force(&entry.path())
                .with_context(|| format!("pruning `{}`", entry.path().display()))?;
        }
    }
    Ok(())
}

// --- project discovery and environment -------------------------------------

/// Nearest `luabox.toml` walking up from `cwd`, cargo-style. Dependency
/// commands (unlike `check`) require a manifest — there is nothing to
/// resolve without one.
pub(crate) fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let (root, manifest) = crate::project::discover_required(cwd)?;
    let rockspec = discover_rockspec(&root)?;
    Ok(Project {
        manifest_path: root.join("luabox.toml"),
        rockspec,
        root,
        manifest,
    })
}

/// The project's rockspec package manifest: the sole `<root>/*.rockspec`,
/// parsed statically. `None` when there is none; more than one is an error
/// (the project's package identity would be ambiguous).
fn discover_rockspec(root: &Path) -> anyhow::Result<Option<Rockspec>> {
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("reading `{}`", root.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rockspec") {
            found.push(path);
        }
    }
    found.sort();
    match found.as_slice() {
        [] => Ok(None),
        [path] => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("cannot read `{}`", path.display()))?;
            Ok(Some(rockspec::read(&text)))
        }
        many => {
            let names: Vec<String> = many
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            bail!(
                "the project has more than one rockspec ({}) — keep exactly one so the \
                 package's name and version are unambiguous",
                names.join(", ")
            )
        }
    }
}

/// The content-addressed store root: `LUABOX_STORE` env override, else
/// `<home>/.luabox/store`.
pub(crate) fn store_root() -> anyhow::Result<PathBuf> {
    if let Ok(dir) = env::var("LUABOX_STORE")
        && !dir.trim().is_empty()
    {
        return Ok(PathBuf::from(dir));
    }
    Ok(home_dir()?.join(".luabox").join("store"))
}

/// The home directory, or a hard error: the dependency commands must land the
/// store *somewhere*, so a missing home (with no `LUABOX_STORE` override) is
/// fatal. Wraps the shared env probe ([`crate::project::home_dir`]); `audit`
/// wraps the same probe with the opposite, non-fatal contract.
fn home_dir() -> anyhow::Result<PathBuf> {
    crate::project::home_dir().ok_or_else(|| {
        anyhow!("cannot locate a home directory (set HOME, USERPROFILE, or LUABOX_STORE)")
    })
}

/// Splits `name[@version]`. The leading char is exempt so scoped names
/// (`@org/pkg`) keep their sigil.
fn split_spec(spec: &str) -> anyhow::Result<(String, Option<String>)> {
    let mut chars = spec.chars();
    let Some(first) = chars.next() else {
        bail!("invalid package spec ``; expected `name` or `name@version`");
    };
    // Skip the leading char (char-safely — it may be a multi-byte sigil such
    // as `@`) and treat the last `@` in the remainder as the version
    // separator, so `@org/pkg` keeps its scope sigil.
    let Some((name_tail, version)) = chars.as_str().rsplit_once('@') else {
        return Ok((spec.to_owned(), None));
    };
    if version.is_empty() {
        bail!("invalid package spec `{spec}`; expected `name` or `name@version`");
    }
    Ok((format!("{first}{name_tail}"), Some(version.to_owned())))
}

/// `remove_dir_all` that also deletes read-only files (store hard-links
/// share the object's read-only bit, which `std` removal refuses on
/// Windows). Missing paths are fine.
fn remove_all_force(path: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            remove_all_force(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else {
        if meta.permissions().readonly() {
            let mut perms = meta.permissions();
            #[allow(
                clippy::permissions_set_readonly_false,
                reason = "store hard-links inherit the object's read-only bit and Windows \
                          refuses to remove read-only files; the attribute is cleared only \
                          to delete the file on the very next line"
            )]
            perms.set_readonly(false);
            fs::set_permissions(path, perms)?;
        }
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::split_spec;

    #[test]
    fn split_spec_forms() {
        assert_eq!(split_spec("pkg").unwrap(), ("pkg".to_owned(), None));
        assert_eq!(
            split_spec("pkg@1.2").unwrap(),
            ("pkg".to_owned(), Some("1.2".to_owned()))
        );
        assert_eq!(
            split_spec("@org/pkg").unwrap(),
            ("@org/pkg".to_owned(), None)
        );
        assert_eq!(
            split_spec("@org/pkg@^2").unwrap(),
            ("@org/pkg".to_owned(), Some("^2".to_owned()))
        );
        assert!(split_spec("pkg@").is_err());
    }

    #[test]
    fn split_spec_multibyte_first_char() {
        // A spec whose first character is multi-byte UTF-8 must not panic on a
        // char boundary (regression: the leading char was sliced as one byte).
        assert_eq!(split_spec("é").unwrap(), ("é".to_owned(), None));
        assert_eq!(
            split_spec("état@1.0").unwrap(),
            ("état".to_owned(), Some("1.0".to_owned()))
        );
        assert_eq!(split_spec("你好").unwrap(), ("你好".to_owned(), None));
        assert!(split_spec("").is_err());
    }
}
