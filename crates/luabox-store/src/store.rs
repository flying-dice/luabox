//! [`Store`] — the public entry point to the content-addressed store.
//!
//! `Store::open(root)` binds to a store directory (the CLI passes
//! `~/.luabox/store`; this crate takes the root as a parameter and adds no
//! directory-discovery dependency). Everything else hangs off it: interning a
//! package tree ([`Store::put_tree`]), reconstituting one into a project
//! ([`Store::materialize`]), integrity checking ([`Store::verify`]), and
//! reclaiming space ([`Store::gc`]).

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::error::{IoResultExt, StoreError};
use crate::hash::hash_file;
use crate::lock::GcLock;
use crate::manifest::{FileEntry, TreeManifest};
use crate::object::{self, ObjectStore};

/// Default grace window for [`Store::gc`]: objects touched more recently than
/// this are never collected, so a `gc` cannot race a concurrent install that
/// has written objects but not yet recorded them in a live manifest.
const GC_DEFAULT_GRACE: Duration = Duration::from_secs(60 * 60);

/// After this long, a `gc.lock` is presumed orphaned by a crashed collector and
/// may be stolen. Set far above any realistic collection time.
const GC_LOCK_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

/// How a materialized project file is linked back to its store object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkMode {
    /// Always copy. The copy is writable — this is the escape hatch for code
    /// that needs to mutate installed files.
    Copy,
    /// Best-effort default (the bun/pnpm model, SPEC.md §6): hard link, and
    /// fall back to a copy on *any* link failure — cross-device, an
    /// unsupported filesystem, or any other I/O error. A hard-linked file
    /// shares the object's read-only bit; a fallback copy is writable.
    /// Reserved to grow reflink support without an API change.
    #[default]
    Auto,
}

/// Tally of how [`Store::materialize`] placed each file. Exposed so callers —
/// and tests — can prove that hard linking actually happened rather than
/// silently degrading to copies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterializeReport {
    /// Files placed as hard links to their store object.
    pub hard_linked: usize,
    /// Files placed as independent copies (either `LinkMode::Copy`, or a link
    /// fallback).
    pub copied: usize,
}

impl MaterializeReport {
    /// Total files materialized.
    #[must_use]
    pub fn total(&self) -> usize {
        self.hard_linked + self.copied
    }
}

/// Aggregate store statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Number of distinct objects held.
    pub objects: u64,
    /// Total bytes across all objects.
    pub bytes: u64,
}

/// Why a manifest entry failed [`Store::verify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptKind {
    /// The object is absent from the store entirely.
    Missing,
    /// The object is present but its contents no longer hash to its address.
    HashMismatch {
        /// The hash the on-disk bytes actually produce now.
        found: String,
    },
}

/// A store object referenced by a manifest that failed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptEntry {
    /// The object address (its expected hash).
    pub hash: String,
    /// One tree path that references this object, for diagnostics.
    pub sample_path: String,
    /// What is wrong.
    pub kind: CorruptKind,
}

/// Tuning for [`Store::gc_with_options`].
#[derive(Debug, Clone, Copy)]
pub struct GcOptions {
    /// Objects modified within this window are never collected (see
    /// [`GC_DEFAULT_GRACE`]). Set to [`Duration::ZERO`] to disable the guard
    /// (tests, or a store known to be quiescent).
    pub grace: Duration,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            grace: GC_DEFAULT_GRACE,
        }
    }
}

/// Outcome of a garbage collection pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Objects deleted.
    pub removed: u64,
    /// Bytes reclaimed.
    pub bytes_freed: u64,
    /// Live objects retained.
    pub kept: u64,
    /// Unreferenced objects spared because they were newer than the grace
    /// window.
    pub skipped_recent: u64,
}

/// A handle to a content-addressed store rooted at a directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    objects: ObjectStore,
    packages_dir: PathBuf,
}

impl Store {
    /// Open (or lazily create) a store under `root`.
    ///
    /// No directories are touched until the first write, so opening a store is
    /// cheap and infallible barring a bad path.
    pub fn open(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let objects_dir = root.join("objects").join("sha256");
        let tmp_dir = root.join("tmp");
        let packages_dir = root.join("packages");
        Self {
            objects: ObjectStore::new(objects_dir, tmp_dir),
            packages_dir,
            root,
        }
    }

