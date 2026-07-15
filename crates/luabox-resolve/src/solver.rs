//! PubGrub version solving over a [`PackageProvider`] (SPEC.md §6).
//!
//! [`resolve`] maps the manifest's dependency kinds onto PubGrub:
//!
//! - package identity is [`PkgKey`] — the synthetic root, or a
//!   [`PackageId`] (name × source, so `foo` from a path and `foo` from the
//!   registry are distinct packages);
//! - version sets are [`VersionRanges`] built with cargo requirement
//!   semantics (see `semver_ranges`);
//! - `lua-versions` compatibility (SPEC.md §6) is a *PubGrub
//!   incompatibility* — an incompatible version is reported as
//!   [`pubgrub::Dependencies::Unavailable`] with a self-describing reason,
//!   so the conflict report explains it instead of a post-hoc error;
//! - an existing `luabox.lock` biases version choice: a locked version
//!   that still satisfies the requirement is preferred over newer ones
//!   (minimal churn), stale pins are simply ignored and re-resolved.
//!
//! Version preference is otherwise highest-satisfying-first, cargo-style.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use pubgrub::{
    DefaultStringReporter, Dependencies, DependencyConstraints, DependencyProvider,
    PackageResolutionStatistics, PubGrubError, Ranges, Reporter as _, SelectedDependencies,
};
use semver::{Version, VersionReq};

use crate::lockfile::{LockedPackage, LockedSource, Lockfile};
use crate::manifest::{Dependency, Manifest};
use crate::provider::{
    GitReference, PackageId, PackageProvider, ProviderError, Source, normalize_path,
    parse_manifest_at, relative_display,
};
use crate::report::ResolveReportFormatter;
use crate::semver_ranges::{VersionRanges, req_to_ranges, version_matches};

/// PubGrub package identity: the project being resolved, or a dependency.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum PkgKey {
    /// The root project (its version is fixed; it is always selected).
    Root,
    Pkg(PackageId),
}

impl fmt::Display for PkgKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => write!(f, "root"),
            Self::Pkg(id) => write!(f, "{id}"),
        }
    }
}

/// Everything the solver knows about the project being resolved.
struct Context<'a> {
    provider: &'a dyn PackageProvider,
    root_dir: PathBuf,
    root_name: String,
    root_version: Version,
    /// The project's `[package] edition` — every selected package's
    /// `lua-versions` must admit it (SPEC.md §6).
    edition: String,
    root_deps: BTreeMap<String, Dependency>,
    root_dev_deps: BTreeMap<String, Dependency>,
    /// Workspace member name → member directory (for
    /// `pkg = { workspace = true }` deps, SPEC.md §5/§6).
    members: BTreeMap<String, PathBuf>,
}

