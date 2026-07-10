//! Package metadata sources for the resolver (SPEC.md §6).
//!
//! [`PackageProvider`] is the boundary between the PubGrub solver and
//! wherever package metadata physically lives. Two implementations ship
//! today:
//!
//! - [`PathProvider`] — path/workspace dependencies, read from each
//!   dependency's `luabox.toml` on disk.
//! - [`StaticProvider`] — an in-memory package universe for tests and
//!   benchmarks.
//!
//! The registry client (sparse index, #20) and the git fetcher (#21) plug
//! in behind this same trait; [`StackedProvider`] chains providers so a
//! project can mix source kinds in one resolve.
//!
//! Package names are plain strings and deliberately admit both flat
//! (`penlight`) and scoped (`@org/pkg`, SPEC.md §19) forms.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use semver::Version;

use crate::manifest::{Dependency, Manifest};

/// Where a package's contents come from. Part of package *identity*: the
/// same name from two sources is two different packages to the solver.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// The first-party registry (SPEC.md §6). Client lands in #20.
    Registry,
    /// A directory on disk containing a `luabox.toml` (path and workspace
    /// dependencies). The path is lexically normalized and absolute.
    Path { path: PathBuf },
    /// A git repository (fetcher lands in #21).
    Git {
        url: String,
        reference: GitReference,
    },
}

/// Which ref a git dependency pins (SPEC.md §6: rev/tag/branch).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GitReference {
    Rev(String),
    Tag(String),
    Branch(String),
    /// No ref given: the remote's default branch.
    DefaultBranch,
}

impl fmt::Display for GitReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rev(r) | Self::Tag(r) | Self::Branch(r) => write!(f, "{r}"),
            Self::DefaultBranch => write!(f, "HEAD"),
        }
    }
}

/// A package identity: name plus source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PackageId {
    pub name: String,
    pub source: Source,
}

impl PackageId {
    #[must_use]
    pub fn registry(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: Source::Registry,
        }
    }

    #[must_use]
    pub fn path(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            source: Source::Path {
                path: normalize_path(&path.into()),
            },
        }
    }

    #[must_use]
    pub fn git(name: impl Into<String>, url: impl Into<String>, reference: GitReference) -> Self {
        Self {
            name: name.into(),
            source: Source::Git {
                url: url.into(),
                reference,
            },
        }
    }
}

impl fmt::Display for PackageId {
    /// Registry packages read as their bare name; path/git packages carry a
    /// short source tag so same-named packages stay distinguishable in
    /// conflict reports.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Source::Registry => write!(f, "{}", self.name),
            Source::Path { path } => {
                write!(f, "{} ({})", self.name, display_path(path))
            }
            Source::Git { url, reference } => {
                write!(f, "{} ({url}#{reference})", self.name)
            }
        }
    }
}

/// Per-version metadata the resolver and lockfile need beyond the
/// dependency map.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageMeta {
    /// SPEC.md §6 `lua-versions`: dialects the package declares itself
    /// compatible with. Empty means compatible with all.
    pub lua_versions: Vec<String>,
    /// Content checksum for the lockfile (`sha256:…`). Plain string here;
    /// verification against the store is #18/#21 integration.
    pub checksum: Option<String>,
}

