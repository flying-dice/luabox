//! First-party registry client (SPEC.md §6, ticket #20).
//!
//! The registry is a **static-CDN sparse index** on the crates.io model: no
//! server-side application, just files at well-known paths under a single
//! root. That root is named by the `LUABOX_REGISTRY` environment variable
//! ([`REGISTRY_ENV`]) and may be a plain directory path, a `file://` URL, or
//! an `http(s)://` base URL. There is **no hosted default registry yet** —
//! callers must error with setup guidance when the variable is absent.
//!
//! # Layout
//!
//! ```text
//! <root>/
//!   index/<name-path>/<name>     # one JSON-lines file per package
//!   artifacts/<name>/<version>.tar   # one packed tree per published version
//! ```
//!
//! `<name-path>` follows crates.io's prefix rules ([`index_rel_path`]):
//! 1-char names under `1/`, 2-char under `2/`, 3-char under `3/<first>/`,
//! longer names under `<first-2>/<next-2>/`. Scoped names (`@org/pkg`,
//! SPEC.md §19) use their organization as the directory: `index/@org/pkg`.
//!
//! Each index line is one published version ([`IndexEntry`]):
//!
//! ```json
//! {"name":"penlight","version":"1.14.0","deps":[{"name":"luafilesystem","req":"^1.8","dev":false}],"lua_versions":["5.1","5.4"],"checksum":"sha256:…","yanked":false}
//! ```
//!
//! `checksum` is the content-addressed store's **tree hash**
//! (`luabox-store`'s `TreeManifest::tree_hash`, prefixed `sha256:`) of the
//! extracted artifact tree — the same value `luabox install` recomputes via
//! `put_tree` after extraction, so a tampered artifact cannot materialize.
//!
//! # Mutation rules (crates.io semantics)
//!
//! Publishing appends a line; a `name@version` line is **immutable** —
//! re-publishing the same version is refused ([`Registry::publish`]).
//! Yanking ([`Registry::set_yanked`]) flips the `yanked` flag in place;
//! nothing is ever deleted. [`RegistryProvider::list_versions`] skips yanked
//! versions for *new* resolutions, but versions pinned by an existing
//! lockfile stay resolvable ([`RegistryProvider::with_locked`]) — the
//! crates.io yank contract.
//!
//! # Transport
//!
//! - Directory / `file://` roots read and write the filesystem directly —
//!   the MVP transport, and the only *writable* one.
//! - `http(s)://` roots are read-only and fetched best-effort with
//!   `curl -fsS` (mirroring the toolchain installer's approach — no HTTP
//!   crate in the dependency tree). Publishing to an HTTP root is refused
//!   with guidance.
//!
//! Artifact signing (sigstore, SPEC.md §6) is **out of scope for this MVP**;
//! integrity rests on the tree-hash checksum recorded in the index.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::lockfile::{LockedSource, Lockfile};
use crate::manifest::Dependency;
use crate::provider::{PackageId, PackageMeta, PackageProvider, ProviderError, Source};

/// Environment variable naming the registry root (path, `file://`, or
/// `http(s)://`).
pub const REGISTRY_ENV: &str = "LUABOX_REGISTRY";

/// One dependency declaration on an index line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexDep {
    pub name: String,
    /// Semver requirement string (`"^1.2"` …) — registry packages may only
    /// depend on other registry packages.
    pub req: String,
    /// Dev-dependency of the published package. Recorded for tooling;
    /// never participates in a consumer's resolution.
    #[serde(default)]
    pub dev: bool,
}

/// One published version: one JSON line in the package's index file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    /// Exact semver version.
    pub version: String,
    #[serde(default)]
    pub deps: Vec<IndexDep>,
    /// SPEC.md §6 `lua-versions` compatibility declaration; empty means
    /// compatible with every dialect.
    #[serde(default)]
    pub lua_versions: Vec<String>,
    /// `sha256:<tree-hash>` of the extracted artifact tree.
    pub checksum: String,
    /// Yanked versions are hidden from new resolutions but never deleted.
    #[serde(default)]
    pub yanked: bool,
}

impl IndexEntry {
    /// Canonical single-line JSON encoding (what [`Registry::publish`]
    /// appends).
    ///
    /// # Panics
    /// Never in practice: serializing this plain data type cannot fail.
    #[must_use]
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("IndexEntry serialization cannot fail")
    }

    /// Parse one index line.
    pub fn parse(line: &str) -> Result<Self, String> {
        serde_json::from_str(line).map_err(|e| e.to_string())
    }
}

