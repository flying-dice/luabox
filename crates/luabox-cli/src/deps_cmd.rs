//! `luabox add/remove/install/update/vendor` — dependency management
//! (SPEC.md §4, §6).
//!
//! # Command semantics
//!
//! - **add** — comment-preserving `luabox.toml` edit
//!   (`Manifest::set_dependency_entry`), then an install. Sources:
//!   `--path <dir>`, `--git <url> [--rev|--tag|--branch]`, and registry
//!   specs (`luabox add penlight@1.14`) when `LUABOX_REGISTRY` names a
//!   registry (SPEC.md §6; without one, registry specs error with setup
//!   guidance — there is no hosted default registry yet).
//! - **remove** — comment-preserving edit (`Manifest::remove_dependency`)
//!   plus a re-install, so `luabox.lock` and `lua_modules/` drop the entry.
//! - **install** — resolve (respecting an existing `luabox.lock` for
//!   minimal churn) over `PathProvider` + `GitProvider` +
//!   `RegistryProvider` (when configured), write the lockfile, and
//!   materialize every non-path package from the content-addressed store
//!   into `lua_modules/<name>/`. Registry artifacts are fetched as tars,
//!   extracted with the `tar` CLI (the toolchain installer's approach),
//!   interned via `Store::put_tree`, and their tree hash is verified
//!   against the index `checksum` before anything is materialized.
//!   Idempotent: when the lockfile and `lua_modules` are already current
//!   it prints `up to date` and does no work.
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
use std::process::Command;

use anyhow::{Context, anyhow, bail};
use luabox_resolve::manifest::{Dependency, GitDependency, PathDependency};
use luabox_resolve::{
    GitProvider, LOCKFILE_NAME, LUAROCKS_PREFIX, Lockfile, LuaRocksProvider, Manifest, PackageId,
    PathProvider, REGISTRY_ENV, Registry, RegistryProvider, Resolution, ResolveError, Source,
    StackedProvider, resolve,
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
        let Some(registry) = registry_from_env()? else {
            bail!(
                "cannot add `{name}` as a registry dependency: no registry is \
                 configured. Set {REGISTRY_ENV} to your registry's location (a \
                 directory, file:// URL, or https:// base — there is no hosted \
                 default registry yet, SPEC.md §6), or use `--path <dir>` / \
                 `--git <url>`"
            );
        };
        let req = match version {
            Some(req) => req,
            // `luabox add pkg` with no version: default to a caret
            // requirement on the newest published version, cargo-style.
            None => default_registry_req(&registry, &name)?,
        };
        Dependency::Version(req)
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

/// `luabox update [pkg]`: re-resolve ignoring the lock (or just `pkg`'s
/// pin), refreshing mutable git references.
pub fn update(cwd: &Path, package: Option<&str>) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    match package {
        None => sync(&project, &LockUse::Ignore, true),
        Some(name) => sync(&project, &LockUse::Without(name.to_owned()), true),
    }
}

/// `luabox vendor`: writable copies of every non-path dependency under
/// `vendor/<name>/`.
pub fn vendor(cwd: &Path) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let registry = registry_from_env()?;
    ensure_registry_configured(&project.manifest, registry.as_ref())?;

    let store = Store::open(store_root()?);
    let git = GitProvider::new(store.root().join("git"));
    let paths = PathProvider::new();
    let luarocks = LuaRocksProvider::from_env(store.root().join("luarocks"));
    let lock = read_lockfile(&project, &LockUse::Full)?;
    let registry_provider = registry_provider_for(registry.as_ref(), lock.as_ref());
    let mut providers: Vec<&dyn luabox_resolve::PackageProvider> = vec![&paths, &git, &luarocks];
    if let Some(provider) = &registry_provider {
        providers.push(provider);
    }
    let stacked = StackedProvider::new(providers);
    let resolution = run_resolve(&project, &stacked, lock.as_ref())?;

    let vendor_dir = project.root.join(VENDOR_DIR);
    let vendored = materialize(
        &resolution,
        &store,
        &git,
        &luarocks,
        registry.as_ref(),
        &vendor_dir,
        LinkMode::Copy,
    )?;
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