impl Context<'_> {
    fn label(&self, key: &PkgKey) -> String {
        match key {
            PkgKey::Root => self.root_name.clone(),
            PkgKey::Pkg(id) => id.to_string(),
        }
    }

    /// The directory path-relative dependencies of `dependent` resolve
    /// against; `None` when the dependent has no local directory.
    fn dependent_dir<'k>(&'k self, dependent: &'k PkgKey) -> Option<&'k Path> {
        match dependent {
            PkgKey::Root => Some(&self.root_dir),
            PkgKey::Pkg(PackageId {
                source: Source::Path { path },
                ..
            }) => Some(path),
            PkgKey::Pkg(_) => None,
        }
    }

    /// Converts the manifest-shaped dependency map of one selected package
    /// version into `(package, version-set)` pairs.
    ///
    /// Root: `[dependencies]` ∪ `[dev-dependencies]` (dev-deps participate
    /// only at the root, cargo-style). Duplicated targets are intersected
    /// by the caller.
    fn dependency_entries(
        &self,
        key: &PkgKey,
        version: &Version,
    ) -> Result<Vec<(PkgKey, VersionRanges)>, ProviderError> {
        let mut entries = Vec::new();
        match key {
            PkgKey::Root => {
                for (name, dep) in self.root_deps.iter().chain(&self.root_dev_deps) {
                    entries.push(self.convert(key, name, dep)?);
                }
            }
            PkgKey::Pkg(id) => {
                for (name, dep) in &self.provider.dependencies(id, version)? {
                    entries.push(self.convert(key, name, dep)?);
                }
            }
        }
        Ok(entries)
    }

    fn convert(
        &self,
        dependent: &PkgKey,
        name: &str,
        dep: &Dependency,
    ) -> Result<(PkgKey, VersionRanges), ProviderError> {
        match dep {
            Dependency::Version(req) => Ok((
                PkgKey::Pkg(PackageId::registry(name)),
                self.parse_req(dependent, name, req)?,
            )),
            Dependency::Git(git) => {
                let reference = git
                    .rev
                    .clone()
                    .map(GitReference::Rev)
                    .or_else(|| git.tag.clone().map(GitReference::Tag))
                    .or_else(|| git.branch.clone().map(GitReference::Branch))
                    .unwrap_or(GitReference::DefaultBranch);
                Ok((
                    PkgKey::Pkg(PackageId::git(name, git.git.clone(), reference)),
                    self.optional_req(dependent, name, git.version.as_deref())?,
                ))
            }
            Dependency::Url(url_dep) => Ok((
                PkgKey::Pkg(PackageId::url(
                    name,
                    url_dep.url.clone(),
                    url_dep.sha256.clone(),
                )),
                self.optional_req(dependent, name, url_dep.version.as_deref())?,
            )),
            Dependency::Path(path_dep) => {
                // Path deps resolve relative to the *dependent's* directory.
                let base = self.dependent_dir(dependent).ok_or_else(|| {
                    ProviderError::PathDependencyNotAllowed {
                        dependency: name.to_owned(),
                        dependent: self.label(dependent),
                    }
                })?;
                let dir = normalize_path(&base.join(&path_dep.path));
                Ok((
                    PkgKey::Pkg(PackageId {
                        name: name.to_owned(),
                        source: Source::Path { path: dir },
                    }),
                    self.optional_req(dependent, name, path_dep.version.as_deref())?,
                ))
            }
            Dependency::Workspace(ws) => {
                // Workspace deps resolve against the member with that name.
                let dir =
                    self.members
                        .get(name)
                        .ok_or_else(|| ProviderError::NotAWorkspaceMember {
                            dependency: name.to_owned(),
                            dependent: self.label(dependent),
                        })?;
                Ok((
                    PkgKey::Pkg(PackageId {
                        name: name.to_owned(),
                        source: Source::Path { path: dir.clone() },
                    }),
                    self.optional_req(dependent, name, ws.version.as_deref())?,
                ))
            }
        }
    }

    fn optional_req(
        &self,
        dependent: &PkgKey,
        name: &str,
        req: Option<&str>,
    ) -> Result<VersionRanges, ProviderError> {
        match req {
            Some(req) => self.parse_req(dependent, name, req),
            None => Ok(Ranges::full()),
        }
    }

    fn parse_req(
        &self,
        dependent: &PkgKey,
        dependency: &str,
        req: &str,
    ) -> Result<VersionRanges, ProviderError> {
        let invalid = |message: String| ProviderError::InvalidRequirement {
            dependent: self.label(dependent),
            dependency: dependency.to_owned(),
            requirement: req.to_owned(),
            message,
        };
        let parsed = VersionReq::parse(req).map_err(|e| invalid(e.to_string()))?;
        req_to_ranges(&parsed).map_err(invalid)
    }

    /// The lua-versions incompatibility sentence for a package version, if
    /// any (SPEC.md §6: empty declaration = compatible with everything).
    fn lua_incompatibility(
        &self,
        id: &PackageId,
        version: &Version,
        lua_versions: &[String],
    ) -> Option<String> {
        if lua_versions.is_empty() || lua_versions.iter().any(|v| v == &self.edition) {
            return None;
        }
        Some(format!(
            "{id} {version} supports Lua {} but the project's edition is Lua {}",
            lua_versions.join(", "),
            self.edition
        ))
    }
}

/// The [`pubgrub::DependencyProvider`] adapter around a [`Context`].
struct Adapter<'a> {
    ctx: Context<'a>,
    /// Registry pins from an existing lockfile: name → locked version.
    /// Preferred over higher versions when still satisfying (minimal
    /// churn). Path/git packages have a single candidate anyway.
    locked: BTreeMap<String, Version>,
    /// Version-list cache, sorted highest-first.
    versions: RefCell<BTreeMap<PackageId, Vec<Version>>>,
}

impl Adapter<'_> {
    fn versions_of(&self, id: &PackageId) -> Result<Vec<Version>, ProviderError> {
        if let Some(cached) = self.versions.borrow().get(id) {
            return Ok(cached.clone());
        }
        let mut versions = self.ctx.provider.list_versions(id)?;
        versions.sort();
        versions.dedup();
        versions.reverse();
        self.versions
            .borrow_mut()
            .insert(id.clone(), versions.clone());
        Ok(versions)
    }
}