/// What went wrong talking to (or mutating) a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    /// `LUABOX_REGISTRY` held something unusable.
    InvalidSpec { spec: String, message: String },
    /// A write (publish/yank) was attempted against a read-only transport.
    ReadOnly { location: String },
    /// Local filesystem failure.
    Io { path: PathBuf, message: String },
    /// `curl` failure fetching an HTTP registry resource.
    Http { url: String, message: String },
    /// A line in an index file did not parse.
    InvalidIndex { location: String, message: String },
    /// Publish refused: this exact version is already in the index.
    DuplicateVersion { name: String, version: String },
    /// Yank target does not exist in the index.
    VersionNotInIndex { name: String, version: String },
    /// The index references an artifact that is not present.
    ArtifactMissing { name: String, version: String },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpec { spec, message } => {
                write!(f, "invalid registry `{spec}`: {message}")
            }
            Self::ReadOnly { location } => write!(
                f,
                "registry `{location}` is read-only: publishing needs a \
                 filesystem (or file://) registry root in this MVP"
            ),
            Self::Io { path, message } => {
                write!(f, "registry I/O error at `{}`: {message}", path.display())
            }
            Self::Http { url, message } => write!(f, "failed to fetch `{url}`: {message}"),
            Self::InvalidIndex { location, message } => {
                write!(f, "malformed registry index `{location}`: {message}")
            }
            Self::DuplicateVersion { name, version } => write!(
                f,
                "`{name}@{version}` is already published; registry versions are \
                 immutable — bump the version, or yank it with \
                 `luabox publish --yank {version}`"
            ),
            Self::VersionNotInIndex { name, version } => {
                write!(f, "`{name}` has no published version {version}")
            }
            Self::ArtifactMissing { name, version } => write!(
                f,
                "the registry index lists `{name}@{version}` but its artifact \
                 is missing"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

/// The crates.io-style prefix path for `name` (without the `index/` root):
/// `1/a`, `2/ab`, `3/a/abc`, `ab/cd/abcdef`; scoped names keep their
/// organization directory (`@org/pkg`).
#[must_use]
pub fn index_rel_path(name: &str) -> String {
    if name.contains('/') {
        // Scoped `@org/pkg`: the org is the directory.
        return name.to_owned();
    }
    match name.chars().count() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => match name.get(..1) {
            Some(first) => format!("3/{first}/{name}"),
            None => format!("3/{name}"),
        },
        _ => match (name.get(..2), name.get(2..4)) {
            (Some(a), Some(b)) => format!("{a}/{b}/{name}"),
            // Non-ASCII prefix that has no clean char boundary: fall back
            // to a flat file rather than panicking on a byte slice.
            _ => name.to_owned(),
        },
    }
}

/// Where a registry physically lives.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Root {
    /// A local directory (also the target of `file://` URLs). Read-write.
    Dir(PathBuf),
    /// An `http(s)://` base URL. Read-only, fetched with `curl`.
    Http(String),
}

/// A handle to one registry root. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    root: Root,
}