/// Errors a provider (or the solver's manifest-shaped dependency
/// conversion) can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// This provider does not handle the package's source kind.
    /// [`StackedProvider`] falls through to the next provider on this.
    UnsupportedSource {
        package: String,
    },
    /// No package of this name/source is known.
    /// [`StackedProvider`] also falls through on this.
    UnknownPackage {
        package: String,
    },
    /// The named version of a known package does not exist.
    VersionNotFound {
        package: String,
        version: String,
    },
    Io {
        path: PathBuf,
        message: String,
    },
    InvalidManifest {
        path: PathBuf,
        message: String,
    },
    /// A path dependency's directory manifest declares a different name.
    NameMismatch {
        expected: String,
        found: String,
        path: PathBuf,
    },
    InvalidVersion {
        package: String,
        version: String,
        message: String,
    },
    InvalidRequirement {
        dependent: String,
        dependency: String,
        requirement: String,
        message: String,
    },
    /// `pkg = { workspace = true }` named a package that is not a
    /// workspace member.
    NotAWorkspaceMember {
        dependency: String,
        dependent: String,
    },
    /// Registry/git packages cannot declare path or workspace dependencies.
    PathDependencyNotAllowed {
        dependency: String,
        dependent: String,
    },
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSource { package } => {
                write!(f, "no provider supports the source of `{package}`")
            }
            Self::UnknownPackage { package } => {
                write!(f, "no package named `{package}` was found")
            }
            Self::VersionNotFound { package, version } => {
                write!(f, "package `{package}` has no version {version}")
            }
            Self::Io { path, message } => {
                write!(f, "failed to read `{}`: {message}", display_path(path))
            }
            Self::InvalidManifest { path, message } => {
                write!(f, "invalid manifest `{}`: {message}", display_path(path))
            }
            Self::NameMismatch {
                expected,
                found,
                path,
            } => write!(
                f,
                "path dependency `{expected}` points at `{}`, which declares package name `{found}`",
                display_path(path)
            ),
            Self::InvalidVersion {
                package,
                version,
                message,
            } => write!(
                f,
                "package `{package}` has invalid version `{version}`: {message}"
            ),
            Self::InvalidRequirement {
                dependent,
                dependency,
                requirement,
                message,
            } => write!(
                f,
                "`{dependent}` has an invalid requirement `{requirement}` on `{dependency}`: {message}"
            ),
            Self::NotAWorkspaceMember {
                dependency,
                dependent,
            } => write!(
                f,
                "`{dependent}` declares `{dependency} = {{ workspace = true }}`, but `{dependency}` is not a workspace member"
            ),
            Self::PathDependencyNotAllowed {
                dependency,
                dependent,
            } => write!(
                f,
                "`{dependent}` is not a local package, so it cannot declare the path/workspace dependency `{dependency}`"
            ),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The seam between the solver and package metadata storage (SPEC.md §6).
///
/// Implementations must be *deterministic*: identical inputs must produce
/// identical outputs, or the lockfile determinism invariants (SPEC.md
/// §16.2) do not hold. Order of [`list_versions`](Self::list_versions) is
/// irrelevant (the solver sorts); contents are not.
pub trait PackageProvider {
    /// Every available version of `package`, in any order.
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError>;

    /// The manifest-shaped dependency map of one version of `package`
    /// (its `[dependencies]`; `[dev-dependencies]` of non-root packages
    /// never participate in resolution).
    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError>;

    /// Compatibility + lockfile metadata of one version of `package`.
    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError>;
}

/// Provider for [`Source::Path`] packages: reads (and caches) each
/// directory's `luabox.toml`. A path package has exactly one version —
/// whatever its manifest declares.
#[derive(Debug, Default)]
pub struct PathProvider {
    manifests: RefCell<BTreeMap<PathBuf, Manifest>>,
}

impl PathProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads (or fetches from cache) the manifest for a path package and
    /// projects it through `f`.
    fn with_manifest<R>(
        &self,
        package: &PackageId,
        f: impl FnOnce(&Manifest) -> R,
    ) -> Result<R, ProviderError> {
        let Source::Path { path } = &package.source else {
            return Err(ProviderError::UnsupportedSource {
                package: package.to_string(),
            });
        };
        let mut cache = self.manifests.borrow_mut();
        if !cache.contains_key(path) {
            let file = path.join("luabox.toml");
            let text = std::fs::read_to_string(&file).map_err(|e| ProviderError::Io {
                path: file.clone(),
                message: e.to_string(),
            })?;
            let manifest =
                Manifest::parse(&text).map_err(|errors| ProviderError::InvalidManifest {
                    path: file.clone(),
                    message: errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                })?;
            cache.insert(path.clone(), manifest);
        }
        let manifest = &cache[path];
        if manifest.package.name != package.name {
            return Err(ProviderError::NameMismatch {
                expected: package.name.clone(),
                found: manifest.package.name.clone(),
                path: path.clone(),
            });
        }
        Ok(f(manifest))
    }

    fn manifest_version(
        package: &PackageId,
        manifest: &Manifest,
    ) -> Result<Version, ProviderError> {
        Version::parse(&manifest.package.version).map_err(|e| ProviderError::InvalidVersion {
            package: package.to_string(),
            version: manifest.package.version.clone(),
            message: e.to_string(),
        })
    }
}

impl PackageProvider for PathProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        self.with_manifest(package, |m| {
            Self::manifest_version(package, m).map(|v| vec![v])
        })?
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        self.with_manifest(package, |m| {
            let actual = Self::manifest_version(package, m)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(m.dependencies.clone())
        })?
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        self.with_manifest(package, |m| {
            let actual = Self::manifest_version(package, m)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(PackageMeta {
                lua_versions: m.package.lua_versions.clone(),
                checksum: None,
            })
        })?
    }
}