    /// The store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether an object with `hash` is present.
    #[must_use]
    pub fn has(&self, hash: &str) -> bool {
        self.objects.has(hash)
    }

    // --- interning -------------------------------------------------------

    /// Walk `dir`, hash and intern every file, and return a manifest.
    ///
    /// Each file is stored write-once under `objects/`; a file whose content is
    /// already present is deduplicated, not rewritten. The returned manifest is
    /// in canonical (sorted) order and is itself content-addressed.
    ///
    /// Symlinks are skipped and empty directories are not recorded — the store
    /// addresses file *content*, and both reconstruct implicitly from the file
    /// set on `materialize`.
    ///
    /// # Errors
    /// Fails on I/O errors, or if any path is not valid UTF-8.
    pub fn put_tree(&self, dir: impl AsRef<Path>) -> Result<TreeManifest, StoreError> {
        let dir = dir.as_ref();
        let mut files = Vec::new();
        collect_files(dir, dir, &mut files)?;

        let mut entries = Vec::with_capacity(files.len());
        for (abs, rel) in files {
            let meta =
                fs::symlink_metadata(&abs).io_context(|| format!("stat {}", abs.display()))?;
            let executable = is_executable(&meta);
            let hash = hash_file(&abs).io_context(|| format!("hashing {}", abs.display()))?;
            self.objects.put_file(&abs, &hash, executable)?;
            entries.push(FileEntry {
                path: rel,
                hash,
                executable,
                size: meta.len(),
            });
        }
        Ok(TreeManifest::from_entries(entries))
    }

    // --- materialization -------------------------------------------------