impl Registry {
    /// Bind to the registry named by `spec`: a directory path, a `file://`
    /// URL, or an `http(s)://` base URL (see [`REGISTRY_ENV`]).
    pub fn open(spec: &str) -> Result<Self, RegistryError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(RegistryError::InvalidSpec {
                spec: spec.to_owned(),
                message: "empty registry location".to_owned(),
            });
        }
        if spec.starts_with("http://") || spec.starts_with("https://") {
            return Ok(Self {
                root: Root::Http(spec.trim_end_matches('/').to_owned()),
            });
        }
        let path = if let Some(rest) = spec.strip_prefix("file://") {
            // `file:///C:/x` (windows) and `file:///home/x` (unix): strip
            // the empty-authority slash before a Windows drive letter.
            let trimmed = if cfg!(windows) {
                rest.trim_start_matches('/')
            } else {
                rest
            };
            PathBuf::from(trimmed)
        } else {
            PathBuf::from(spec)
        };
        Ok(Self {
            root: Root::Dir(path),
        })
    }

    /// Human-readable location for messages.
    #[must_use]
    pub fn location(&self) -> String {
        match &self.root {
            Root::Dir(dir) => dir.display().to_string().replace('\\', "/"),
            Root::Http(base) => base.clone(),
        }
    }

    /// Whether this transport supports publish/yank (filesystem only).
    #[must_use]
    pub fn is_writable(&self) -> bool {
        matches!(self.root, Root::Dir(_))
    }

    /// Registry-relative path of `name`'s index file.
    fn index_rel(name: &str) -> String {
        format!("index/{}", index_rel_path(name))
    }

    /// Registry-relative path of the packed tree for `name@version`.
    fn artifact_rel(name: &str, version: &str) -> String {
        format!("artifacts/{name}/{version}.tar")
    }

    /// Every published version of `name`, index order (publish order).
    /// `Ok(None)` means the package has never been published.
    pub fn load_entries(&self, name: &str) -> Result<Option<Vec<IndexEntry>>, RegistryError> {
        let rel = Self::index_rel(name);
        let (text, location) = match &self.root {
            Root::Dir(dir) => {
                let path = dir.join(&rel);
                match fs::read_to_string(&path) {
                    Ok(text) => (text, path.display().to_string()),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(e) => {
                        return Err(RegistryError::Io {
                            path,
                            message: e.to_string(),
                        });
                    }
                }
            }
            Root::Http(base) => {
                let url = format!("{base}/{rel}");
                match http_get_text(&url)? {
                    Some(text) => (text, url),
                    None => return Ok(None),
                }
            }
        };
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            entries.push(IndexEntry::parse(line).map_err(|message| {
                RegistryError::InvalidIndex {
                    location: location.clone(),
                    message,
                }
            })?);
        }
        Ok(Some(entries))
    }

    /// Obtain the artifact tar for `name@version` as a local file:
    /// filesystem registries serve it in place, HTTP registries download it
    /// into `staging` with `curl`.
    pub fn fetch_artifact(
        &self,
        name: &str,
        version: &str,
        staging: &Path,
    ) -> Result<PathBuf, RegistryError> {
        let rel = Self::artifact_rel(name, version);
        match &self.root {
            Root::Dir(dir) => {
                let path = dir.join(&rel);
                if path.is_file() {
                    Ok(path)
                } else {
                    Err(RegistryError::ArtifactMissing {
                        name: name.to_owned(),
                        version: version.to_owned(),
                    })
                }
            }
            Root::Http(base) => {
                let url = format!("{base}/{rel}");
                let dest = staging.join(format!("{version}.tar"));
                let status = Command::new("curl")
                    .args(["-fsSL", "--max-time", "300", "-o"])
                    .arg(&dest)
                    .arg(&url)
                    .status()
                    .map_err(|e| RegistryError::Http {
                        url: url.clone(),
                        message: format!("failed to run `curl`: {e}"),
                    })?;
                if !status.success() {
                    return Err(RegistryError::Http {
                        url,
                        message: format!("`curl` exited with {status}"),
                    });
                }
                Ok(dest)
            }
        }
    }

    /// Publish one version: copy the packed tree into `artifacts/` and
    /// append the index line. Refuses a duplicate `name@version` — published
    /// versions are immutable (yank instead). Filesystem transport only.
    pub fn publish(&self, entry: &IndexEntry, artifact_tar: &Path) -> Result<(), RegistryError> {
        let Root::Dir(dir) = &self.root else {
            return Err(RegistryError::ReadOnly {
                location: self.location(),
            });
        };
        if let Some(existing) = self.load_entries(&entry.name)?
            && existing.iter().any(|e| e.version == entry.version)
        {
            return Err(RegistryError::DuplicateVersion {
                name: entry.name.clone(),
                version: entry.version.clone(),
            });
        }

        // Artifact first, index line second: the line is the commit point,
        // so a crash in between leaves an unreferenced artifact, never a
        // dangling index entry.
        let artifact_path = dir.join(Self::artifact_rel(&entry.name, &entry.version));
        write_parent_dirs(&artifact_path)?;
        fs::copy(artifact_tar, &artifact_path).map_err(|e| RegistryError::Io {
            path: artifact_path.clone(),
            message: e.to_string(),
        })?;

        let index_path = dir.join(Self::index_rel(&entry.name));
        write_parent_dirs(&index_path)?;
        let mut line = entry.to_json_line();
        line.push('\n');
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&index_path)
            .and_then(|mut file| file.write_all(line.as_bytes()))
            .map_err(|e| RegistryError::Io {
                path: index_path,
                message: e.to_string(),
            })
    }

    /// Flip the `yanked` flag of one published version (crates.io rule:
    /// yank hides, never deletes). Returns `true` if the flag changed,
    /// `false` if it already had the requested value. Filesystem only.
    pub fn set_yanked(
        &self,
        name: &str,
        version: &str,
        yanked: bool,
    ) -> Result<bool, RegistryError> {
        let Root::Dir(dir) = &self.root else {
            return Err(RegistryError::ReadOnly {
                location: self.location(),
            });
        };
        let mut entries =
            self.load_entries(name)?
                .ok_or_else(|| RegistryError::VersionNotInIndex {
                    name: name.to_owned(),
                    version: version.to_owned(),
                })?;
        let entry = entries
            .iter_mut()
            .find(|e| e.version == version)
            .ok_or_else(|| RegistryError::VersionNotInIndex {
                name: name.to_owned(),
                version: version.to_owned(),
            })?;
        if entry.yanked == yanked {
            return Ok(false);
        }
        entry.yanked = yanked;
        let index_path = dir.join(Self::index_rel(name));
        let mut text = String::new();
        for entry in &entries {
            text.push_str(&entry.to_json_line());
            text.push('\n');
        }
        fs::write(&index_path, text).map_err(|e| RegistryError::Io {
            path: index_path,
            message: e.to_string(),
        })?;
        Ok(true)
    }
}

