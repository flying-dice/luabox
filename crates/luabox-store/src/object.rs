//! The object layer: content-addressed storage with write-once semantics.
//!
//! Objects live at `objects/sha256/<2-char prefix>/<remaining 62 chars>`. The
//! two-character fan-out keeps any single directory from holding the whole
//! store (the same reason git and pnpm shard by prefix).
//!
//! Writes are **write-once and atomic**: content is streamed into a temp file
//! on the same volume, made read-only, then `rename`d into place. Because the
//! address *is* the content hash, two processes writing the same object produce
//! byte-identical results and the rename is idempotent — a concurrent double
//! write can never yield a torn object.
//!
//! Objects are made read-only to protect the CAS (pnpm does the same). A
//! hard-linked project file therefore shares that read-only bit; callers who
//! need writable files materialize with `LinkMode::Copy` (see `store`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{IoResultExt, StoreError};

/// Monotonic tie-breaker so temp names are unique within a process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Content-addressed object storage rooted at a store directory.
#[derive(Debug, Clone)]
pub(crate) struct ObjectStore {
    /// `<root>/objects/sha256`.
    objects_dir: PathBuf,
    /// `<root>/tmp` — staging on the same volume as `objects_dir`.
    tmp_dir: PathBuf,
}

impl ObjectStore {
    pub(crate) fn new(objects_dir: PathBuf, tmp_dir: PathBuf) -> Self {
        Self {
            objects_dir,
            tmp_dir,
        }
    }

    /// Absolute path an object with `hash` occupies (whether or not it exists).
    pub(crate) fn object_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2.min(hash.len()));
        self.objects_dir.join(prefix).join(rest)
    }

    /// The root under which every object lives (for `stats`/`gc` walks).
    pub(crate) fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    /// Whether an object with `hash` is present.
    pub(crate) fn has(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// Copy `src` into the store under `hash`, write-once and atomically.
    ///
    /// `executable` records the Unix executable bit onto the stored object so
    /// hard-linked materializations inherit it.
    pub(crate) fn put_file(
        &self,
        src: &Path,
        hash: &str,
        executable: bool,
    ) -> Result<(), StoreError> {
        if hash.len() < 3 {
            return Err(StoreError::InvalidHash {
                hash: hash.to_string(),
            });
        }
        let dst = self.object_path(hash);
        if dst.exists() {
            return Ok(());
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .io_context(|| format!("creating object dir {}", parent.display()))?;
        }
        fs::create_dir_all(&self.tmp_dir)
            .io_context(|| format!("creating temp dir {}", self.tmp_dir.display()))?;

        let tmp = self.temp_path(hash);
        fs::copy(src, &tmp)
            .io_context(|| format!("staging {} -> {}", src.display(), tmp.display()))?;
        set_object_perms(&tmp, executable)
            .io_context(|| format!("sealing temp object {}", tmp.display()))?;

        match fs::rename(&tmp, &dst) {
            Ok(()) => Ok(()),
            Err(err) => {
                // Either another writer won the race (dst now exists — fine, the
                // content is identical) or the rename genuinely failed.
                let _ = remove_file(&tmp);
                if dst.exists() {
                    Ok(())
                } else {
                    Err(err).io_context(|| format!("committing object {}", dst.display()))
                }
            }
        }
    }

    /// A unique temp path on the object volume: `<hash>.<pid>.<n>.<nanos>.tmp`.
    fn temp_path(&self, hash: &str) -> PathBuf {
        let pid = std::process::id();
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        self.tmp_dir.join(format!("{hash}.{pid}.{n}.{nanos}.tmp"))
    }
}

/// Seal a stored object: read-only, plus the Unix executable bit if requested.
#[cfg(unix)]
pub(crate) fn set_object_perms(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o555 } else { 0o444 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Seal a stored object read-only. Windows has no executable bit.
#[cfg(not(unix))]
pub(crate) fn set_object_perms(path: &Path, _executable: bool) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(true);
    fs::set_permissions(path, perms)
}

/// Set permissions on a **copy-materialized** (writable) project file.
#[cfg(unix)]
pub(crate) fn set_writable_perms(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Clear the read-only attribute copied over from the object on Windows.
///
/// `set_readonly(false)` is precisely the Windows API for clearing the
/// read-only file attribute; the world-writable concern the lint raises applies
/// only to Unix, which uses the mode-based branch above.
#[cfg(not(unix))]
#[allow(
    clippy::permissions_set_readonly_false,
    reason = "clearing the read-only attribute is the intended Windows operation here; \
              the world-writable concern the lint raises is Unix-only and handled by the \
              mode-based branch above"
)]
pub(crate) fn set_writable_perms(path: &Path, _executable: bool) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Remove a file, clearing a read-only attribute first if that blocks it.
///
/// Store objects are read-only; on Windows `remove_file` refuses read-only
/// files outright, so `gc` must clear the attribute before deleting.
pub(crate) fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            clear_readonly(path)?;
            fs::remove_file(path)
        }
        Err(err) => Err(err),
    }
}

/// Clear a read-only attribute so the file can be deleted.
///
/// This exists for Windows, which refuses to `remove_file` a read-only file. On
/// Unix, deletability depends on the parent directory, not the file mode, so
/// this branch is effectively unreached there; the momentary world-writable
/// window the lint warns about is closed immediately by the delete that
/// follows, hence the targeted allow.
#[allow(
    clippy::permissions_set_readonly_false,
    reason = "Windows refuses to remove a read-only file, so the attribute must be cleared \
              before the delete; the momentary writable window is closed immediately by the \
              remove that follows"
)]
fn clear_readonly(path: &Path) -> io::Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    if perms.readonly() {
        perms.set_readonly(false);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}