/// A discovered project: nearest ancestor directory holding `luabox.toml`.
/// Shared with `luabox publish` (`crate::publish_cmd`).
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest: Manifest,
}

/// Resolve + lock + materialize — the shared body of install/add/remove/
/// update. Prints `up to date` and does nothing when a `Full` sync finds
/// the lockfile byte-identical and `lua_modules` complete.
fn sync(project: &Project, lock_use: &LockUse, refresh_git: bool) -> anyhow::Result<()> {
    let registry = registry_from_env()?;
    ensure_registry_configured(&project.manifest, registry.as_ref())?;

    let lock_path = project.root.join(LOCKFILE_NAME);
    let existing_text = fs::read_to_string(&lock_path).ok();
    let lock = read_lockfile(project, lock_use)?;

    let store = Store::open(store_root()?);
    let git = GitProvider::new(store.root().join("git")).with_refresh(refresh_git);
    let paths = PathProvider::new();
    let luarocks = LuaRocksProvider::from_env(store.root().join("luarocks"));
    let registry_provider = registry_provider_for(registry.as_ref(), lock.as_ref());
    let mut providers: Vec<&dyn luabox_resolve::PackageProvider> = vec![&paths, &git, &luarocks];
    if let Some(provider) = &registry_provider {
        providers.push(provider);
    }
    let stacked = StackedProvider::new(providers);
    let resolution = run_resolve(project, &stacked, lock.as_ref())?;

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

    let installed = materialize(
        &resolution,
        &store,
        &git,
        &luarocks,
        registry.as_ref(),
        &modules_dir,
        LinkMode::Auto,
    )?;
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
    project: &Project,
    provider: &dyn luabox_resolve::PackageProvider,
    lock: Option<&Lockfile>,
) -> anyhow::Result<Resolution> {
    resolve(&project.manifest, &project.root, provider, lock).map_err(|e| match &e {
        ResolveError::Provider(
            luabox_resolve::ProviderError::UnknownPackage { .. }
            | luabox_resolve::ProviderError::UnsupportedSource { .. },
        ) => anyhow!(
            "{e}\nnote: registry dependencies resolve from the registry named by \
             {REGISTRY_ENV}; check that it provides this package (SPEC.md §6)"
        ),
        _ => anyhow!("{e}"),
    })
}

/// The registry named by `LUABOX_REGISTRY`, if any. `None` simply means no
/// registry is configured (there is no hosted default yet, SPEC.md §6).
pub(crate) fn registry_from_env() -> anyhow::Result<Option<Registry>> {
    match env::var(REGISTRY_ENV) {
        Ok(spec) if !spec.trim().is_empty() => Ok(Some(Registry::open(&spec)?)),
        _ => Ok(None),
    }
}

/// A [`RegistryProvider`] over `registry`, exempting the lockfile's pins
/// from yank filtering (so a project whose locked version was yanked
/// upstream still restores).
fn registry_provider_for(
    registry: Option<&Registry>,
    lock: Option<&Lockfile>,
) -> Option<RegistryProvider> {
    registry.map(|registry| {
        let provider = RegistryProvider::new(registry.clone());
        match lock {
            Some(lock) => provider.with_locked(lock),
            None => provider,
        }
    })
}

/// Registry deps need a configured registry; fail with setup guidance
/// before resolution produces a cryptic `UnknownPackage`.
fn ensure_registry_configured(
    manifest: &Manifest,
    registry: Option<&Registry>,
) -> anyhow::Result<()> {
    if registry.is_some() {
        return Ok(());
    }
    let registry_deps: Vec<&str> = manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
        // `luarocks/<rock>` version deps resolve through the LuaRocks bridge,
        // not the first-party registry — they need no `LUABOX_REGISTRY`.
        .filter(|(name, dep)| {
            matches!(dep, Dependency::Version(_)) && !name.starts_with(LUAROCKS_PREFIX)
        })
        .map(|(name, _)| name.as_str())
        .collect();
    if let Some(first) = registry_deps.first() {
        bail!(
            "`{first}` is a registry dependency, but no registry is configured. \
             Set {REGISTRY_ENV} to your registry's location (a directory, \
             file:// URL, or https:// base — there is no hosted default \
             registry yet, SPEC.md §6), or use a path/git dependency"
        );
    }
    Ok(())
}

