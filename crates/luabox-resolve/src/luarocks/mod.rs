//! The luarocks.org bridge — luabox's registry (SPEC.md §6).
//!
//! luabox follows the pnpm/bun model: [luarocks.org](https://luarocks.org) **is**
//! the registry. A bare version-requirement dependency (`name = "^1.2"`, carried
//! as a [`Source::Registry`] package) resolves here — there is no separate
//! first-party registry. [`LuaRocksProvider`] implements the same
//! [`PackageProvider`] seam as the git and path providers, so the PubGrub solver
//! never learns rocks are special — [`StackedProvider`](crate::StackedProvider)
//! simply routes every `Source::Registry` package to this provider.
//!
//! # How a rock becomes a package
//!
//! 1. **Versions** — the rock's version set comes from luarocks.org's
//!    `manifest.json` (`repository[<rock>]`), each LuaRocks version translated
//!    to semver ([`constraint::translate_version`]); several rock revisions of
//!    one version collapse to one semver, keeping the highest revision.
//! 2. **Metadata & dependencies** — the per-version *rockspec* (itself a Lua
//!    file) is fetched and read statically ([`rockspec::read`]). Its
//!    `dependencies` become registry packages by bare name (recursively
//!    bridged), their LuaRocks constraints translated to semver
//!    ([`constraint::translate_constraint`]); the special `lua` dependency
//!    becomes `lua-versions` metadata instead of a package.
//! 3. **Source** — [`LuaRocksProvider::fetch`] downloads `source.url`
//!    (`git+…` via `git`, or an `http(s)` archive via `curl` + `tar`),
//!    lays the modules out per `build.modules`, and returns a tree ready for
//!    the content-addressed store.
//!
//! # Pure-Lua only
//!
//! C-module rocks (`build.type = make`/`cmake`/`command`, native module
//! sources, or `external_dependencies`) are **out of scope** for the MVP:
//! luabox is not a C build system (SPEC.md §6). They are rejected with a
//! clear error naming the rock and why — never silently mis-resolved.
//!
//! # Offline / hermetic operation
//!
//! Like [`GitProvider`](crate::GitProvider), everything fetched is mirrored
//! under `<store>/luarocks/` so a second resolve is offline. Setting
//! `LUABOX_LUAROCKS_MIRROR` (honored by [`LuaRocksProvider::from_env`]) points
//! the provider at a pre-populated directory of `<rock>-<version>.rockspec`
//! files and extracted `<rock>-<version>/` source trees, so resolves run with
//! no network at all — the basis of the hermetic acceptance tests.

pub mod constraint;
pub mod rockspec;
pub mod rockspec_edit;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::{Version, VersionReq};

use crate::manifest::Dependency;
use crate::provider::{PackageId, PackageMeta, PackageProvider, ProviderError, Source};
use constraint::{translate_constraint, translate_version};
use rockspec::{ModuleSpec, Rockspec};

/// The environment variable that points the provider at a local mirror
/// directory instead of the network (hermetic mode).
pub const MIRROR_ENV: &str = "LUABOX_LUAROCKS_MIRROR";

const DEFAULT_BASE_URL: &str = "https://luarocks.org";

/// One rock's translated version set: semver → the LuaRocks version string it
/// came from (highest rock revision wins on collision).
#[derive(Debug, Clone, Default)]
struct VersionIndex {
    by_semver: BTreeMap<Version, String>,
}

/// One discovered rock, the unit [`LuaRocksProvider::search`] returns.
#[derive(Debug, Clone)]
pub struct RockSummary {
    /// The bare rock name (a luarocks.org `repository` key).
    pub name: String,
    /// The highest translated semver, or `None` when every version is
    /// non-numeric (`scm`/`dev`/…) and has no semver image.
    pub latest: Option<Version>,
    /// How many distinct translated semver versions the rock has.
    pub version_count: usize,
}

/// [`PackageProvider`] for registry (luarocks.org) packages, keyed by bare
/// rock name, backed by luarocks.org (or a local mirror), caching everything
/// it fetches.
pub struct LuaRocksProvider {
    /// `<store>/luarocks` — where fetched rockspecs and sources are mirrored.
    cache_dir: PathBuf,
    /// Base URL of the rocks server (overridable for tests).
    base_url: String,
    /// When set, resolve entirely from this directory (no network).
    mirror: Option<PathBuf>,
    versions: RefCell<BTreeMap<String, VersionIndex>>,
    rockspecs: RefCell<BTreeMap<(String, String), Rockspec>>,
}

