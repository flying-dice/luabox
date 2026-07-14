//! Git dependency fetcher (SPEC.md §6: git deps with rev/tag/branch).
//!
//! [`GitProvider`] implements [`PackageProvider`] for [`Source::Git`]
//! packages by shelling out to the `git` CLI — no in-process git
//! implementation, no new dependencies. Each `(url, reference)` pair is
//! fetched once into a cache directory (the CLI passes
//! `<store-root>/git`) and reused across resolves.
//!
//! # Cache layout
//!
//! ```text
//! <cache>/<name>-<hash-of-url-and-ref>/
//!   checkout/    # the exported tree — WITHOUT `.git` (see below)
//!   COMMIT       # the resolved commit sha, one line
//! ```
//!
//! The `.git` directory is deleted after clone and the resolved sha is
//! written to `COMMIT` instead. Two reasons:
//!
//! - the checkout is what `luabox install` hands to the content-addressed
//!   store (`Store::put_tree` interns every file, so `.git` must not be
//!   part of the package tree);
//! - git object/pack files are read-only, which trips naive recursive
//!   deletion on Windows — exporting once and force-deleting `.git` here
//!   keeps that platform quirk in one place.
//!
//! # Pinning
//!
//! [`PackageProvider::metadata`] reports the resolved commit as
//! [`PackageMeta::pinned`], which the solver records in `luabox.lock` as
//! `git+<url>#<sha>` — even a branch reference locks to the exact commit
//! that was fetched. A cached checkout is reused as-is (deterministic,
//! offline-friendly); `luabox update` opts into re-fetching mutable refs
//! via [`GitProvider::with_refresh`]. Reusing a *previously locked* sha
//! after the cache is wiped requires the lock to carry the symbolic ref
//! alongside the sha — that refinement lands with the registry work (#20).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;

use crate::manifest::{Dependency, Manifest};
use crate::provider::{
    GitReference, PackageId, PackageMeta, PackageProvider, ProviderError, Source, parse_manifest_at,
};

/// Name of the file that records a cache entry's resolved commit.
const COMMIT_FILE: &str = "COMMIT";

/// One fetched-and-exported git tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCheckout {
    /// The exported tree (no `.git` inside) — safe to hand to
    /// `Store::put_tree`.
    pub dir: PathBuf,
    /// The resolved commit sha (`git rev-parse HEAD` at fetch time).
    pub commit: String,
}

/// A fetched checkout plus its parsed manifest.
#[derive(Debug, Clone)]
struct Entry {
    checkout: GitCheckout,
    manifest: Manifest,
}

/// Provider for [`Source::Git`] packages, backed by the `git` CLI.
///
/// A git package has exactly one version — whatever the manifest at the
/// fetched revision declares (mirroring [`crate::PathProvider`]).
#[derive(Debug, Default)]
pub struct GitProvider {
    cache_dir: PathBuf,
    /// Re-fetch mutable references (tag/branch/default) even when a cached
    /// checkout exists. `rev` pins are immutable and never re-fetched.
    refresh: bool,
    entries: RefCell<BTreeMap<(String, GitReference), Entry>>,
}