/// `^<newest published version>` for `luabox add <pkg>` without a version.
fn default_registry_req(registry: &Registry, name: &str) -> anyhow::Result<String> {
    use luabox_resolve::PackageProvider as _;
    let provider = RegistryProvider::new(registry.clone());
    let versions = provider
        .list_versions(&PackageId::registry(name))
        .map_err(|e| {
            anyhow!(
                "cannot add `{name}`: {e} (registry: `{}`)",
                registry.location()
            )
        })?;
    let newest = versions
        .into_iter()
        .max()
        .ok_or_else(|| anyhow!("`{name}` has no non-yanked versions in the registry"))?;
    Ok(format!("^{newest}"))
}

/// Interns every non-path package into the store and links it into
/// `dest_root/<name>/`. Path/workspace packages are used in place and
/// skipped. Registry packages are fetched from the registry as artifact
/// tars, extracted, and their tree hash is **verified against the index
/// checksum** before materializing. Returns the names materialized.
fn materialize(
    resolution: &Resolution,
    store: &Store,
    git: &GitProvider,
    luarocks: &LuaRocksProvider,
    registry: Option<&Registry>,
    dest_root: &Path,
    mode: LinkMode,
) -> anyhow::Result<Vec<String>> {
    let mut installed = Vec::new();
    for package in &resolution.packages {
        // Keeps a registry package's extraction dir alive until interned
        // (dropped — deleted — at the end of the loop iteration).
        let _staging: tempfile::TempDir;
        let tree_dir = match &package.source {
            Source::Path { .. } => continue,
            Source::Git { url, reference } => {
                git.checkout(url, reference)
                    .map_err(|e| anyhow!("fetching `{}`: {e}", package.name))?
                    .dir
            }
            // A `luarocks/<rock>` package rides on `Source::Registry` (the
            // name prefix routes it to the bridge, SPEC.md §6): fetch the
            // rock's module tree instead of a registry artifact.
            Source::Registry if package.name.starts_with(LUAROCKS_PREFIX) => luarocks
                .fetch(&PackageId::registry(&package.name), &package.version)
                .map_err(|e| anyhow!("fetching `{}`: {e}", package.name))?,
            Source::Registry => {
                let Some(registry) = registry else {
                    bail!(
                        "`{}` is a registry package, but no registry is configured; \
                         set {REGISTRY_ENV} (SPEC.md §6)",
                        package.name
                    );
                };
                let staging = tempfile::tempdir()
                    .context("cannot create a temp dir for the registry artifact")?;
                let version = package.version.to_string();
                let tar = registry
                    .fetch_artifact(&package.name, &version, staging.path())
                    .map_err(|e| anyhow!("fetching `{}`: {e}", package.name))?;
                let tree = staging.path().join("tree");
                _staging = staging;
                fs::create_dir_all(&tree)
                    .with_context(|| format!("cannot create `{}`", tree.display()))?;
                extract_tar(&tar, &tree)
                    .with_context(|| format!("extracting `{}@{version}`", package.name))?;
                tree
            }
        };
        let tree = store
            .put_tree(&tree_dir)
            .with_context(|| format!("interning `{}` into the store", package.name))?;
        // SPEC.md §6: a registry artifact must hash to the checksum its
        // index line promised, or nothing is installed.
        if matches!(package.source, Source::Registry)
            && let Some(expected) = &package.checksum
        {
            let actual = format!("sha256:{}", tree.tree_hash);
            if &actual != expected {
                bail!(
                    "checksum mismatch for `{}@{}`: the registry index says \
                     {expected}, but the artifact hashes to {actual}. Refusing \
                     to install a corrupt or tampered package",
                    package.name,
                    package.version
                );
            }
        }
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

// --- tar transport (shared with `luabox publish`) ---------------------------

/// `tar` to shell out to. On Windows, prefer the system `bsdtar` so a
/// git-shipped GNU tar on PATH doesn't shadow it (mirrors the toolchain
/// installer — archives are handled by the `tar` CLI, no archive crate).
pub(crate) fn tar_program() -> PathBuf {
    if cfg!(windows)
        && let Ok(root) = env::var("SystemRoot")
    {
        let system_tar = Path::new(&root).join("System32").join("tar.exe");
        if system_tar.is_file() {
            return system_tar;
        }
    }
    PathBuf::from("tar")
}

/// Unpack `archive` into `dest` with `tar -xf`.
pub(crate) fn extract_tar(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let tar = tar_program();
    let status = Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .with_context(|| {
            format!(
                "failed to run `{}` — registry artifacts need `tar` on PATH",
                tar.display()
            )
        })?;
    if !status.success() {
        bail!(
            "`tar -xf` failed to unpack `{}` (exit {})",
            archive.display(),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Pack the contents of `dir` (not the directory itself) into `archive`
/// with `tar -cf` — the artifact format `luabox publish` uploads.
pub(crate) fn create_tar(dir: &Path, archive: &Path) -> anyhow::Result<()> {
    let tar = tar_program();
    let status = Command::new(&tar)
        .arg("-cf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .arg(".")
        .status()
        .with_context(|| {
            format!(
                "failed to run `{}` — publishing needs `tar` on PATH",
                tar.display()
            )
        })?;
    if !status.success() {
        bail!(
            "`tar -cf` failed to pack `{}` (exit {})",
            dir.display(),
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

// --- project discovery and environment -------------------------------------

/// Nearest `luabox.toml` walking up from `cwd`, cargo-style. Dependency
/// commands (unlike `check`) require a manifest — there is nothing to
/// resolve without one.
pub(crate) fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        let manifest_path = current.join("luabox.toml");
        if manifest_path.is_file() {
            let text = fs::read_to_string(&manifest_path)
                .with_context(|| format!("cannot read `{}`", manifest_path.display()))?;
            let manifest = Manifest::parse(&text).map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow!("invalid `{}`:\n{rendered}", manifest_path.display())
            })?;
            return Ok(Project {
                root: current.to_path_buf(),
                manifest_path,
                manifest,
            });
        }
        dir = current.parent();
    }
    bail!(
        "no `luabox.toml` found in `{}` or any parent directory — run `luabox init` first",
        cwd.display()
    )
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

/// `$HOME` (unix) / `%USERPROFILE%` (windows) — no directory-discovery
/// dependency, per luabox-store's design.
fn home_dir() -> anyhow::Result<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(dir) = env::var(var)
            && !dir.trim().is_empty()
        {
            return Ok(PathBuf::from(dir));
        }
    }
    bail!("cannot locate a home directory (set HOME, USERPROFILE, or LUABOX_STORE)")
}

/// Splits `name[@version]`. The leading char is exempt so scoped names
/// (`@org/pkg`) keep their sigil.
fn split_spec(spec: &str) -> anyhow::Result<(String, Option<String>)> {
    if spec.is_empty() {
        bail!("invalid package spec ``; expected `name` or `name@version`");
    }
    let Some(at) = spec[1..].rfind('@').map(|i| i + 1) else {
        return Ok((spec.to_owned(), None));
    };
    let (name, version) = (&spec[..at], &spec[at + 1..]);
    if name.is_empty() || version.is_empty() {
        bail!("invalid package spec `{spec}`; expected `name` or `name@version`");
    }
    Ok((name.to_owned(), Some(version.to_owned())))
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
            #[allow(clippy::permissions_set_readonly_false)]
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
}