impl LuaRocksProvider {
    /// A provider caching under `cache_dir` and talking to luarocks.org.
    #[must_use]
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            mirror: None,
            versions: RefCell::new(BTreeMap::new()),
            rockspecs: RefCell::new(BTreeMap::new()),
        }
    }

    /// [`Self::new`], then honoring `LUABOX_LUAROCKS_MIRROR` for hermetic
    /// resolves: a non-empty value makes the provider read only from that
    /// directory.
    #[must_use]
    pub fn from_env(cache_dir: impl Into<PathBuf>) -> Self {
        let mut provider = Self::new(cache_dir);
        if let Ok(dir) = std::env::var(MIRROR_ENV)
            && !dir.trim().is_empty()
        {
            provider.mirror = Some(PathBuf::from(dir));
        }
        provider
    }

    /// Point the provider at a mirror directory (no network).
    #[must_use]
    pub fn with_mirror(mut self, mirror: Option<PathBuf>) -> Self {
        self.mirror = mirror;
        self
    }

    /// Override the rocks-server base URL (for tests).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Whether `package` is a registry package this provider owns (any
    /// [`Source::Registry`] package — a bare rock name on luarocks.org).
    #[must_use]
    pub fn supports(package: &PackageId) -> bool {
        rock_name(package).is_some()
    }

    /// Fetches (or reuses from cache) the module tree for registry rock
    /// `package` at `version`, ready to intern into the content-addressed
    /// store. This is
    /// the seam `luabox install` uses after resolution.
    ///
    /// # Errors
    /// Fails for unknown rocks/versions, C-module rocks, unresolvable source
    /// URLs, and download/extract failures.
    pub fn fetch(&self, package: &PackageId, version: &Version) -> Result<PathBuf, ProviderError> {
        let rock = require_rock(package)?;
        let (luarocks_version, spec) = self.rockspec_for(rock, version)?;
        classify(rock, &luarocks_version, &spec)?;

        let tree_dir = self
            .cache_dir
            .join("tree")
            .join(format!("{rock}-{luarocks_version}"));
        if tree_dir.join(".luabox-fetched").is_file() {
            return Ok(tree_dir);
        }

        let source_root = self.fetch_source(rock, &luarocks_version, &spec)?;
        remove_all_force(&tree_dir).map_err(|e| io(&tree_dir, &format!("clear tree: {e}")))?;
        fs::create_dir_all(&tree_dir).map_err(|e| io(&tree_dir, &format!("create tree: {e}")))?;
        build_module_tree(&source_root, &spec, &tree_dir)?;
        fs::write(tree_dir.join(".luabox-fetched"), b"")
            .map_err(|e| io(&tree_dir, &format!("mark fetched: {e}")))?;
        Ok(tree_dir)
    }

    // --- version / rockspec loading --------------------------------------

    /// The translated version set of `rock`, built once and cached.
    fn version_index(&self, rock: &str) -> Result<VersionIndex, ProviderError> {
        if let Some(index) = self.versions.borrow().get(rock) {
            return Ok(index.clone());
        }
        let luarocks_versions = self.list_luarocks_versions(rock)?;
        if luarocks_versions.is_empty() {
            return Err(ProviderError::UnknownPackage {
                package: rock.to_owned(),
            });
        }
        let mut index = VersionIndex::default();
        for luarocks_version in luarocks_versions {
            let Some(semver) = translate_version(&luarocks_version) else {
                continue; // scm/dev and other non-numeric versions
            };
            // Keep the highest rock revision for a given semver.
            let keep_existing = index.by_semver.get(&semver).is_some_and(|existing| {
                rock_revision(existing) >= rock_revision(&luarocks_version)
            });
            if !keep_existing {
                index.by_semver.insert(semver, luarocks_version);
            }
        }
        if index.by_semver.is_empty() {
            return Err(ProviderError::UnknownPackage {
                package: rock.to_owned(),
            });
        }
        self.versions
            .borrow_mut()
            .insert(rock.to_owned(), index.clone());
        Ok(index)
    }

    /// The raw LuaRocks version strings for `rock` (mirror listing or the
    /// network manifest).
    fn list_luarocks_versions(&self, rock: &str) -> Result<Vec<String>, ProviderError> {
        if let Some(mirror) = &self.mirror {
            return Ok(mirror_versions(mirror, rock));
        }
        self.network_versions(rock)
    }

    /// The `(luarocks_version, rockspec)` for a resolved semver.
    fn rockspec_for(
        &self,
        rock: &str,
        version: &Version,
    ) -> Result<(String, Rockspec), ProviderError> {
        let index = self.version_index(rock)?;
        let luarocks_version = index.by_semver.get(version).cloned().ok_or_else(|| {
            ProviderError::VersionNotFound {
                package: rock.to_owned(),
                version: version.to_string(),
            }
        })?;
        let key = (rock.to_owned(), luarocks_version.clone());
        if let Some(spec) = self.rockspecs.borrow().get(&key) {
            return Ok((luarocks_version, spec.clone()));
        }
        let text = self.rockspec_text(rock, &luarocks_version)?;
        let spec = rockspec::read(&text);
        self.rockspecs.borrow_mut().insert(key, spec.clone());
        Ok((luarocks_version, spec))
    }

    /// Rockspec text: mirror file, on-disk cache, or network fetch (cached).
    fn rockspec_text(&self, rock: &str, luarocks_version: &str) -> Result<String, ProviderError> {
        let file_name = format!("{rock}-{luarocks_version}.rockspec");
        if let Some(mirror) = &self.mirror {
            let path = mirror.join(&file_name);
            return fs::read_to_string(&path).map_err(|e| ProviderError::Io {
                path,
                message: format!("mirror is missing rockspec `{file_name}`: {e}"),
            });
        }
        let cached = self.cache_dir.join("rockspecs").join(&file_name);
        if let Ok(text) = fs::read_to_string(&cached) {
            return Ok(text);
        }
        let url = format!("{}/{file_name}", self.base_url);
        let text = curl_text(&url)?;
        if let Some(parent) = cached.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&cached, &text);
        Ok(text)
    }

    /// Network version listing: read `repository[rock]`'s keys from the
    /// (cached) `manifest.json`.
    fn network_versions(&self, rock: &str) -> Result<Vec<String>, ProviderError> {
        let manifest = self.load_manifest()?;
        let Some(entry) = manifest
            .get("repository")
            .and_then(|r| r.get(rock))
            .and_then(serde_json::Value::as_object)
        else {
            return Ok(Vec::new());
        };
        Ok(entry.keys().cloned().collect())
    }

    /// Fetches (and caches under `<cache>/manifest.json`) the luarocks.org root
    /// `manifest.json`, parsed as JSON. The single fetch+cache seam shared by
    /// version listing and rock discovery ([`Self::search`]).
    fn load_manifest(&self) -> Result<serde_json::Value, ProviderError> {
        let manifest_path = self.cache_dir.join("manifest.json");
        let text = if let Ok(text) = fs::read_to_string(&manifest_path) {
            text
        } else {
            let url = format!("{}/manifest.json", self.base_url);
            let text = curl_text(&url)?;
            if let Some(parent) = manifest_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&manifest_path, &text);
            text
        };
        serde_json::from_str(&text).map_err(|e| ProviderError::InvalidManifest {
            path: manifest_path,
            message: format!("luarocks manifest.json is not valid JSON: {e}"),
        })
    }

    /// Every rock the registry knows, keyed by bare rock name, each mapped to
    /// its raw LuaRocks version strings. Mirror mode enumerates the
    /// `<rock>-<version>.rockspec` files; network mode reads the root
    /// `manifest.json`'s `repository` object. The discovery counterpart to
    /// [`Self::list_luarocks_versions`] (which serves one known rock).
    fn all_rock_versions(&self) -> Result<BTreeMap<String, Vec<String>>, ProviderError> {
        if let Some(mirror) = &self.mirror {
            return Ok(mirror_all_rocks(mirror));
        }
        let manifest = self.load_manifest()?;
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if let Some(repository) = manifest.get("repository").and_then(serde_json::Value::as_object) {
            for (rock, versions) in repository {
                let list = versions
                    .as_object()
                    .map(|obj| obj.keys().cloned().collect())
                    .unwrap_or_default();
                out.insert(rock.clone(), list);
            }
        }
        Ok(out)
    }

    /// Discovers registry rocks whose bare name contains `query`
    /// (case-insensitive substring; an empty query matches every rock). Each
    /// hit carries its highest translated semver ([`Self::latest`](RockSummary::latest))
    /// and the count of distinct translated versions, name-sorted. Reads the
    /// same manifest/mirror source the resolver does (network manifest cached
    /// under `<cache>/manifest.json`); no per-rock rockspec is fetched, so
    /// descriptions are not available here.
    ///
    /// This is the seam `luabox search` is built on.
    ///
    /// # Errors
    /// Propagates a manifest fetch/parse failure (network mode).
    pub fn search(&self, query: &str) -> Result<Vec<RockSummary>, ProviderError> {
        let needle = query.trim().to_lowercase();
        let rocks = self.all_rock_versions()?;
        let mut out = Vec::new();
        for (name, raw_versions) in rocks {
            if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                continue;
            }
            let mut semvers: Vec<Version> = raw_versions
                .iter()
                .filter_map(|v| translate_version(v))
                .collect();
            semvers.sort();
            semvers.dedup();
            out.push(RockSummary {
                name,
                latest: semvers.last().cloned(),
                version_count: semvers.len(),
            });
        }
        Ok(out)
    }

    // --- source fetching --------------------------------------------------

    /// Materializes the rock's *source tree* (before module layout): the
    /// mirror's pre-extracted directory, or a fresh download.
    fn fetch_source(
        &self,
        rock: &str,
        luarocks_version: &str,
        spec: &Rockspec,
    ) -> Result<PathBuf, ProviderError> {
        if let Some(mirror) = &self.mirror {
            let dir = mirror.join(format!("{rock}-{luarocks_version}"));
            if !dir.is_dir() {
                return Err(ProviderError::Io {
                    path: dir.clone(),
                    message: format!(
                        "mirror is missing the source tree for `{rock}-{luarocks_version}`"
                    ),
                });
            }
            // The mirror tree is authoritative — honor only an explicit
            // `source.dir`, never guess at a wrapper subdirectory.
            return Ok(apply_source_dir(dir, spec, false));
        }

        let url = spec.source.url.as_deref().ok_or_else(|| {
            spec_error(
                rock,
                luarocks_version,
                "its rockspec has no statically resolvable `source.url`",
            )
        })?;

        let src_dir = self
            .cache_dir
            .join("src")
            .join(format!("{rock}-{luarocks_version}"));
        remove_all_force(&src_dir).map_err(|e| io(&src_dir, &format!("clear src: {e}")))?;
        fs::create_dir_all(&src_dir).map_err(|e| io(&src_dir, &format!("create src: {e}")))?;

        // Archives commonly extract into a single wrapper directory, so allow
        // descending into one when `source.dir` is not given; git checkouts do
        // not wrap, so honor only an explicit `source.dir`.
        let allow_wrapper = if let Some(git_url) = url
            .strip_prefix("git+")
            .or_else(|| url.starts_with("git://").then_some(url))
        {
            fetch_git(git_url, spec, &src_dir)?;
            false
        } else if url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("ftp://")
        {
            fetch_archive(url, &src_dir)?;
            true
        } else {
            return Err(spec_error(
                rock,
                luarocks_version,
                &format!("unsupported `source.url` scheme: `{url}`"),
            ));
        };
        Ok(apply_source_dir(src_dir, spec, allow_wrapper))
    }
}