#[derive(Debug, Clone, Default)]
struct StaticPackage {
    dependencies: BTreeMap<String, Dependency>,
    meta: PackageMeta,
}

/// In-memory package universe: the test/bench provider, and the reference
/// for what #20's registry client must behave like.
#[derive(Debug, Clone, Default)]
pub struct StaticProvider {
    packages: BTreeMap<PackageId, BTreeMap<Version, StaticPackage>>,
}

impl StaticProvider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a registry package version whose dependencies are
    /// `(name, version-requirement)` pairs.
    ///
    /// # Panics
    /// Panics on an unparsable `version` — this is a test-universe builder.
    pub fn add(&mut self, name: &str, version: &str, deps: &[(&str, &str)]) {
        self.add_full(name, version, deps, &[], None);
    }

    /// [`Self::add`] plus a `lua-versions` declaration.
    ///
    /// # Panics
    /// Panics on an unparsable `version`.
    pub fn add_with_lua(
        &mut self,
        name: &str,
        version: &str,
        deps: &[(&str, &str)],
        lua_versions: &[&str],
    ) {
        self.add_full(name, version, deps, lua_versions, None);
    }

    /// [`Self::add`] plus lua-versions and a lockfile checksum.
    ///
    /// # Panics
    /// Panics on an unparsable `version`.
    pub fn add_full(
        &mut self,
        name: &str,
        version: &str,
        deps: &[(&str, &str)],
        lua_versions: &[&str],
        checksum: Option<&str>,
    ) {
        let dependencies = deps
            .iter()
            .map(|(dep, req)| ((*dep).to_owned(), Dependency::Version((*req).to_owned())))
            .collect();
        self.add_package(
            PackageId::registry(name),
            Version::parse(version).expect("StaticProvider::add: valid semver version"),
            dependencies,
            PackageMeta {
                lua_versions: lua_versions.iter().map(|s| (*s).to_owned()).collect(),
                checksum: checksum.map(str::to_owned),
            },
        );
    }

    /// Registers one version of an arbitrary package (any source kind, any
    /// dependency kinds) — the fully general form.
    pub fn add_package(
        &mut self,
        id: PackageId,
        version: Version,
        dependencies: BTreeMap<String, Dependency>,
        meta: PackageMeta,
    ) {
        self.packages
            .entry(id)
            .or_default()
            .insert(version, StaticPackage { dependencies, meta });
    }

    fn versions_of(
        &self,
        package: &PackageId,
    ) -> Result<&BTreeMap<Version, StaticPackage>, ProviderError> {
        self.packages
            .get(package)
            .ok_or_else(|| ProviderError::UnknownPackage {
                package: package.to_string(),
            })
    }

    fn version_of(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<&StaticPackage, ProviderError> {
        self.versions_of(package)?
            .get(version)
            .ok_or_else(|| ProviderError::VersionNotFound {
                package: package.to_string(),
                version: version.to_string(),
            })
    }
}

impl PackageProvider for StaticProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        Ok(self.versions_of(package)?.keys().cloned().collect())
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        Ok(self.version_of(package, version)?.dependencies.clone())
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        Ok(self.version_of(package, version)?.meta.clone())
    }
}

/// Chains providers: the first one that recognizes the package (does not
/// answer [`ProviderError::UnsupportedSource`] or
/// [`ProviderError::UnknownPackage`]) wins. This is how registry (#20),
/// git (#21), path, and luarocks-bridge providers will compose.
pub struct StackedProvider<'a> {
    providers: Vec<&'a dyn PackageProvider>,
}

impl<'a> StackedProvider<'a> {
    #[must_use]
    pub fn new(providers: Vec<&'a dyn PackageProvider>) -> Self {
        Self { providers }
    }

    fn first_supporting<T>(
        &self,
        package: &PackageId,
        f: impl Fn(&dyn PackageProvider) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let mut last: Option<ProviderError> = None;
        for provider in &self.providers {
            match f(*provider) {
                Err(
                    e @ (ProviderError::UnsupportedSource { .. }
                    | ProviderError::UnknownPackage { .. }),
                ) => last = Some(e),
                other => return other,
            }
        }
        Err(last.unwrap_or_else(|| ProviderError::UnsupportedSource {
            package: package.to_string(),
        }))
    }
}

impl PackageProvider for StackedProvider<'_> {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        self.first_supporting(package, |p| p.list_versions(package))
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        self.first_supporting(package, |p| p.dependencies(package, version))
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        self.first_supporting(package, |p| p.metadata(package, version))
    }
}