impl DependencyProvider for Adapter<'_> {
    type P = PkgKey;
    type V = Version;
    type VS = VersionRanges;
    type M = String;
    type Err = ProviderError;
    type Priority = (u32, Reverse<u64>);

    /// Decide conflict-prone packages first, then packages with the fewest
    /// candidates (the classic PubGrub heuristic).
    fn prioritize(
        &self,
        package: &PkgKey,
        range: &VersionRanges,
        stats: &PackageResolutionStatistics,
    ) -> Self::Priority {
        match package {
            PkgKey::Root => (u32::MAX, Reverse(0)),
            PkgKey::Pkg(id) => {
                let candidates = self.versions_of(id).map_or(0, |versions| {
                    versions
                        .iter()
                        .filter(|v| version_matches(range, v))
                        .count()
                });
                (
                    stats.conflict_count(),
                    Reverse(u64::try_from(candidates).unwrap_or(u64::MAX)),
                )
            }
        }
    }

    fn choose_version(
        &self,
        package: &PkgKey,
        range: &VersionRanges,
    ) -> Result<Option<Version>, ProviderError> {
        match package {
            PkgKey::Root => Ok(range
                .contains(&self.ctx.root_version)
                .then(|| self.ctx.root_version.clone())),
            PkgKey::Pkg(id) => {
                let versions = self.versions_of(id)?;
                // Lockfile bias: a still-satisfying pin wins over anything
                // newer, so re-resolves churn only what they must.
                if matches!(id.source, Source::Registry)
                    && let Some(locked) = self.locked.get(&id.name)
                    && version_matches(range, locked)
                    && versions.contains(locked)
                {
                    return Ok(Some(locked.clone()));
                }
                Ok(versions.into_iter().find(|v| version_matches(range, v)))
            }
        }
    }

    fn get_dependencies(
        &self,
        package: &PkgKey,
        version: &Version,
    ) -> Result<Dependencies<PkgKey, VersionRanges, String>, ProviderError> {
        if let PkgKey::Pkg(id) = package {
            let meta = self.ctx.provider.metadata(id, version)?;
            if let Some(reason) = self
                .ctx
                .lua_incompatibility(id, version, &meta.lua_versions)
            {
                // Surface lua-versions mismatch as a PubGrub incompatibility
                // so the conflict report explains it.
                return Ok(Dependencies::Unavailable(reason));
            }
        }
        let mut constraints: DependencyConstraints<PkgKey, VersionRanges> =
            DependencyConstraints::default();
        for (key, ranges) in self.ctx.dependency_entries(package, version)? {
            constraints
                .entry(key)
                .and_modify(|existing| *existing = existing.intersection(&ranges))
                .or_insert(ranges);
        }
        Ok(Dependencies::Available(constraints))
    }
}

/// One package selected by [`resolve`] (the root project is *not* listed;
/// it lives only in the lockfile's own entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: String,
    pub version: Version,
    pub source: Source,
    /// Registry checksum for the lockfile (placeholder until #20).
    pub checksum: Option<String>,
    /// Direct dependencies as lockfile-style refs (sorted, deduplicated).
    pub dependencies: Vec<String>,
}

/// A successful resolution: the selected package set plus the
/// ready-to-serialize lockfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// Sorted by name, then version, then source.
    pub packages: Vec<ResolvedPackage>,
    pub lockfile: Lockfile,
}

/// Why resolution failed. `Display` renders the cargo-style conflict
/// report for [`ResolveError::NoSolution`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No package set satisfies every requirement; `report` is the full
    /// human-readable derivation ("Because X depends on … , version
    /// solving failed.").
    NoSolution { report: String },
    /// A provider failed or a manifest declared something unusable.
    Provider(ProviderError),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSolution { report } => write!(f, "{report}"),
            Self::Provider(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ResolveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSolution { .. } => None,
            Self::Provider(e) => Some(e),
        }
    }
}

impl From<ProviderError> for ResolveError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e)
    }
}