/// Clones a `git+`/`git://` source (shallow, at `source.tag`/`branch`) and
/// strips `.git` so the tree is store-ready.
fn fetch_git(url: &str, spec: &Rockspec, dest: &Path) -> Result<(), ProviderError> {
    let mut args = vec![
        "clone".to_owned(),
        "--quiet".to_owned(),
        "--depth".to_owned(),
        "1".to_owned(),
    ];
    if let Some(reference) = spec.source.tag.as_deref().or(spec.source.branch.as_deref()) {
        args.push("--branch".to_owned());
        args.push(reference.to_owned());
    }
    args.push("--".to_owned());
    args.push(url.to_owned());
    args.push(dest.to_string_lossy().into_owned());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run("git", &arg_refs, dest)?;
    // Drop `.git` so the tree is store-ready (git objects are read-only).
    remove_all_force(&dest.join(".git")).map_err(|e| io(dest, &format!("strip .git: {e}")))?;
    Ok(())
}

/// Downloads an `http(s)`/`ftp` archive with `curl` and unpacks it with `tar`.
fn fetch_archive(url: &str, dest: &Path) -> Result<(), ProviderError> {
    let name = url
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("archive");
    let archive = dest.join(name);
    run(
        "curl",
        &[
            "-fsSL",
            "--max-time",
            "120",
            "-o",
            &archive.to_string_lossy(),
            url,
        ],
        dest,
    )?;
    let tar = tar_program();
    run(
        &tar.to_string_lossy(),
        &[
            "-xf",
            &archive.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ],
        dest,
    )?;
    let _ = fs::remove_file(&archive);
    Ok(())
}