/// Lexically normalizes a path: resolves `.`/`..` segments without touching
/// the filesystem (so path identity is stable even for not-yet-fetched
/// trees, and no `\\?\` canonical prefixes leak into messages on Windows).
#[must_use]
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(Component::ParentDir);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Forward-slash rendering of a path (deterministic across platforms).
#[must_use]
pub(crate) fn display_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// `path` relative to `root` (both lexically normalized), rendered with
/// forward slashes for deterministic lockfiles. Falls back to the absolute
/// form when the two share no common prefix (e.g. different drives).
#[must_use]
pub(crate) fn relative_display(root: &Path, path: &Path) -> String {
    let root_components: Vec<Component<'_>> = root.components().collect();
    let path_components: Vec<Component<'_>> = path.components().collect();
    let common = root_components
        .iter()
        .zip(&path_components)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return display_path(path);
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in common..root_components.len() {
        parts.push("..".to_owned());
    }
    for component in &path_components[common..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(
            normalize_path(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
        assert_eq!(normalize_path(Path::new("../x")), PathBuf::from("../x"));
        assert_eq!(
            normalize_path(Path::new("a/../../b")),
            PathBuf::from("../b")
        );
    }

    #[test]
    fn relative_display_forms() {
        let root = PathBuf::from("/w/proj");
        assert_eq!(
            relative_display(&root, Path::new("/w/proj/libs/a")),
            "libs/a"
        );
        assert_eq!(
            relative_display(&root, Path::new("/w/sibling")),
            "../sibling"
        );
        assert_eq!(relative_display(&root, Path::new("/w/proj")), ".");
    }

    #[test]
    fn static_provider_round_trips_metadata() {
        let mut p = StaticProvider::new();
        p.add_full("a", "1.2.3", &[("b", "^1")], &["5.1"], Some("sha256:aa"));
        let id = PackageId::registry("a");
        let v = Version::parse("1.2.3").unwrap();
        assert_eq!(p.list_versions(&id).unwrap(), vec![v.clone()]);
        let meta = p.metadata(&id, &v).unwrap();
        assert_eq!(meta.lua_versions, vec!["5.1".to_owned()]);
        assert_eq!(meta.checksum.as_deref(), Some("sha256:aa"));
        assert!(p.dependencies(&id, &v).unwrap().contains_key("b"));
    }

    #[test]
    fn static_provider_unknown_package_and_version() {
        let mut p = StaticProvider::new();
        p.add("a", "1.0.0", &[]);
        assert!(matches!(
            p.list_versions(&PackageId::registry("nope")),
            Err(ProviderError::UnknownPackage { .. })
        ));
        assert!(matches!(
            p.dependencies(&PackageId::registry("a"), &Version::new(9, 9, 9)),
            Err(ProviderError::VersionNotFound { .. })
        ));
    }

    #[test]
    fn path_provider_reads_manifest_and_checks_name() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("libs").join("a");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("luabox.toml"),
            "[package]\nname = \"a\"\nversion = \"0.3.1\"\nedition = \"5.4\"\nlua-versions = [\"5.4\"]\n",
        )
        .unwrap();

        let provider = PathProvider::new();
        let id = PackageId::path("a", &dir);
        let versions = provider.list_versions(&id).unwrap();
        assert_eq!(versions, vec![Version::parse("0.3.1").unwrap()]);
        let meta = provider.metadata(&id, &versions[0]).unwrap();
        assert_eq!(meta.lua_versions, vec!["5.4".to_owned()]);
        assert_eq!(meta.checksum, None);

        // Wrong name at that path is a NameMismatch, not silent success.
        let wrong = PackageId::path("b", &dir);
        assert!(matches!(
            provider.list_versions(&wrong),
            Err(ProviderError::NameMismatch { .. })
        ));

        // Non-path sources are not this provider's job.
        assert!(matches!(
            provider.list_versions(&PackageId::registry("a")),
            Err(ProviderError::UnsupportedSource { .. })
        ));
    }

    #[test]
    fn stacked_provider_falls_through() {
        let mut registry = StaticProvider::new();
        registry.add("a", "1.0.0", &[]);
        let paths = PathProvider::new();
        let stacked = StackedProvider::new(vec![&paths, &registry]);
        // PathProvider says UnsupportedSource for a registry id; the static
        // universe answers.
        assert_eq!(
            stacked
                .list_versions(&PackageId::registry("a"))
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            stacked.list_versions(&PackageId::registry("missing")),
            Err(ProviderError::UnknownPackage { .. })
        ));
    }
}
