//! HTTP(S)/local tarball dependency fetcher (SPEC.md §6: bun-style
//! `pkg = { url = "…", sha256 = "…" }` deps).
//!
//! [`UrlProvider`] implements [`PackageProvider`] for [`Source::Url`] packages
//! by shelling out to the same external tools the rest of the Distribution
//! context uses — `curl` to download and `tar` to unpack — so no HTTP or
//! archive crate is pulled in (mirroring [`crate::GitProvider`] and the
//! luarocks bridge). Each `(url, sha256)` pair is fetched once into a cache
//! directory (the CLI passes `<store-root>/url`) and reused across resolves.
//!
//! # Integrity first
//!
//! The pinned SHA-256 is verified against the downloaded bytes **before**
//! extraction. A mismatch is a hard [`ProviderError::IntegrityMismatch`]
//! naming the expected and actual digests, and the cache slot is removed, so a
//! corrupt or tampered archive leaves nothing behind. This is the bun model:
//! the digest is captured once (at `luabox add --url` time) and enforced on
//! every fetch forever after.
//!
//! # Cache layout
//!
//! ```text
//! <cache>/<tail-of-url>-<hash-of-url-and-sha>/
//!   tree/       # the extracted tarball — store-ready
//!   FETCHED     # the verified sha256, one line (offline-reuse guard)
//! ```
//!
//! A slot whose `FETCHED` marker matches the requested digest is reused as-is
//! (deterministic and offline-friendly: a second `luabox install` touches no
//! network).
//!
//! # Versioning
//!
//! A url tarball is opaque content. When the extracted tree carries a
//! `luabox.toml` (a luabox package published as a tarball), its version,
//! `lua-versions`, and dependencies are honored, exactly as
//! [`crate::PathProvider`]/[`crate::GitProvider`] read the manifest at their
//! source. Otherwise — the common bun-style raw tarball — the version defaults
//! to `0.0.0` with no transitive dependencies. Either way the dependency's
//! optional `version` field is a version *requirement* (see the solver), not
//! the reported version.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::manifest::{Dependency, Manifest};
use crate::provider::{
    PackageId, PackageMeta, PackageProvider, ProviderError, Source, parse_manifest_at,
};

/// Name of the file that marks a cache slot's verified digest.
const FETCHED_FILE: &str = "FETCHED";

/// The version reported for a url tarball that carries no `luabox.toml`.
const DEFAULT_VERSION: &str = "0.0.0";

/// One fetched-and-extracted tarball, plus its manifest when it shipped one.
#[derive(Debug, Clone)]
struct Entry {
    /// The extracted tree — safe to hand to `Store::put_tree`.
    tree: PathBuf,
    /// The package's `luabox.toml`, when the tarball included one.
    manifest: Option<Manifest>,
}

/// Provider for [`Source::Url`] packages, backed by `curl` + `tar`.
///
/// A url package has exactly one version: the manifest's when the tarball
/// ships a `luabox.toml`, else [`DEFAULT_VERSION`].
#[derive(Debug, Default)]
pub struct UrlProvider {
    cache_dir: PathBuf,
    entries: RefCell<BTreeMap<(String, String), Entry>>,
}