/// A [`ProviderError::Io`] carrying a path and message.
fn io(path: &Path, message: &str) -> ProviderError {
    ProviderError::Io {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

/// A [`ProviderError::External`] naming the tool that failed and the reason.
fn external(command: &str, message: String) -> ProviderError {
    ProviderError::External {
        command: command.to_owned(),
        message,
    }
}

/// A [`ProviderError::InvalidManifest`] naming a rock whose rockspec cannot be
/// used as-is.
fn spec_error(rock: &str, version: &str, why: &str) -> ProviderError {
    ProviderError::InvalidManifest {
        path: PathBuf::from(format!("{rock}-{version}.rockspec")),
        message: format!("cannot use luarocks rock `{rock}` {version}: {why}"),
    }
}

impl PackageProvider for LuaRocksProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        let rock = require_rock(package)?;
        Ok(self
            .version_index(rock)?
            .by_semver
            .keys()
            .cloned()
            .collect())
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        let rock = require_rock(package)?;
        let (luarocks_version, spec) = self.rockspec_for(rock, version)?;
        classify(rock, &luarocks_version, &spec)?;

        let mut deps = BTreeMap::new();
        for dep in &spec.dependencies {
            let (name, constraints) = parse_dependency_string(dep);
            if name.is_empty() || name == "lua" {
                // `lua` is the interpreter, not a rock: handled as metadata.
                continue;
            }
            let requirement = translate_constraint(constraints).map_err(|message| {
                ProviderError::InvalidRequirement {
                    dependent: rock.to_owned(),
                    dependency: name.to_owned(),
                    requirement: constraints.trim().to_owned(),
                    message,
                }
            })?;
            deps.insert(name.to_owned(), Dependency::Version(requirement));
        }
        Ok(deps)
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        let rock = require_rock(package)?;
        let (luarocks_version, spec) = self.rockspec_for(rock, version)?;
        classify(rock, &luarocks_version, &spec)?;
        Ok(PackageMeta {
            lua_versions: lua_dialects(&spec),
            checksum: None,
            pinned: None,
        })
    }
}