    /// Reconstruct `manifest` into `dest_dir` using `mode`.
    ///
    /// Parent directories are created as needed and any pre-existing file at a
    /// target path is replaced. Fails cleanly — before touching `dest_dir` for
    /// that entry — if a referenced object is missing (run [`Store::verify`]
    /// first to distinguish missing from corrupt).
    ///
    /// # Errors
    /// Fails if an object is missing or on I/O errors. Hard-link failures are
    /// never surfaced: both [`LinkMode`] variants fall back to a writable copy
    /// (see the enum docs), so only a failing *copy* aborts materialization.
    pub fn materialize(
        &self,
        manifest: &TreeManifest,
        dest_dir: impl AsRef<Path>,
        mode: LinkMode,
    ) -> Result<MaterializeReport, StoreError> {
        let dest = dest_dir.as_ref();
        let mut report = MaterializeReport::default();
        for entry in &manifest.entries {
            let object = self.objects.object_path(&entry.hash);
            if !object.exists() {
                return Err(StoreError::MissingObject {
                    hash: entry.hash.clone(),
                    path: entry.path.clone(),
                });
            }
            let target = join_relative(dest, &entry.path)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .io_context(|| format!("creating {}", parent.display()))?;
            }
            replace_existing(&target)?;
            place(&object, &target, entry.executable, mode, &mut report)?;
        }
        Ok(report)
    }

    // --- integrity -------------------------------------------------------

    /// Re-hash every object referenced by `manifest` and report any that are
    /// missing or whose bytes no longer match their address.
    ///
    /// Distinct objects are checked once even if referenced by several paths.
    /// An empty result means the manifest is fully backed by intact objects.
    ///
    /// # Errors
    /// Fails only on unexpected I/O while reading an object (not on the
    /// corruption it is looking for — that is reported in the returned vec).
    pub fn verify(&self, manifest: &TreeManifest) -> Result<Vec<CorruptEntry>, StoreError> {
        let mut corrupt = Vec::new();
        for hash in manifest.object_hashes() {
            let object = self.objects.object_path(hash);
            let sample_path = manifest
                .entries
                .iter()
                .find(|e| e.hash == hash)
                .map_or_else(String::new, |e| e.path.clone());
            if !object.exists() {
                corrupt.push(CorruptEntry {
                    hash: hash.to_string(),
                    sample_path,
                    kind: CorruptKind::Missing,
                });
                continue;
            }
            let found = hash_file(&object)
                .io_context(|| format!("re-hashing object {}", object.display()))?;
            if found != hash {
                corrupt.push(CorruptEntry {
                    hash: hash.to_string(),
                    sample_path,
                    kind: CorruptKind::HashMismatch { found },
                });
            }
        }
        Ok(corrupt)
    }

    // --- statistics ------------------------------------------------------

    /// Count objects and total bytes held by the store.
    ///
    /// # Errors
    /// Fails on I/O while walking the object tree.
    pub fn stats(&self) -> Result<StoreStats, StoreError> {
        let mut stats = StoreStats::default();
        self.for_each_object(|_hash, _path, size| {
            stats.objects += 1;
            stats.bytes += size;
        })?;
        Ok(stats)
    }

    // --- garbage collection ---------------------------------------------

    /// Collect unreferenced objects, using the default grace window.
    ///
    /// `live` is every manifest whose objects must be preserved. See
    /// [`Store::gc_with_options`] for the safety model.
    ///
    /// # Errors
    /// Fails if another `gc` holds the lock, or on I/O.
    pub fn gc(&self, live: &[TreeManifest]) -> Result<GcReport, StoreError> {
        self.gc_with_options(live, GcOptions::default())
    }

    /// Collect unreferenced objects with explicit options.
    ///
    /// Safety model:
    /// - A coarse advisory lock (`gc.lock`) serializes cooperating collectors.
    /// - Objects newer than `options.grace` are spared, so a collection cannot
    ///   delete an object an in-flight install just wrote but has not yet
    ///   published in a live manifest.
    ///
    /// Limitations: this is best-effort against *uncooperative* concurrent
    /// writers. A process that writes an object and holds a reference to it for
    /// longer than the grace window without recording it in a `live` manifest
    /// could still see it collected. In practice installs publish their
    /// manifest immediately; the window exists only to cover the write→publish
    /// gap.
    ///
    /// # Errors
    /// Fails if another `gc` holds the lock, or on I/O.
    pub fn gc_with_options(
        &self,
        live: &[TreeManifest],
        options: GcOptions,
    ) -> Result<GcReport, StoreError> {
        let _lock = GcLock::acquire(self.root.join("gc.lock"), GC_LOCK_STALE_AFTER)?;

        let mut live_set = std::collections::HashSet::new();
        for manifest in live {
            for hash in manifest.object_hashes() {
                live_set.insert(hash.to_string());
            }
        }

        let mut report = GcReport::default();
        let mut victims = Vec::new();
        self.for_each_object(|hash, path, size| {
            if live_set.contains(hash) {
                report.kept += 1;
            } else if is_recent(path, options.grace) {
                report.skipped_recent += 1;
            } else {
                victims.push((path.to_path_buf(), size));
            }
        })?;

        for (path, size) in victims {
            match object::remove_file(&path) {
                Ok(()) => {
                    report.removed += 1;
                    report.bytes_freed += size;
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    // Raced with another remover; nothing to reclaim.
                }
                Err(err) => {
                    return Err(StoreError::Io {
                        context: format!("collecting {}", path.display()),
                        message: err.to_string(),
                    });
                }
            }
        }
        self.prune_empty_prefixes();
        Ok(report)
    }

    // --- package manifest index -----------------------------------------

    /// Persist a manifest under `packages/<name>/<version>/<tree_hash>.json`
    /// and return the path written. This is the store's package index; it is
    /// what `install` (#21) reads to materialize without re-walking a tree.
    ///
    /// # Errors
    /// Fails on I/O.
    pub fn write_package_manifest(
        &self,
        name: &str,
        version: &str,
        manifest: &TreeManifest,
    ) -> Result<PathBuf, StoreError> {
        let dir = self.packages_dir.join(name).join(version);
        fs::create_dir_all(&dir).io_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{}.json", manifest.tree_hash));
        let tmp = dir.join(format!("{}.json.tmp", manifest.tree_hash));
        fs::write(&tmp, manifest.to_json()).io_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).io_context(|| format!("committing {}", path.display()))?;
        Ok(path)
    }

    /// Load a manifest previously written by [`Store::write_package_manifest`].
    ///
    /// # Errors
    /// Fails on I/O or if the file is not a valid, self-consistent manifest.
    pub fn read_package_manifest(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<TreeManifest, StoreError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).io_context(|| format!("reading {}", path.display()))?;
        TreeManifest::from_json(&text).map_err(|source| StoreError::ManifestFile {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }

    // --- internals -------------------------------------------------------

    /// Invoke `f(hash, object_path, size)` for every object in the store.
    fn for_each_object(&self, mut f: impl FnMut(&str, &Path, u64)) -> Result<(), StoreError> {
        let objects_dir = self.objects.objects_dir();
        let prefixes = match fs::read_dir(objects_dir) {
            Ok(rd) => rd,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(StoreError::Io {
                    context: format!("reading {}", objects_dir.display()),
                    message: err.to_string(),
                });
            }
        };
        for prefix in prefixes {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            let prefix_name = prefix.file_name();
            let prefix_str = prefix_name.to_string_lossy();
            for object in fs::read_dir(prefix.path())? {
                let object = object?;
                let meta = object.metadata()?;
                if !meta.is_file() {
                    continue;
                }
                let rest = object.file_name();
                let hash = format!("{prefix_str}{}", rest.to_string_lossy());
                f(&hash, &object.path(), meta.len());
            }
        }
        Ok(())
    }

    /// Remove now-empty prefix directories left behind by `gc`.
    fn prune_empty_prefixes(&self) {
        let Ok(prefixes) = fs::read_dir(self.objects.objects_dir()) else {
            return;
        };
        for prefix in prefixes.flatten() {
            let path = prefix.path();
            if path.is_dir() {
                // Only removes if empty; ignore the error when it is not.
                let _ = fs::remove_dir(&path);
            }
        }
    }
}