/// Resolves the dependency graph of `root_manifest` (whose directory is
/// `root_dir`) against `provider`.
///
/// With `lockfile`, still-satisfying pins are kept (minimal churn) and
/// stale or requirement-invalidated entries are re-resolved; without one,
/// the highest satisfying version of every package wins.
pub fn resolve(
    root_manifest: &Manifest,
    root_dir: &Path,
    provider: &dyn PackageProvider,
    lockfile: Option<&Lockfile>,
) -> Result<Resolution, ResolveError> {
    let ctx = build_context(root_manifest, root_dir, provider)?;
    let root_version = ctx.root_version.clone();
    let root_name = ctx.root_name.clone();
    let adapter = Adapter {
        ctx,
        locked: lockfile.map(registry_pins).unwrap_or_default(),
        versions: RefCell::new(BTreeMap::new()),
    };
    match pubgrub::resolve(&adapter, PkgKey::Root, root_version) {
        Ok(solution) => build_resolution(&adapter.ctx, &solution),
        Err(PubGrubError::NoSolution(mut tree)) => {
            // Providers know the complete version universe, so folding
            // no-versions nodes into their causes reads better.
            tree.collapse_no_versions();
            let report = DefaultStringReporter::report_with_formatter(
                &tree,
                &ResolveReportFormatter::new(root_name),
            );
            Err(ResolveError::NoSolution { report })
        }
        Err(
            PubGrubError::ErrorRetrievingDependencies { source, .. }
            | PubGrubError::ErrorChoosingVersion { source, .. }
            | PubGrubError::ErrorInShouldCancel(source),
        ) => Err(ResolveError::Provider(source)),
    }
}

fn build_context<'a>(
    root_manifest: &Manifest,
    root_dir: &Path,
    provider: &'a dyn PackageProvider,
) -> Result<Context<'a>, ResolveError> {
    let root_version = Version::parse(&root_manifest.package.version).map_err(|e| {
        ProviderError::InvalidVersion {
            package: root_manifest.package.name.clone(),
            version: root_manifest.package.version.clone(),
            message: e.to_string(),
        }
    })?;
    Ok(Context {
        provider,
        root_dir: normalize_path(root_dir),
        root_name: root_manifest.package.name.clone(),
        root_version,
        edition: root_manifest.package.edition.clone(),
        root_deps: root_manifest.dependencies.clone(),
        root_dev_deps: root_manifest.dev_dependencies.clone(),
        members: workspace_member_map(root_manifest, root_dir)?,
    })
}

/// Reads each workspace member's manifest to map member *package names*
/// onto their directories.
fn workspace_member_map(
    root_manifest: &Manifest,
    root_dir: &Path,
) -> Result<BTreeMap<String, PathBuf>, ProviderError> {
    let mut members = BTreeMap::new();
    for dir in root_manifest.workspace_members(root_dir) {
        let file = dir.join("luabox.toml");
        let text = std::fs::read_to_string(&file).map_err(|e| ProviderError::Io {
            path: file.clone(),
            message: e.to_string(),
        })?;
        let manifest = parse_manifest_at(&file, &text)?;
        members.insert(manifest.package.name.clone(), normalize_path(&dir));
    }
    Ok(members)
}

/// Registry pins carried over from an existing lockfile.
fn registry_pins(lockfile: &Lockfile) -> BTreeMap<String, Version> {
    let mut pins = BTreeMap::new();
    for package in &lockfile.packages {
        if matches!(package.source, Some(LockedSource::Registry)) {
            pins.insert(package.name.clone(), package.version.clone());
        }
    }
    pins
}