// -------------------------------------------------------------------------
// Free functions
// -------------------------------------------------------------------------

/// The rock name of a registry package (its bare name), or `None` for
/// non-registry (path/git) sources.
fn rock_name(package: &PackageId) -> Option<&str> {
    matches!(package.source, Source::Registry).then_some(package.name.as_str())
}

/// The rock name of a package this provider must own, or
/// [`ProviderError::UnsupportedSource`] (so [`StackedProvider`] falls through
/// for path/git packages).
fn require_rock(package: &PackageId) -> Result<&str, ProviderError> {
    rock_name(package).ok_or_else(|| ProviderError::UnsupportedSource {
        package: package.to_string(),
    })
}

/// Translates one raw LuaRocks dependency string (as found in a rockspec's
/// `dependencies`/`test_dependencies`, e.g. `"lpeg >= 1.0, < 2.0"`) into a
/// registry [`Dependency`] keyed by the bare rock name.
///
/// Returns `Ok(None)` for the special `lua` interpreter constraint (dialect
/// metadata, not a rock) and for an empty entry. The rock's LuaRocks
/// constraint grammar is translated to a Cargo requirement via
/// [`translate_constraint`].
///
/// # Errors
/// Propagates [`translate_constraint`]'s error for an absurd `~>` operand.
pub fn dependency_from_spec(spec: &str) -> Result<Option<(String, Dependency)>, String> {
    let (name, constraints) = parse_dependency_string(spec);
    if name.is_empty() || name == "lua" {
        return Ok(None);
    }
    let requirement = translate_constraint(constraints)?;
    Ok(Some((name.to_owned(), Dependency::Version(requirement))))
}

/// The integer rock revision of a LuaRocks version (`1.4.1-3` → 3), or 0.
fn rock_revision(luarocks_version: &str) -> u64 {
    luarocks_version
        .rsplit_once('-')
        .and_then(|(_, rev)| rev.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Splits a LuaRocks dependency string (`"lpeg >= 1.0, < 2.0"`) into its rock
/// name (namespace stripped) and the constraint text.
fn parse_dependency_string(dep: &str) -> (&str, &str) {
    let dep = dep.trim();
    let end = dep
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '=' | '~' | '!'))
        .unwrap_or(dep.len());
    let (name, constraints) = dep.split_at(end);
    // `user/rock` namespaced deps resolve by bare rock name (a documented
    // simplification; the root rockspec server is keyed by bare name).
    let bare = name.rsplit('/').next().unwrap_or(name);
    (bare, constraints)
}

/// Maps the rockspec's `lua` dependency constraint onto the dialects it
/// admits (SPEC.md §6 `lua-versions`). No `lua` dependency → all dialects.
fn lua_dialects(spec: &Rockspec) -> Vec<String> {
    let lua_constraint = spec.dependencies.iter().find_map(|dep| {
        let (name, constraints) = parse_dependency_string(dep);
        (name == "lua").then_some(constraints)
    });
    let Some(constraints) = lua_constraint else {
        return Vec::new();
    };
    let Ok(requirement) = translate_constraint(constraints) else {
        return Vec::new();
    };
    let Ok(req) = VersionReq::parse(&requirement) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (dialect, minor) in [("5.1", 1u64), ("5.2", 2), ("5.3", 3), ("5.4", 4)] {
        if req.matches(&Version::new(5, minor, 0)) {
            out.push(dialect.to_owned());
        }
    }
    // LuaJIT is 5.1-compatible; admit it when 5.1 is admitted.
    if out.iter().any(|d| d == "5.1") {
        out.push("luajit".to_owned());
    }
    out
}