impl UrlProvider {
    /// A provider caching fetches under `cache_dir` (created on demand).
    #[must_use]
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            entries: RefCell::new(BTreeMap::new()),
        }
    }

    /// Fetch (or reuse from cache) the extracted tree for `url` pinned at
    /// `sha256`. This is the seam `luabox install` uses to hand the verified
    /// tree to the content-addressed store after resolution.
    ///
    /// # Errors
    /// Fails when the download fails (`curl` missing, bad url), the digest
    /// does not match ([`ProviderError::IntegrityMismatch`]), extraction fails
    /// (`tar` missing, corrupt archive), or on cache I/O errors.
    pub fn tree(&self, url: &str, sha256: &str) -> Result<PathBuf, ProviderError> {
        Ok(self.entry(url, sha256)?.tree)
    }

    /// Downloads (or reads, for a local/`file://` source) the bytes named by
    /// `url` into `staging` and returns their SHA-256 — the digest
    /// `luabox add --url` captures. Nothing is extracted or cached.
    ///
    /// # Errors
    /// Fails when the source cannot be fetched or hashed.
    pub fn digest_of(url: &str, staging: &Path) -> Result<String, ProviderError> {
        let archive = obtain(url, staging)?;
        luabox_store::hash_file(&archive)
            .map_err(|e| io(&archive, &format!("cannot hash url dependency archive: {e}")))
    }

    /// Loads (or fetches) the entry for `(url, sha256)`.
    fn entry(&self, url: &str, sha256: &str) -> Result<Entry, ProviderError> {
        let key = (url.to_owned(), sha256.to_owned());
        if let Some(entry) = self.entries.borrow().get(&key) {
            return Ok(entry.clone());
        }

        let tree = self.fetch(url, sha256)?;
        // A url tarball may or may not ship a manifest; read it when present.
        let manifest_path = tree.join("luabox.toml");
        let manifest = match fs::read_to_string(&manifest_path) {
            Ok(text) => Some(parse_manifest_at(&manifest_path, &text)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(io(
                    &manifest_path,
                    &format!("cannot read url dependency manifest: {e}"),
                ));
            }
        };

        let entry = Entry { tree, manifest };
        self.entries.borrow_mut().insert(key, entry.clone());
        Ok(entry)
    }

    /// Materializes the cache slot for `(url, sha256)`: reuse a verified
    /// extraction, or download → verify → extract fresh.
    fn fetch(&self, url: &str, sha256: &str) -> Result<PathBuf, ProviderError> {
        let slot = self.cache_dir.join(cache_key(url, sha256));
        let tree = slot.join("tree");
        let marker = slot.join(FETCHED_FILE);

        // Offline reuse: a verified extraction whose marker matches is served
        // as-is, so a second resolve never touches the network.
        if tree.is_dir()
            && let Ok(recorded) = fs::read_to_string(&marker)
            && recorded.trim().eq_ignore_ascii_case(sha256)
        {
            return Ok(tree);
        }

        // (Re)build the slot from scratch.
        remove_all_force(&slot)
            .map_err(|e| io(&slot, &format!("cannot clear url cache entry: {e}")))?;
        fs::create_dir_all(&tree)
            .map_err(|e| io(&slot, &format!("cannot create url cache entry: {e}")))?;

        // Obtain the archive: a local/`file://` source is used in place, an
        // http(s) source is downloaded into the slot (beside, not inside, the
        // tree, so it is never interned).
        let archive = match obtain(url, &slot) {
            Ok(archive) => archive,
            Err(e) => {
                remove_all_force(&slot).ok();
                return Err(e);
            }
        };

        // Verify BEFORE extraction: a digest mismatch installs nothing.
        let digest = luabox_store::hash_file(&archive)
            .map_err(|e| io(&archive, &format!("cannot hash url dependency archive: {e}")))?;
        if !digest.eq_ignore_ascii_case(sha256) {
            remove_all_force(&slot).ok();
            return Err(ProviderError::IntegrityMismatch {
                url: url.to_owned(),
                expected: sha256.to_owned(),
                actual: digest,
            });
        }

        if let Err(e) = extract(&archive, &tree) {
            remove_all_force(&slot).ok();
            return Err(e);
        }
        // Drop a downloaded archive; a local source is left untouched.
        if archive.starts_with(&slot) {
            let _ = fs::remove_file(&archive);
        }
        fs::write(&marker, format!("{sha256}\n"))
            .map_err(|e| io(&marker, &format!("cannot record url fetch marker: {e}")))?;
        Ok(tree)
    }

    /// Loads the entry for a url package and projects it through `f`.
    fn with_entry<R>(
        &self,
        package: &PackageId,
        f: impl FnOnce(&Entry) -> R,
    ) -> Result<R, ProviderError> {
        let Source::Url { url, sha256 } = &package.source else {
            return Err(ProviderError::UnsupportedSource {
                package: package.to_string(),
            });
        };
        let entry = self.entry(url, sha256)?;
        Ok(f(&entry))
    }

    /// The version a url package reports: its manifest's when it ships one,
    /// else [`DEFAULT_VERSION`].
    fn entry_version(package: &PackageId, entry: &Entry) -> Result<Version, ProviderError> {
        let raw = entry
            .manifest
            .as_ref()
            .map_or(DEFAULT_VERSION, |m| m.package.version.as_str());
        Version::parse(raw).map_err(|e| ProviderError::InvalidVersion {
            package: package.to_string(),
            version: raw.to_owned(),
            message: e.to_string(),
        })
    }
}

impl PackageProvider for UrlProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        self.with_entry(package, |entry| {
            Self::entry_version(package, entry).map(|v| vec![v])
        })?
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        self.with_entry(package, |entry| {
            let actual = Self::entry_version(package, entry)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(entry
                .manifest
                .as_ref()
                .map(|m| m.dependencies.clone())
                .unwrap_or_default())
        })?
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        let Source::Url { sha256, .. } = &package.source else {
            return Err(ProviderError::UnsupportedSource {
                package: package.to_string(),
            });
        };
        self.with_entry(package, |entry| {
            let actual = Self::entry_version(package, entry)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(PackageMeta {
                lua_versions: entry
                    .manifest
                    .as_ref()
                    .map(|m| m.package.lua_versions.clone())
                    .unwrap_or_default(),
                // The tarball's `luabox.toml` edition, when it ships one (some
                // tarballs are bare source with no manifest).
                edition: entry.manifest.as_ref().map(|m| m.package.edition.clone()),
                // The pinned digest is the lockfile checksum — content is
                // addressed by exactly the sha256 the manifest declares.
                checksum: Some(format!("sha256:{sha256}")),
                pinned: None,
            })
        })?
    }
}