impl GitProvider {
    /// A provider caching fetches under `cache_dir` (created on demand).
    #[must_use]
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            refresh: false,
            entries: RefCell::new(BTreeMap::new()),
        }
    }

    /// Re-fetch mutable references (tag/branch/default branch) instead of
    /// reusing cached checkouts — `luabox update` semantics. Immutable
    /// `rev` pins are still served from cache.
    #[must_use]
    pub fn with_refresh(mut self, refresh: bool) -> Self {
        self.refresh = refresh;
        self
    }

    /// Fetch (or reuse from cache) the checkout for `url` at `reference`.
    ///
    /// This is the seam `luabox install` uses to hand the fetched tree to
    /// the content-addressed store after resolution.
    ///
    /// # Errors
    /// Fails when `git` is missing from `PATH`, the clone/checkout fails
    /// (bad url, unknown ref), or on cache I/O errors.
    pub fn checkout(
        &self,
        url: &str,
        reference: &GitReference,
    ) -> Result<GitCheckout, ProviderError> {
        Ok(self.entry(url, reference)?.checkout)
    }

    /// Loads (or fetches) the entry for `(url, reference)`.
    fn entry(&self, url: &str, reference: &GitReference) -> Result<Entry, ProviderError> {
        let key = (url.to_owned(), reference.clone());
        if let Some(entry) = self.entries.borrow().get(&key) {
            return Ok(entry.clone());
        }

        let checkout = self.fetch(url, reference)?;
        let manifest_path = checkout.dir.join("luabox.toml");
        let text = fs::read_to_string(&manifest_path).map_err(|e| ProviderError::Io {
            path: manifest_path.clone(),
            message: format!("git dependency has no readable luabox.toml: {e}"),
        })?;
        let manifest = parse_manifest_at(&manifest_path, &text)?;

        let entry = Entry { checkout, manifest };
        self.entries.borrow_mut().insert(key, entry.clone());
        Ok(entry)
    }

    /// Materializes the cache slot for `(url, reference)`: reuse a valid
    /// existing checkout, or clone fresh.
    fn fetch(&self, url: &str, reference: &GitReference) -> Result<GitCheckout, ProviderError> {
        let slot = self.cache_dir.join(cache_key(url, reference));
        let dir = slot.join("checkout");
        let commit_file = slot.join(COMMIT_FILE);

        let immutable = matches!(reference, GitReference::Rev(_));
        let reusable = immutable || !self.refresh;
        if reusable
            && dir.is_dir()
            && let Ok(commit) = fs::read_to_string(&commit_file)
        {
            let commit = commit.trim().to_owned();
            if !commit.is_empty() {
                return Ok(GitCheckout { dir, commit });
            }
        }

        // (Re)build the slot from scratch.
        remove_all_force(&slot).map_err(|e| ProviderError::Io {
            path: slot.clone(),
            message: format!("cannot clear git cache entry: {e}"),
        })?;
        fs::create_dir_all(&slot).map_err(|e| ProviderError::Io {
            path: slot.clone(),
            message: format!("cannot create git cache entry: {e}"),
        })?;

        match reference {
            GitReference::DefaultBranch => {
                clone(&["--depth", "1"], url, &dir)?;
            }
            GitReference::Tag(name) | GitReference::Branch(name) => {
                clone(&["--depth", "1", "--branch", name], url, &dir)?;
            }
            GitReference::Rev(rev) => {
                // Arbitrary shas cannot be shallow-fetched portably; clone
                // fully, then detach at the requested revision.
                clone(&[], url, &dir)?;
                git(&["checkout", "--quiet", "--detach", rev], &dir, Some(&dir))?;
            }
        }

        let commit = git(&["rev-parse", "HEAD"], &dir, Some(&dir))?;
        let commit = commit.trim().to_owned();

        // Export: drop `.git` (read-only object files force-removed) and
        // record the pin, so the tree is store-ready and the cache hit
        // path never touches git again.
        remove_all_force(&dir.join(".git")).map_err(|e| ProviderError::Io {
            path: dir.join(".git"),
            message: format!("cannot remove .git from cached checkout: {e}"),
        })?;
        fs::write(&commit_file, format!("{commit}\n")).map_err(|e| ProviderError::Io {
            path: commit_file,
            message: format!("cannot record resolved commit: {e}"),
        })?;

        Ok(GitCheckout { dir, commit })
    }

    /// Loads the entry for a git package and projects it through `f`,
    /// verifying the fetched manifest declares the expected package name.
    fn with_manifest<R>(
        &self,
        package: &PackageId,
        f: impl FnOnce(&Entry) -> R,
    ) -> Result<R, ProviderError> {
        let Source::Git { url, reference } = &package.source else {
            return Err(ProviderError::UnsupportedSource {
                package: package.to_string(),
            });
        };
        let entry = self.entry(url, reference)?;
        if entry.manifest.package.name != package.name {
            return Err(ProviderError::NameMismatch {
                expected: package.name.clone(),
                found: entry.manifest.package.name.clone(),
                path: entry.checkout.dir.clone(),
            });
        }
        Ok(f(&entry))
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

impl PackageProvider for GitProvider {
    fn list_versions(&self, package: &PackageId) -> Result<Vec<Version>, ProviderError> {
        self.with_manifest(package, |entry| {
            Self::manifest_version(package, &entry.manifest).map(|v| vec![v])
        })?
    }

    fn dependencies(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<BTreeMap<String, Dependency>, ProviderError> {
        self.with_manifest(package, |entry| {
            let actual = Self::manifest_version(package, &entry.manifest)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(entry.manifest.dependencies.clone())
        })?
    }

    fn metadata(
        &self,
        package: &PackageId,
        version: &Version,
    ) -> Result<PackageMeta, ProviderError> {
        self.with_manifest(package, |entry| {
            let actual = Self::manifest_version(package, &entry.manifest)?;
            if &actual != version {
                return Err(ProviderError::VersionNotFound {
                    package: package.to_string(),
                    version: version.to_string(),
                });
            }
            Ok(PackageMeta {
                lua_versions: entry.manifest.package.lua_versions.clone(),
                checksum: None,
                pinned: Some(entry.checkout.commit.clone()),
            })
        })?
    }
}

/// `git clone --quiet [extra…] -- <url> <dest>`.
fn clone(extra: &[&str], url: &str, dest: &Path) -> Result<(), ProviderError> {
    let mut args = vec!["clone", "--quiet"];
    args.extend_from_slice(extra);
    args.extend_from_slice(&["--", url]);
    let dest_text = dest.to_string_lossy();
    args.push(&dest_text);
    git(&args, dest, None).map(|_| ())
}

/// Runs one git command; on success returns stdout, on failure a
/// [`ProviderError::Io`] carrying the command line and git's stderr.
fn git(args: &[&str], context_path: &Path, cwd: Option<&Path>) -> Result<String, ProviderError> {
    let mut command = Command::new("git");
    command.args(args);
    // Never let a bad URL hang an install waiting for credentials.
    command.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().map_err(|e| ProviderError::Io {
        path: context_path.to_path_buf(),
        message: format!("cannot run `git {}`: {e}", args.join(" ")),
    })?;
    if !output.status.success() {
        return Err(ProviderError::Io {
            path: context_path.to_path_buf(),
            message: format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Stable, filesystem-safe cache slot name for a `(url, reference)` pair:
/// a readable tail from the url plus an FNV-1a hash covering both.
fn cache_key(url: &str, reference: &GitReference) -> String {
    let tail = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or("repo");
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
        "repo".to_owned()
    } else {
        tail
    };
    let discriminator = match reference {
        GitReference::Rev(r) => format!("rev\0{r}"),
        GitReference::Tag(t) => format!("tag\0{t}"),
        GitReference::Branch(b) => format!("branch\0{b}"),
        GitReference::DefaultBranch => "head".to_owned(),
    };
    let hash = fnv1a64(format!("{url}\0{discriminator}").as_bytes());
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

/// `remove_dir_all` that also deletes read-only files (git object/pack
/// files are read-only, which `std`'s removal refuses on Windows).
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

    #[test]
    fn cache_keys_distinguish_url_and_reference() {
        let url_a = "https://example.com/org/repo.git";
        let url_b = "https://example.com/other/repo.git";
        let tag = GitReference::Tag("v1".to_owned());
        let branch = GitReference::Branch("v1".to_owned());
        let a_tag = cache_key(url_a, &tag);
        assert_ne!(a_tag, cache_key(url_b, &tag), "different urls");
        assert_ne!(a_tag, cache_key(url_a, &branch), "tag vs branch, same name");
        assert_eq!(a_tag, cache_key(url_a, &tag), "stable");
        assert!(a_tag.starts_with("repo-"), "readable tail: {a_tag}");
    }

    #[test]
    fn cache_key_sanitizes_hostile_tails() {
        let key = cache_key(
            "https://example.com/We!rd Näme",
            &GitReference::DefaultBranch,
        );
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{key}"
        );
    }

    #[test]
    fn remove_all_force_clears_readonly_trees() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("tree").join("nested");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("pack.idx");
        fs::write(&file, b"x").unwrap();
        let mut perms = fs::metadata(&file).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&file, perms).unwrap();

        remove_all_force(&tmp.path().join("tree")).expect("force removal succeeds");
        assert!(!tmp.path().join("tree").exists());
        // Idempotent on missing paths.
        remove_all_force(&tmp.path().join("tree")).expect("missing path is fine");
    }
}