/// Create the parent directories of `path`.
fn write_parent_dirs(path: &Path) -> Result<(), RegistryError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| RegistryError::Io {
            path: parent.to_path_buf(),
            message: e.to_string(),
        })?;
    }
    Ok(())
}

/// `curl -fsS <url>` capturing stdout. `Ok(None)` on HTTP-level failure
/// (curl exit 22 — typically 404: package not in the index); hard error on
/// anything else. Best-effort HTTP, mirroring the toolchain installer.
fn http_get_text(url: &str) -> Result<Option<String>, RegistryError> {
    let output = Command::new("curl")
        .args(["-fsS", "--max-time", "60"])
        .arg(url)
        .output()
        .map_err(|e| RegistryError::Http {
            url: url.to_owned(),
            message: format!("failed to run `curl`: {e}"),
        })?;
    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
    }
    if output.status.code() == Some(22) {
        return Ok(None);
    }
    Err(RegistryError::Http {
        url: url.to_owned(),
        message: format!(
            "`curl` exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    })
}

/// [`PackageProvider`] over a sparse-index [`Registry`] — the provider the
/// solver stacks behind path and git providers (SPEC.md §6).
///
/// Yanked versions are invisible to [`Self::list_versions`] unless an
/// existing lockfile pins them ([`Self::with_locked`]): new resolutions
/// avoid yanked releases, existing lockfiles keep restoring them.
#[derive(Debug)]
pub struct RegistryProvider {
    registry: Registry,
    /// `name -> version` registry pins from an existing lockfile — the
    /// yank exemptions.
    locked: BTreeMap<String, Version>,
    /// Per-process index cache: one read per package name per resolve.
    cache: RefCell<BTreeMap<String, Option<Vec<IndexEntry>>>>,
}

impl RegistryProvider {
    #[must_use]
    pub fn new(registry: Registry) -> Self {
        Self {
            registry,
            locked: BTreeMap::new(),
            cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// Exempt this lockfile's registry pins from yank filtering, so a
    /// project whose locked version was yanked upstream still restores.
    #[must_use]
    pub fn with_locked(mut self, lockfile: &Lockfile) -> Self {
        for package in &lockfile.packages {
            if matches!(package.source, Some(LockedSource::Registry)) {
                self.locked
                    .insert(package.name.clone(), package.version.clone());
            }
        }
        self
    }

    /// Cached index lookup for `name`.
    fn entries(&self, name: &str) -> Result<Option<Vec<IndexEntry>>, ProviderError> {
        if let Some(cached) = self.cache.borrow().get(name) {
            return Ok(cached.clone());
        }
        let loaded = self
            .registry
            .load_entries(name)
            .map_err(|e| ProviderError::Io {
                path: PathBuf::from(self.registry.location()).join(Registry::index_rel(name)),
                message: e.to_string(),
            })?;
        self.cache
            .borrow_mut()
            .insert(name.to_owned(), loaded.clone());
        Ok(loaded)
    }

    /// The index entry for one exact version of `name`.
    fn entry_for(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<IndexEntry, ProviderError> {
        let entries =
            self.entries(&package.name)?
                .ok_or_else(|| ProviderError::UnknownPackage {
                    package: package.to_string(),
                })?;
        let wanted = version.to_string();
        entries
            .into_iter()
            .find(|e| e.version == wanted)
            .ok_or_else(|| ProviderError::VersionNotFound {
                package: package.to_string(),
                version: wanted,
            })
    }

    /// Non-registry sources are not this provider's job.
    fn require_registry_source(package: &PackageId) -> Result<(), ProviderError> {
        if matches!(package.source, Source::Registry) {
            Ok(())
        } else {
            Err(ProviderError::UnsupportedSource {
                package: package.to_string(),
            })
        }
    }
}

impl PackageProvider for RegistryProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        Self::require_registry_source(package)?;
        let entries =
            self.entries(&package.name)?
                .ok_or_else(|| ProviderError::UnknownPackage {
                    package: package.to_string(),
                })?;
        let locked = self.locked.get(&package.name);
        let mut versions = Vec::with_capacity(entries.len());
        for entry in &entries {
            let version =
                Version::parse(&entry.version).map_err(|e| ProviderError::InvalidVersion {
                    package: package.to_string(),
                    version: entry.version.clone(),
                    message: e.to_string(),
                })?;
            // Yanked versions are hidden from new resolutions; a version an
            // existing lockfile pins stays restorable (crates.io yank rule).
            if !entry.yanked || locked == Some(&version) {
                versions.push(version);
            }
        }
        Ok(versions)
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        Self::require_registry_source(package)?;
        let entry = self.entry_for(package, version)?;
        Ok(entry
            .deps
            .into_iter()
            // A published package's dev-deps never participate in a
            // consumer's resolution (cargo semantics).
            .filter(|dep| !dep.dev)
            .map(|dep| (dep.name, Dependency::Version(dep.req)))
            .collect())
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        Self::require_registry_source(package)?;
        let entry = self.entry_for(package, version)?;
        Ok(PackageMeta {
            lua_versions: entry.lua_versions,
            checksum: (!entry.checksum.is_empty()).then_some(entry.checksum),
            pinned: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_rel_path_follows_crates_io_rules() {
        assert_eq!(index_rel_path("a"), "1/a");
        assert_eq!(index_rel_path("ab"), "2/ab");
        assert_eq!(index_rel_path("abc"), "3/a/abc");
        assert_eq!(index_rel_path("penlight"), "pe/nl/penlight");
        // Scoped names keep the org directory.
        assert_eq!(index_rel_path("@org/pkg"), "@org/pkg");
    }

    #[test]
    fn open_parses_all_transport_forms() {
        assert!(
            Registry::open("https://pkgs.example.com/reg")
                .unwrap()
                .location()
                == "https://pkgs.example.com/reg"
        );
        assert!(!Registry::open("https://x.example").unwrap().is_writable());
        assert!(Registry::open("/some/dir").unwrap().is_writable());
        let file_url = if cfg!(windows) {
            "file:///C:/reg"
        } else {
            "file:///var/reg"
        };
        assert!(Registry::open(file_url).unwrap().is_writable());
        assert!(Registry::open("  ").is_err());
    }

    #[test]
    fn index_entry_round_trips() {
        let entry = IndexEntry {
            name: "penlight".to_owned(),
            version: "1.14.0".to_owned(),
            deps: vec![IndexDep {
                name: "luafilesystem".to_owned(),
                req: "^1.8".to_owned(),
                dev: false,
            }],
            lua_versions: vec!["5.4".to_owned()],
            checksum: "sha256:aa".to_owned(),
            yanked: false,
        };
        let line = entry.to_json_line();
        assert!(!line.contains('\n'), "index lines must be single-line");
        assert_eq!(IndexEntry::parse(&line).unwrap(), entry);
        // Optional fields default (forward/backward tolerance).
        let minimal = r#"{"name":"a","version":"1.0.0","checksum":"sha256:bb"}"#;
        let parsed = IndexEntry::parse(minimal).unwrap();
        assert!(parsed.deps.is_empty() && parsed.lua_versions.is_empty() && !parsed.yanked);
    }
}