/// Classifies a rock as pure-Lua (ok) or a C module (rejected, SPEC.md §6).
fn classify(rock: &str, version: &str, spec: &Rockspec) -> Result<(), ProviderError> {
    let reject = |why: String| {
        Err(ProviderError::InvalidManifest {
            path: PathBuf::from(format!("{rock}-{version}.rockspec")),
            message: format!(
                "luarocks rock `{rock}` {version} is a C/native module ({why}); \
                 luabox does not build C modules — only pure-Lua rocks are supported \
                 by the luarocks bridge (SPEC.md §6)"
            ),
        })
    };

    if spec.build.has_external_dependencies {
        return reject("it declares `external_dependencies`".to_owned());
    }
    match spec.build.build_type.as_deref() {
        Some("make") => return reject("build.type = make".to_owned()),
        Some("cmake") => return reject("build.type = cmake".to_owned()),
        Some("command") => return reject("build.type = command".to_owned()),
        _ => {}
    }
    for (name, module) in &spec.build.modules {
        match module {
            ModuleSpec::NativeFile(path) => {
                return reject(format!("module `{name}` has native source `{path}`"));
            }
            ModuleSpec::Native => {
                return reject(format!("module `{name}` compiles C sources"));
            }
            ModuleSpec::LuaFile(_) | ModuleSpec::Unknown => {}
        }
    }
    Ok(())
}

/// Resolves the true source root: an explicit `source.dir` under `root` when
/// present, else (only when `allow_wrapper`, i.e. for downloaded archives) the
/// sole wrapper subdirectory a tarball extracted into.
fn apply_source_dir(root: PathBuf, spec: &Rockspec, allow_wrapper: bool) -> PathBuf {
    if let Some(dir) = &spec.source.dir {
        let nested = root.join(dir);
        if nested.is_dir() {
            return nested;
        }
    }
    if allow_wrapper {
        return single_subdir(&root).unwrap_or(root);
    }
    root
}

/// The sole subdirectory of `dir` when it contains exactly one entry and that
/// entry is a directory (the common tarball layout), else `None`.
fn single_subdir(dir: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dir).ok()?.flatten();
    let first = entries.next()?.path();
    if entries.next().is_some() || !first.is_dir() {
        return None;
    }
    Some(first)
}

/// Lays out the rock's Lua modules into `dest` at their `require` paths.
///
/// Heuristics (SPEC.md §6): `build.modules` is authoritative — each
/// `modname = "path.lua"` entry is copied to `dest/<modname→slashes>.lua`.
/// With no usable module map, fall back to the conventional layout: `lua/`,
/// then `src/`, then the source root itself.
fn build_module_tree(
    source_root: &Path,
    spec: &Rockspec,
    dest: &Path,
) -> Result<(), ProviderError> {
    let lua_modules: Vec<(&String, &String)> = spec
        .build
        .modules
        .iter()
        .filter_map(|(name, module)| match module {
            ModuleSpec::LuaFile(path) => Some((name, path)),
            _ => None,
        })
        .collect();

    if !lua_modules.is_empty() {
        for (modname, rel_path) in lua_modules {
            let from = source_root.join(rel_path);
            let to = dest.join(format!("{}.lua", modname.replace('.', "/")));
            copy_file(&from, &to)?;
        }
        return Ok(());
    }

    // Fallback heuristics for module-less pure-Lua rocks.
    for candidate in ["lua", "src"] {
        let dir = source_root.join(candidate);
        if dir.is_dir() {
            return copy_tree(&dir, dest);
        }
    }
    copy_tree(source_root, dest)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), ProviderError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| io(parent, &format!("create `{}`: {e}", parent.display())))?;
    }
    fs::copy(from, to).map(|_| ()).map_err(|e| {
        io(
            to,
            &format!("copy `{}` -> `{}`: {e}", from.display(), to.display()),
        )
    })
}

