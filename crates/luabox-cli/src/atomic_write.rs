//! Replacing a file the *user* wrote, without a window in which it does not
//! exist.
//!
//! `fs::write` is `open(O_TRUNC)` followed by `write`. Between those two the
//! user's source is a zero-byte file, and if the write then fails part-way —
//! a full disk, a quota, an `RLIMIT_FSIZE` — what is left on disk is a
//! truncated fragment and the original is gone. It is not recoverable: luabox
//! is the only process that still had the bytes, and it has just exited.
//! Reproduced by the round-8 review with `ulimit -f 8`: a 97,780-byte source
//! came back as 8,192 bytes, and `luabox fmt` was the only thing that had
//! touched it.
//!
//! [`write_atomic`] is the replacement every rewrite of a user-authored file
//! goes through ( `fmt`, and `lint --fix` ). It stages the *whole* new content
//! in a sibling temp file, `fsync`s it, then `rename`s it over the target.
//! `rename(2)` within one directory is atomic: a concurrent reader sees either
//! every old byte or every new byte, never a prefix, and a crash at any point
//! leaves one of those two states. The temp file is a sibling deliberately —
//! it has to be on the same filesystem, or `rename` degrades to a copy and the
//! atomicity is gone.
//!
//! ## What this does not preserve
//!
//! **Hardlinks break.** The rename installs a *new inode* under the target's
//! name, so another name for the old inode keeps pointing at the old content.
//! That is the accepted trade (round-8 review): a hardlinked source is rare,
//! and losing the file outright is worse than the link going stale. Symlinks
//! are *not* in that trade — see below.
//!
//! ## Symlinks are followed, on purpose
//!
//! `src/main.lua -> ../real.lua` is a supported layout (a prior review pinned
//! "writes go through the link" as behaviour), and renaming over the link
//! *itself* would replace it with a regular file and quietly detach the real
//! source. So the target is [`fs::canonicalize`]d first and the rename lands
//! on the resolved path, in the resolved path's own directory.
//!
//! ## Which writes are "the user's"
//!
//! Only files the user authored are replaced this way. `init`/`new` create
//! files that did not exist (there is nothing to destroy), and `build`/`doc`
//! write into an output directory luabox owns and regenerates — a truncated
//! artifact there is fixed by running the command again.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Disambiguates temp files created within the same process and directory —
/// `lint --fix` rewrites files from several rayon workers at once.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replace `path`'s contents with `contents`, atomically.
///
/// Writes a sibling temp file, `fsync`s it, copies the target's permissions
/// onto it, and renames it over the target's *resolved* (symlink-followed)
/// path. On any failure the temp file is removed (best effort) and the target
/// is left exactly as it was — that is the whole point of the module.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    // Resolve first: the bytes belong to whatever `path` ultimately names, and
    // the temp file has to be a sibling of *that* to rename over it.
    let target = fs::canonicalize(path)?;
    let directory = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{}` has no parent directory", target.display()),
        )
    })?;
    let temp = directory.join(temp_file_name());
    match stage_and_rename(&target, &temp, contents) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Best effort by construction: the reason the stage failed may be
            // the same reason the cleanup does. Never mask the real error.
            let _ = fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// The whole staged write, so [`write_atomic`] has one place to clean up from.
///
/// `sync_all` before the rename is what makes the guarantee survive a crash
/// rather than merely a failed write: without it the rename can reach the disk
/// before the data does, and the target's name would point at an inode whose
/// blocks were never written.
fn stage_and_rename(target: &Path, temp: &Path, contents: &str) -> io::Result<()> {
    let mut file = File::create(temp)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    copy_permissions(target, temp)?;
    fs::rename(temp, target)
}

/// Give the staged file the target's permissions before it takes the target's
/// name, so an executable script or a group-writable source keeps its mode
/// across the replace. A fresh `File::create` would otherwise land on
/// `0o666 & !umask`.
#[cfg(unix)]
fn copy_permissions(from: &Path, to: &Path) -> io::Result<()> {
    fs::set_permissions(to, fs::metadata(from)?.permissions())
}

/// Windows access control is inherited from the containing directory rather
/// than carried in a mode word, and `fs::Permissions` there exposes only the
/// read-only flag — copying that alone would be a half-truth. The new file
/// inherits the directory's ACL, which is what a file created in that
/// directory gets anyway.
// The `Result` return mirrors the unix twin so the call site is
// cfg-free; clippy (rightly) notices this arm can never fail.
#[cfg(not(unix))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "signature must match the cfg(unix) twin, which is fallible"
)]
fn copy_permissions(_from: &Path, _to: &Path) -> io::Result<()> {
    Ok(())
}

/// A name for the staged file that **no** part of luabox will look at.
///
/// Three independent rules already exclude it, so it cannot be picked up as
/// source nor trigger a watch rerun even if one of them were relaxed:
/// - it is dot-prefixed, and `layout::is_in_project_tree` rejects any path
///   with a dot-prefixed component;
/// - its extension is `.tmp`, not `.lua`, so `layout::is_project_source`
///   rejects it on kind;
/// - `watch::is_editor_temp` treats a `*.tmp` name as editor scratch, so
///   `watch::is_relevant` rejects it too.
///
/// The pid + sequence keep two concurrent rewrites in one directory (rayon, or
/// two luabox processes) from staging into the same file.
fn temp_file_name() -> String {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(".luabox-{}-{sequence}.tmp", std::process::id())
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{temp_file_name, write_atomic};
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A file at `root/name` holding `contents`.
    fn file(root: &Path, name: &str, contents: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, contents).expect("write fixture");
        path
    }

    #[test]
    fn a_replace_leaves_exactly_the_new_contents() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = file(tmp.path(), "main.lua", "local x = 1\n");

        write_atomic(&path, "local x = 2\n").expect("replace succeeds");

        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            "local x = 2\n"
        );
    }

    #[test]
    fn no_staged_file_is_left_behind_by_a_successful_replace() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = file(tmp.path(), "main.lua", "local x = 1\n");

        write_atomic(&path, "local x = 2\n").expect("replace succeeds");

        let left: Vec<String> = fs::read_dir(tmp.path())
            .expect("list")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["main.lua"]);
    }

    #[test]
    fn a_target_that_does_not_exist_is_an_error_rather_than_a_create() {
        // `write_atomic` replaces; creating a new file is `fs::write`'s job
        // (scaffolding). Resolving the target is what enforces that.
        let tmp = tempfile::tempdir().expect("tempdir");
        let error = write_atomic(&tmp.path().join("ghost.lua"), "x").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    #[cfg(unix)]
    fn the_targets_permissions_survive_the_replace() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        let path = file(tmp.path(), "run.lua", "-- old\n");
        // An executable, group-and-other-readable script: a mode a fresh
        // `File::create` would never produce under any umask.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");

        write_atomic(&path, "-- new\n").expect("replace succeeds");

        let mode = fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "mode was {:o}", mode & 0o777);
        assert_eq!(fs::read_to_string(&path).expect("read back"), "-- new\n");
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_source_is_rewritten_through_the_link() {
        // `src/main.lua -> ../real.lua` is a supported layout: the bytes must
        // land in `real.lua`, and the link must still be a link afterwards.
        let tmp = tempfile::tempdir().expect("tempdir");
        let real = file(tmp.path(), "real.lua", "local x = 1\n");
        fs::create_dir(tmp.path().join("src")).expect("mkdir src");
        let link = tmp.path().join("src").join("main.lua");
        std::os::unix::fs::symlink("../real.lua", &link).expect("symlink");

        write_atomic(&link, "local x = 2\n").expect("replace succeeds");

        assert_eq!(
            fs::read_to_string(&real).expect("read real"),
            "local x = 2\n"
        );
        assert!(
            fs::symlink_metadata(&link)
                .expect("stat link")
                .file_type()
                .is_symlink(),
            "the link must survive the replace, not be replaced by a file"
        );
        // ...and no staged file was left in either directory.
        assert_eq!(
            fs::read_dir(tmp.path().join("src")).expect("list").count(),
            1
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_write_that_cannot_be_staged_leaves_the_original_byte_identical() {
        use std::os::unix::fs::PermissionsExt as _;

        let tmp = tempfile::tempdir().expect("tempdir");
        let original = "local x = 1\n-- a source nobody wants to lose\n";
        let path = file(tmp.path(), "main.lua", original);

        // Deny creation in the directory, so staging the temp file fails
        // before a single byte of the target could have been touched.
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o555)).expect("chmod dir");
        let enforced = fs::File::create(tmp.path().join("probe")).is_err();
        if !enforced {
            // Running as a user that ignores the mode bits (root in a
            // container); the injection is not available, so say so rather
            // than assert something the environment never arranged.
            let _ = fs::remove_file(tmp.path().join("probe"));
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).expect("restore");
            println!("skipped: this user writes into a mode-0555 directory anyway");
            return;
        }

        let error = write_atomic(&path, "truncated garbage").unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755)).expect("restore");
        assert_eq!(
            fs::read_to_string(&path).expect("read back"),
            original,
            "a failed replace must not have touched the target"
        );
    }

    #[test]
    fn the_staged_file_name_is_invisible_to_the_source_walk_and_the_watcher() {
        // If the name were visible, `fmt` could try to format its own scratch
        // file and `--watch` would rerun for every rewrite it makes.
        let root = Path::new("/proj");
        let staged = Path::new("/proj/src").join(temp_file_name());
        assert!(!luabox_manifest::layout::is_project_source(
            &staged, root, None
        ));
        assert!(!luabox_manifest::layout::is_in_project_tree(
            &staged, root, None
        ));
        assert!(!crate::watch::is_relevant(&staged, root, None));
    }

    #[test]
    fn staged_file_names_are_unique_within_a_process() {
        let first = temp_file_name();
        let second = temp_file_name();
        assert_ne!(first, second);
        assert!(first.starts_with(".luabox-"), "{first}");
        assert_eq!(
            Path::new(&first).extension(),
            Some("tmp".as_ref()),
            "{first}"
        );
    }
}