/// Interpret `url` as a local path when it is one: a `file://` URL or an
/// existing filesystem path. Mirrors the toolchain installer so hermetic tests
/// (and offline vendoring) can point a url dependency at a local tarball.
fn local_path(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        // `file:///C:/x` (windows) and `file:///home/x` (unix): strip the
        // authority-empty leading slash before a Windows drive letter.
        let trimmed = if cfg!(windows) {
            rest.trim_start_matches('/')
        } else {
            rest
        };
        return Some(PathBuf::from(trimmed));
    }
    let path = Path::new(url);
    path.exists().then(|| path.to_path_buf())
}

/// Obtain the archive named by `url` into `staging`, returning the local file
/// path to hash/extract. Local/`file://` sources are used in place; http(s)
/// sources are downloaded with `curl -fsSL` (the shared external-tool policy,
/// SPEC.md §6).
fn obtain(url: &str, staging: &Path) -> Result<PathBuf, ProviderError> {
    if let Some(path) = local_path(url) {
        if !path.is_file() {
            return Err(io(&path, "url dependency source does not exist"));
        }
        return Ok(path);
    }
    fs::create_dir_all(staging)
        .map_err(|e| io(staging, &format!("cannot create download dir: {e}")))?;
    let archive = staging.join("download");
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", "120", "-o"])
        .arg(&archive)
        .arg(url)
        .output()
        .map_err(|e| external("curl", format!("cannot run `curl` (needed to reach {url}): {e}")))?;
    if !output.status.success() {
        return Err(external(
            "curl",
            format!(
                "`curl` failed to download {url}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(archive)
}

/// Unpack `archive` into `dest` with `tar -xf` (no archive crate; SPEC.md §6).
fn extract(archive: &Path, dest: &Path) -> Result<(), ProviderError> {
    let tar = tar_program();
    let output = Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()
        .map_err(|e| {
            external(
                &tar.to_string_lossy(),
                format!("cannot run `tar` (needed to unpack {}): {e}", archive.display()),
            )
        })?;
    if !output.status.success() {
        return Err(external(
            &tar.to_string_lossy(),
            format!(
                "`tar -xf` failed to unpack `{}`: {}",
                archive.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(())
}

/// `tar` program to shell out to (Windows: prefer the system `bsdtar` so a
/// git-shipped GNU tar on PATH can't shadow it — it can't read `.zip`).
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

/// Stable, filesystem-safe cache slot name for a `(url, sha256)` pair: a
/// readable tail from the url plus an FNV-1a hash covering both.
fn cache_key(url: &str, sha256: &str) -> String {
    let tail = url
        .trim_end_matches('/')
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("archive");
    let tail: String = tail
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    let tail = if tail.is_empty() {
        "archive".to_owned()
    } else {
        tail
    };
    let hash = fnv1a64(format!("{url}\0{sha256}").as_bytes());
    format!("{tail}-{hash:016x}")
}

/// FNV-1a, 64-bit — tiny, dependency-free, and plenty for cache slot names.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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

/// `remove_dir_all` that also deletes read-only files (store hard-links share
/// the object's read-only bit, which `std` removal refuses on Windows).
/// Missing paths are fine.
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
                reason = "clearing the read-only bit is exactly the intent: extracted archive \
                          files may be read-only and must be writable to delete on Windows"
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
    use std::process::Command;

    /// Build a `.tar.gz` fixture at `archive` from `files` (name → contents),
    /// returning its SHA-256. Entries are archived flat (no wrapper dir), so
    /// the extracted tree root is the package root.
    fn make_fixture(archive: &Path, files: &[(&str, &str)]) -> String {
        let src = tempfile::tempdir().unwrap();
        let mut names = Vec::new();
        for (name, contents) in files {
            let path = src.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, contents).unwrap();
            names.push((*name).to_owned());
        }
        let mut cmd = Command::new("tar");
        cmd.arg("-czf").arg(archive).arg("-C").arg(src.path());
        for name in &names {
            cmd.arg(name);
        }
        let status = cmd
            .status()
            .expect("tar must be available to build the fixture");
        assert!(status.success(), "tar failed to build the fixture archive");
        luabox_store::hash_file(archive).unwrap()
    }

    #[test]
    fn fetches_verifies_and_extracts_a_tarball() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("mylib.tar.gz");
        let sha = make_fixture(&archive, &[("init.lua", "return 42\n")]);

        let provider = UrlProvider::new(root.path().join("cache"));
        let url = archive.to_string_lossy().replace('\\', "/");
        let id = PackageId::url("mylib", &url, &sha);

        // No manifest in the tarball → default version, no deps.
        let versions = provider.list_versions(&id).unwrap();
        assert_eq!(versions, vec![Version::parse(DEFAULT_VERSION).unwrap()]);
        assert!(provider.dependencies(&id, &versions[0]).unwrap().is_empty());
        let meta = provider.metadata(&id, &versions[0]).unwrap();
        assert_eq!(meta.checksum.as_deref(), Some(&*format!("sha256:{sha}")));

        // The extracted tree carries the archived file.
        let tree = provider.tree(&url, &sha).unwrap();
        assert_eq!(
            fs::read_to_string(tree.join("init.lua")).unwrap(),
            "return 42\n"
        );
    }

    #[test]
    fn honors_a_manifest_shipped_in_the_tarball() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("withmanifest.tar.gz");
        let manifest = "[package]\nname = \"withmanifest\"\nversion = \"2.3.4\"\nedition = \"5.4\"\nlua-versions = [\"5.4\"]\n";
        let sha = make_fixture(
            &archive,
            &[("luabox.toml", manifest), ("init.lua", "return 1\n")],
        );

        let provider = UrlProvider::new(root.path().join("cache"));
        let url = archive.to_string_lossy().replace('\\', "/");
        let id = PackageId::url("withmanifest", &url, &sha);

        let versions = provider.list_versions(&id).unwrap();
        assert_eq!(versions, vec![Version::parse("2.3.4").unwrap()]);
        let meta = provider.metadata(&id, &versions[0]).unwrap();
        assert_eq!(meta.lua_versions, vec!["5.4".to_owned()]);
    }

    #[test]
    fn a_mismatched_digest_is_rejected_and_nothing_is_extracted() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("mylib.tar.gz");
        make_fixture(&archive, &[("init.lua", "return 1\n")]);
        let bad = "0".repeat(64);

        let cache = root.path().join("cache");
        let provider = UrlProvider::new(&cache);
        let url = archive.to_string_lossy().replace('\\', "/");
        let id = PackageId::url("mylib", &url, &bad);

        let err = provider.list_versions(&id).unwrap_err();
        assert!(
            matches!(err, ProviderError::IntegrityMismatch { .. }),
            "expected an integrity mismatch, got {err:?}"
        );
        let message = err.to_string();
        assert!(message.contains(&bad), "names the expected digest: {message}");
        // Nothing left behind after a rejected fetch.
        for entry in fs::read_dir(&cache).into_iter().flatten().flatten() {
            assert!(
                !entry.path().join("tree").join("init.lua").exists(),
                "a rejected fetch must extract nothing"
            );
        }
    }

    #[test]
    fn a_second_fetch_reuses_the_cache_offline() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("mylib.tar.gz");
        let sha = make_fixture(&archive, &[("init.lua", "return 1\n")]);

        let provider = UrlProvider::new(root.path().join("cache"));
        let url = archive.to_string_lossy().replace('\\', "/");
        let first = provider.tree(&url, &sha).unwrap();

        // Delete the source archive: a cache hit must not need it again.
        fs::remove_file(&archive).unwrap();
        let fresh = UrlProvider::new(root.path().join("cache"));
        let second = fresh.tree(&url, &sha).unwrap();
        assert_eq!(first, second);
        assert!(second.join("init.lua").is_file());
    }

    #[test]
    fn cache_keys_distinguish_url_and_digest() {
        let a = cache_key("https://example.com/x.tar.gz", "aa");
        assert_ne!(a, cache_key("https://example.com/y.tar.gz", "aa"), "url");
        assert_ne!(a, cache_key("https://example.com/x.tar.gz", "bb"), "digest");
        assert_eq!(a, cache_key("https://example.com/x.tar.gz", "aa"), "stable");
        assert!(a.starts_with("x-tar-gz-"), "readable tail: {a}");
    }

    #[test]
    fn digest_of_hashes_without_extracting() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("mylib.tar.gz");
        let sha = make_fixture(&archive, &[("init.lua", "return 1\n")]);
        let url = archive.to_string_lossy().replace('\\', "/");
        let staging = root.path().join("staging");
        assert_eq!(UrlProvider::digest_of(&url, &staging).unwrap(), sha);
    }
}