/// Recursively copies `src` into `dst`, skipping VCS/metadata noise.
fn copy_tree(src: &Path, dst: &Path) -> Result<(), ProviderError> {
    fs::create_dir_all(dst).map_err(|e| io(dst, &format!("create `{}`: {e}", dst.display())))?;
    let entries =
        fs::read_dir(src).map_err(|e| io(src, &format!("read `{}`: {e}", src.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| io(src, &format!("read entry: {e}")))?;
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | ".luabox-fetched")) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

/// LuaRocks version strings for `rock` from a mirror: `<rock>-<version>.rockspec`
/// files.
fn mirror_versions(mirror: &Path, rock: &str) -> Vec<String> {
    let prefix = format!("{rock}-");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(mirror) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(rest) = name.strip_prefix(&prefix)
            && let Some(version) = rest.strip_suffix(".rockspec")
        {
            out.push(version.to_owned());
        }
    }
    out
}

/// Every rock in a mirror directory, keyed by bare rock name, mapped to its
/// raw LuaRocks version strings — the mirror-mode counterpart of the network
/// `manifest.json`'s `repository` object. Enumerates `<rock>-<version>.rockspec`
/// files (source-tree directories are ignored).
fn mirror_all_rocks(mirror: &Path) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir(mirror) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some((rock, version)) = split_rockspec_filename(name) {
            out.entry(rock).or_default().push(version);
        }
    }
    out
}

/// Splits a `<rock>-<version>.rockspec` file name into `(rock, luarocks_version)`.
/// The version begins at the first `-` immediately followed by an ASCII digit,
/// so hyphenated rock names (`lua-cjson-2.1.0-1.rockspec` → `lua-cjson`,
/// `2.1.0-1`) split correctly. Returns `None` for a name that is not a
/// rockspec or carries no numeric version. Uses `split_at` on the `-` (an
/// ASCII byte boundary) — never `string_slice`.
fn split_rockspec_filename(file: &str) -> Option<(String, String)> {
    let stem = file.strip_suffix(".rockspec")?;
    let bytes = stem.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if *byte == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let (rock, rest) = stem.split_at(i);
            let version = rest.strip_prefix('-').unwrap_or(rest);
            if rock.is_empty() {
                return None;
            }
            return Some((rock.to_owned(), version.to_owned()));
        }
    }
    None
}

/// `tar` program to shell out to (Windows: prefer system `bsdtar` so a
/// git-shipped GNU tar can't shadow it — it can't read `.zip`).
fn tar_program() -> PathBuf {
    if cfg!(windows)
        && let Ok(root) = std::env::var("SystemRoot")
    {
        let system_tar = Path::new(&root).join("System32").join("tar.exe");
        if system_tar.is_file() {
            return system_tar;
        }
    }
    PathBuf::from("tar")
}