// --- free helpers --------------------------------------------------------

/// Recursively collect regular files under `dir`, as `(absolute, rel-slash)`.
fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(PathBuf, String)>,
) -> Result<(), StoreError> {
    for entry in fs::read_dir(dir).io_context(|| format!("walking {}", dir.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| StoreError::InvalidPath {
                    message: format!("path {} escaped tree root", path.display()),
                })?;
            out.push((path.clone(), rel_to_slash(rel)?));
        }
        // Symlinks (and other exotic file types) are intentionally skipped.
    }
    Ok(())
}

/// Render a relative path as a `/`-separated string, erroring on non-UTF-8.
fn rel_to_slash(rel: &Path) -> Result<String, StoreError> {
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or_else(|| StoreError::InvalidPath {
                    message: format!("non-UTF-8 path component in {}", rel.display()),
                })?;
                parts.push(text);
            }
            Component::CurDir => {}
            _ => {
                return Err(StoreError::InvalidPath {
                    message: format!("unexpected path component in {}", rel.display()),
                });
            }
        }
    }
    Ok(parts.join("/"))
}

/// Turn a manifest's `/`-separated relative path into a real path under `dest`.
fn join_relative(dest: &Path, rel: &str) -> Result<PathBuf, StoreError> {
    let mut path = dest.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(StoreError::InvalidPath {
                message: format!("refusing to materialize suspicious path {rel:?}"),
            });
        }
        path.push(part);
    }
    Ok(path)
}

/// Delete an existing target so a fresh link/copy can take its place.
fn replace_existing(target: &Path) -> Result<(), StoreError> {
    match object::remove_file(target) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).io_context(|| format!("replacing {}", target.display())),
    }
}

/// Place one object at `target` according to `mode`, updating `report`.
fn place(
    object: &Path,
    target: &Path,
    executable: bool,
    mode: LinkMode,
    report: &mut MaterializeReport,
) -> Result<(), StoreError> {
    match mode {
        LinkMode::Copy => {
            copy_object(object, target, executable)?;
            report.copied += 1;
        }
        LinkMode::Auto => {
            if fs::hard_link(object, target).is_ok() {
                report.hard_linked += 1;
            } else {
                copy_object(object, target, executable)?;
                report.copied += 1;
            }
        }
    }
    Ok(())
}

/// Copy an object into place as a writable project file.
fn copy_object(object: &Path, target: &Path, executable: bool) -> Result<(), StoreError> {
    fs::copy(object, target)
        .io_context(|| format!("copying {} -> {}", object.display(), target.display()))?;
    object::set_writable_perms(target, executable)
        .io_context(|| format!("setting permissions on {}", target.display()))?;
    Ok(())
}

/// Whether `path` was modified within `grace` of now.
fn is_recent(path: &Path, grace: Duration) -> bool {
    if grace.is_zero() {
        return false;
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age < grace))
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}