fn build_resolution(
    ctx: &Context<'_>,
    solution: &SelectedDependencies<Adapter<'_>>,
) -> Result<Resolution, ResolveError> {
    let mut selected: Vec<(&PackageId, &Version)> = solution
        .iter()
        .filter_map(|(key, version)| match key {
            PkgKey::Pkg(id) => Some((id, version)),
            PkgKey::Root => None,
        })
        .collect();
    selected.sort_by(|(a, av), (b, bv)| (&a.name, *av, &a.source).cmp(&(&b.name, *bv, &b.source)));

    // Names appearing more than once (several sources/versions) get a
    // version suffix in dependency lists, cargo-style.
    let mut name_counts: BTreeMap<&str, usize> = BTreeMap::new();
    *name_counts.entry(ctx.root_name.as_str()).or_default() += 1;
    for (id, _) in &selected {
        *name_counts.entry(id.name.as_str()).or_default() += 1;
    }
    let ambiguous: BTreeSet<&str> = name_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(name, _)| *name)
        .collect();

    let dependency_refs = |entries: &[(PkgKey, VersionRanges)]| -> Vec<String> {
        let mut refs: Vec<String> = entries
            .iter()
            .filter_map(|(key, _)| match key {
                PkgKey::Pkg(id) => Some(if ambiguous.contains(id.name.as_str()) {
                    match solution.get(key) {
                        Some(version) => format!("{} {version}", id.name),
                        None => id.name.clone(),
                    }
                } else {
                    id.name.clone()
                }),
                PkgKey::Root => None,
            })
            .collect();
        refs.sort();
        refs.dedup();
        refs
    };

    let mut packages = Vec::with_capacity(selected.len());
    let mut lock_packages = Vec::with_capacity(selected.len() + 1);

    let root_entries = ctx.dependency_entries(&PkgKey::Root, &ctx.root_version)?;
    lock_packages.push(LockedPackage {
        name: ctx.root_name.clone(),
        version: ctx.root_version.clone(),
        source: None,
        checksum: None,
        dependencies: dependency_refs(&root_entries),
    });

    for (id, version) in selected {
        let meta = ctx.provider.metadata(id, version)?;
        let entries = ctx.dependency_entries(&PkgKey::Pkg(id.clone()), version)?;
        let dependencies = dependency_refs(&entries);
        let locked_source = match &id.source {
            Source::Registry => LockedSource::Registry,
            Source::Path { path } => LockedSource::Path {
                path: relative_display(&ctx.root_dir, path),
            },
            Source::Git { url, reference } => LockedSource::Git {
                // Pin the resolved commit sha when the provider reports one
                // (SPEC.md §6: reproducible installs even for branch refs);
                // fall back to the symbolic reference otherwise.
                spec: match &meta.pinned {
                    Some(commit) => format!("{url}#{commit}"),
                    None => format!("{url}#{reference}"),
                },
            },
            Source::Url { url, .. } => LockedSource::Url { url: url.clone() },
        };
        packages.push(ResolvedPackage {
            name: id.name.clone(),
            version: version.clone(),
            source: id.source.clone(),
            checksum: meta.checksum.clone(),
            dependencies: dependencies.clone(),
        });
        lock_packages.push(LockedPackage {
            name: id.name.clone(),
            version: version.clone(),
            source: Some(locked_source),
            checksum: meta.checksum,
            dependencies,
        });
    }

    Ok(Resolution {
        packages,
        lockfile: Lockfile::new(lock_packages),
    })
}

/// Independent checker for SPEC.md §16.2's "solution respects every
/// requirement" invariant: every selected package's requirements are
/// satisfied by the selected versions, and every selected package admits
/// the project's edition. Returns all violations, newline-joined.
pub fn verify_resolution(
    root_manifest: &Manifest,
    root_dir: &Path,
    provider: &dyn PackageProvider,
    resolution: &Resolution,
) -> Result<(), String> {
    let ctx = build_context(root_manifest, root_dir, provider)
        .map_err(|e| format!("verification could not build a context: {e}"))?;

    let mut chosen: Vec<(PkgKey, Version)> = vec![(PkgKey::Root, ctx.root_version.clone())];
    for package in &resolution.packages {
        chosen.push((
            PkgKey::Pkg(PackageId {
                name: package.name.clone(),
                source: package.source.clone(),
            }),
            package.version.clone(),
        ));
    }
    let version_of = |key: &PkgKey| -> Option<&Version> {
        chosen
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, version)| version)
    };

    let mut violations = Vec::new();
    for (key, version) in &chosen {
        if let PkgKey::Pkg(id) = key {
            match ctx.provider.metadata(id, version) {
                Ok(meta) => {
                    if let Some(reason) = ctx.lua_incompatibility(id, version, &meta.lua_versions) {
                        violations.push(reason);
                    }
                }
                Err(e) => violations.push(e.to_string()),
            }
        }
        match ctx.dependency_entries(key, version) {
            Ok(entries) => {
                for (dep_key, ranges) in entries {
                    match version_of(&dep_key) {
                        None => violations.push(format!(
                            "{} requires {dep_key}, which is not in the solution",
                            ctx.label(key)
                        )),
                        Some(dep_version) => {
                            if !version_matches(&ranges, dep_version) {
                                violations.push(format!(
                                    "{} requires {dep_key} {}, but {dep_version} was selected",
                                    ctx.label(key),
                                    crate::semver_ranges::display_ranges(&ranges)
                                ));
                            }
                        }
                    }
                }
            }
            Err(e) => violations.push(e.to_string()),
        }
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations.join("\n"))
    }
}