/// Runs a subprocess (`git`/`tar`), mapping a spawn failure or non-zero exit
/// to [`ProviderError::External`] so it reads as the tool/network failure it
/// is, not a local I/O error.
fn run(program: &str, args: &[&str], cwd: &Path) -> Result<(), ProviderError> {
    let output = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| external(program, format!("cannot run `{program}`: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(external(
            program,
            format!(
                "`{program} {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

/// `curl -fsSL` fetching a URL's body as text, via the shared [`crate::http`]
/// transport. Network/tool failures surface as [`ProviderError::External`].
fn curl_text(url: &str) -> Result<String, ProviderError> {
    let output = crate::http::get(url, 120, true).map_err(|e| {
        external(
            "curl",
            format!("cannot run `curl` (needed to reach {url}): {e}"),
        )
    })?;
    if !output.status.success() {
        return Err(external(
            "curl",
            format!(
                "`curl` failed for {url}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `remove_dir_all` that also clears read-only files (git objects) and
/// tolerates missing paths.
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
                reason = "clearing the read-only bit is exactly the intent: git packs objects \
                          read-only and they must be writable to delete on Windows"
            )]
            perms.set_readonly(false);
            fs::set_permissions(path, perms)?;
        }
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rock_id(name: &str) -> PackageId {
        PackageId::registry(name)
    }

    #[test]
    fn recognizes_registry_packages_by_bare_name() {
        // Every registry package is a luarocks.org rock now (bare name, no
        // prefix); path/git sources are not this provider's job.
        assert!(LuaRocksProvider::supports(&rock_id("penlight")));
        assert!(LuaRocksProvider::supports(&PackageId::registry("inspect")));
        assert!(!LuaRocksProvider::supports(&PackageId::path("x", "/tmp/x")));
    }

    #[test]
    fn dependency_from_spec_skips_lua_and_translates() {
        assert_eq!(dependency_from_spec("lua >= 5.1").unwrap(), None);
        assert_eq!(dependency_from_spec("").unwrap(), None);
        let (name, dep) = dependency_from_spec("lpeg >= 1.0, < 2.0").unwrap().unwrap();
        assert_eq!(name, "lpeg");
        assert_eq!(dep, Dependency::Version(">=1.0, <2.0".to_owned()));
        // Namespaced `user/rock` resolves by bare rock name.
        let (name, _) = dependency_from_spec("hisham/luaposix").unwrap().unwrap();
        assert_eq!(name, "luaposix");
    }

    #[test]
    fn rockspec_filename_splits_name_and_version() {
        assert_eq!(
            split_rockspec_filename("penlight-1.14.0-1.rockspec"),
            Some(("penlight".to_owned(), "1.14.0-1".to_owned()))
        );
        // A hyphenated rock name splits at the first `-<digit>`, not the first `-`.
        assert_eq!(
            split_rockspec_filename("lua-cjson-2.1.0-1.rockspec"),
            Some(("lua-cjson".to_owned(), "2.1.0-1".to_owned()))
        );
        // Not a rockspec, or no numeric version → no split.
        assert_eq!(split_rockspec_filename("penlight-1.0-1"), None);
        assert_eq!(split_rockspec_filename("README.md"), None);
        assert_eq!(split_rockspec_filename("-1.0-1.rockspec"), None);
    }

    #[test]
    fn mirror_search_matches_substring_and_reports_latest() {
        let dir = tempfile::tempdir().expect("tempdir");
        for file in [
            "penlight-1.0-1.rockspec",
            "penlight-1.5.4-1.rockspec",
            "inspect-3.1.3-0.rockspec",
        ] {
            std::fs::write(dir.path().join(file), b"").expect("write rockspec");
        }
        let provider = LuaRocksProvider::new(dir.path().join("cache"))
            .with_mirror(Some(dir.path().to_path_buf()));

        let hits = provider.search("pen").expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "penlight");
        assert_eq!(hits[0].latest, Some(Version::new(1, 5, 4)));
        assert_eq!(hits[0].version_count, 2);

        // Empty query lists every rock, name-sorted.
        let all = provider.search("").expect("search all");
        let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["inspect", "penlight"]);
    }

    #[test]
    fn rock_revision_parsing() {
        assert_eq!(rock_revision("1.4.1-3"), 3);
        assert_eq!(rock_revision("2.0-1"), 1);
        assert_eq!(rock_revision("scm"), 0);
    }

    #[test]
    fn dependency_string_parsing() {
        assert_eq!(parse_dependency_string("lua >= 5.1"), ("lua", " >= 5.1"));
        assert_eq!(
            parse_dependency_string("lpeg >= 1.0, < 2.0"),
            ("lpeg", " >= 1.0, < 2.0")
        );
        assert_eq!(
            parse_dependency_string("luafilesystem"),
            ("luafilesystem", "")
        );
        assert_eq!(parse_dependency_string("hisham/luaposix"), ("luaposix", ""));
        assert_eq!(
            parse_dependency_string("penlight>=1.5"),
            ("penlight", ">=1.5")
        );
    }

    #[test]
    fn c_rock_is_rejected_with_a_clear_message() {
        let mut spec = Rockspec::default();
        spec.build.build_type = Some("make".to_owned());
        let err = classify("luasocket", "3.0-1", &spec).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("luasocket"), "{text}");
        assert!(text.contains("C/native module"), "{text}");
        assert!(text.contains("make"), "{text}");
    }

    #[test]
    fn native_module_source_is_rejected() {
        let mut spec = Rockspec::default();
        spec.build.modules.insert(
            "cjson".to_owned(),
            ModuleSpec::NativeFile("cjson.c".to_owned()),
        );
        assert!(classify("lua-cjson", "2.1-1", &spec).is_err());
    }

    #[test]
    fn pure_lua_rock_passes_classification() {
        let mut spec = Rockspec::default();
        spec.build.build_type = Some("builtin".to_owned());
        spec.build.modules.insert(
            "inspect".to_owned(),
            ModuleSpec::LuaFile("inspect.lua".to_owned()),
        );
        assert!(classify("inspect", "3.1.3-0", &spec).is_ok());
    }

    #[test]
    fn lua_dependency_becomes_dialect_metadata() {
        let mut spec = Rockspec::default();
        spec.dependencies.push("lua >= 5.1".to_owned());
        assert_eq!(
            lua_dialects(&spec),
            vec!["5.1", "5.2", "5.3", "5.4", "luajit"]
        );
        let mut spec = Rockspec::default();
        spec.dependencies.push("lua >= 5.3".to_owned());
        assert_eq!(lua_dialects(&spec), vec!["5.3", "5.4"]);
        assert!(lua_dialects(&Rockspec::default()).is_empty());
    }
}
