//! A coarse, best-effort advisory lock for garbage collection.
//!
//! `gc` mutates the store (it deletes objects), so two collectors must not run
//! at once. Rather than pull in a file-locking crate, this uses the oldest
//! portable primitive: an exclusive-create lock *file* (`gc.lock`). The winner
//! is whoever manages `create_new` first; it records its PID and a creation
//! timestamp so a lock orphaned by a crashed process can be detected and
//! stolen once it ages past a grace period.
//!
//! ## What this guarantees, and what it does not
//!
//! - It **serializes cooperating `gc` runs** in this toolchain. It is advisory:
//!   nothing stops an unrelated process from deleting store files.
//! - Stale-detection is heuristic. A very long, legitimate `gc` could in
//!   principle be robbed of its lock by another collector after the timeout.
//!   The timeout is set well above any realistic collection, and `gc` itself is
//!   written to be safe under concurrent *readers* regardless (it never removes
//!   recently-touched objects — see `store::Store::gc`).
//! - It protects `gc` against `gc`. It does **not** block installs: `put`/
//!   `materialize` are safe against a concurrent `gc` by the grace-period rule,
//!   not by this lock.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::error::{IoResultExt, StoreError};

/// A held gc lock. Dropping it releases (deletes) the lock file.
#[derive(Debug)]
pub(crate) struct GcLock {
    path: PathBuf,
}

impl GcLock {
    /// Try to acquire the lock, stealing it if the existing one is older than
    /// `stale_after`.
    ///
    /// # Errors
    /// Fails if another live collector holds the lock, or on unexpected I/O.
    pub(crate) fn acquire(lock_path: PathBuf, stale_after: Duration) -> Result<Self, StoreError> {
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .io_context(|| format!("creating store dir {}", parent.display()))?;
        }
        match Self::try_create(&lock_path) {
            Ok(()) => Ok(Self { path: lock_path }),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if Self::is_stale(&lock_path, stale_after) {
                    // Steal: remove the orphaned lock and retry exactly once.
                    let _ = fs::remove_file(&lock_path);
                    Self::try_create(&lock_path)
                        .io_context(|| "re-acquiring stale gc lock".to_string())?;
                    Ok(Self { path: lock_path })
                } else {
                    Err(StoreError::GcLocked {
                        path: lock_path.display().to_string(),
                    })
                }
            }
            Err(err) => Err(err).io_context(|| format!("creating gc lock {}", lock_path.display())),
        }
    }

    fn try_create(path: &Path) -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        writeln!(file, "pid={} ts={}", std::process::id(), nanos)?;
        file.sync_all()
    }

    fn is_stale(path: &Path, stale_after: Duration) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            // Vanished between the failed create and now: treat as stealable.
            return true;
        };
        let Ok(modified) = meta.modified() else {
            return false;
        };
        modified.elapsed().is_ok_and(|age| age > stale_after)
    }
}

impl Drop for GcLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
