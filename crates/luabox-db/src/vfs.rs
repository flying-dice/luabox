//! Virtual file system: path interning plus a disk/overlay content store.
//!
//! The VFS is a plain data structure with no salsa dependency. It owns two
//! things:
//!
//! - **[`FileId`] interning** — a stable, dense integer id per path. An id is
//!   assigned once and never changes for the lifetime of the VFS, regardless
//!   of later content edits, so downstream maps can key on it cheaply.
//! - **A two-layer content store** — an on-disk text and an optional in-memory
//!   *overlay* (an editor buffer). The overlay always wins: an open, unsaved
//!   buffer shadows what is on disk. Clearing the overlay reverts to disk.
//!
//! There is no file watching here; applying disk changes is the caller's job
//! (SPEC.md §8 — the watch ticket owns that). [`AnalysisHost`](crate::AnalysisHost)
//! bridges VFS content into salsa inputs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use luabox_syntax::lua::Dialect;

/// A stable, interned identifier for a file path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);

impl FileId {
    /// The raw index, for debugging and dense side tables.
    #[must_use]
    pub fn index(self) -> u32 {
        self.0
    }
}

/// The disk + overlay content and dialect for one interned file.
#[derive(Debug, Clone)]
struct FileState {
    disk: Option<String>,
    overlay: Option<String>,
    dialect: Dialect,
}

/// Path interner and content store. See the module docs.
#[derive(Debug, Default)]
pub struct Vfs {
    paths: Vec<PathBuf>,
    ids: HashMap<PathBuf, FileId>,
    states: HashMap<FileId, FileState>,
}

impl Vfs {
    /// A fresh, empty VFS.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `path`, returning its stable [`FileId`].
    ///
    /// Idempotent: the same path always maps to the same id, and re-interning
    /// never disturbs stored content. `dialect` seeds a *newly* interned
    /// file; for an existing file it is ignored (use [`Vfs::set_dialect`]).
    ///
    /// # Panics
    ///
    /// Panics only if more than `u32::MAX` distinct paths are interned.
    pub fn intern(&mut self, path: PathBuf, dialect: Dialect) -> FileId {
        if let Some(&id) = self.ids.get(&path) {
            return id;
        }
        let id = FileId(u32::try_from(self.paths.len()).expect("more than u32::MAX files"));
        self.paths.push(path.clone());
        self.ids.insert(path, id);
        self.states.insert(
            id,
            FileState {
                disk: None,
                overlay: None,
                dialect,
            },
        );
        id
    }

    /// The id previously interned for `path`, if any. Never allocates a new id.
    #[must_use]
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.ids.get(path).copied()
    }

    /// The path an id was interned for.
    #[must_use]
    pub fn path(&self, id: FileId) -> &Path {
        &self.paths[id.0 as usize]
    }

    /// The dialect a file is parsed under.
    #[must_use]
    pub fn dialect(&self, id: FileId) -> Dialect {
        self.states[&id].dialect
    }

    /// Set the on-disk text (or `None` to mark it absent on disk).
    pub fn set_disk_text(&mut self, id: FileId, text: Option<String>) {
        self.state_mut(id).disk = text;
    }

    /// Set the in-memory overlay (an editor buffer). Shadows the disk text.
    pub fn set_overlay(&mut self, id: FileId, text: String) {
        self.state_mut(id).overlay = Some(text);
    }

    /// Drop the overlay, reverting the effective text to the disk content.
    pub fn clear_overlay(&mut self, id: FileId) {
        self.state_mut(id).overlay = None;
    }

    /// Whether a file currently has an overlay.
    #[must_use]
    pub fn has_overlay(&self, id: FileId) -> bool {
        self.states[&id].overlay.is_some()
    }

    /// Change the dialect a file is parsed under.
    pub fn set_dialect(&mut self, id: FileId, dialect: Dialect) {
        self.state_mut(id).dialect = dialect;
    }

    /// The effective text: the overlay if present, otherwise the disk text.
    #[must_use]
    pub fn effective_text(&self, id: FileId) -> Option<&str> {
        let state = self.states.get(&id)?;
        state.overlay.as_deref().or(state.disk.as_deref())
    }

    /// Read `id`'s path from disk into its disk-content layer.
    ///
    /// # Errors
    ///
    /// Propagates any I/O error from reading the file.
    pub fn load_from_disk(&mut self, id: FileId) -> std::io::Result<()> {
        let path = self.path(id).to_path_buf();
        let text = std::fs::read_to_string(&path)?;
        self.set_disk_text(id, Some(text));
        Ok(())
    }

    /// Every interned file id, in interning order.
    ///
    /// # Panics
    ///
    /// Panics only if more than `u32::MAX` distinct paths were interned.
    pub fn ids(&self) -> impl Iterator<Item = FileId> + '_ {
        (0..self.paths.len()).map(|i| FileId(u32::try_from(i).expect("more than u32::MAX files")))
    }

    fn state_mut(&mut self, id: FileId) -> &mut FileState {
        self.states.get_mut(&id).expect("file id not interned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable_across_edits() {
        let mut vfs = Vfs::new();
        let a = vfs.intern(PathBuf::from("a.lua"), Dialect::Lua54);
        let b = vfs.intern(PathBuf::from("b.lua"), Dialect::Lua54);
        assert_ne!(a, b);

        // Re-interning the same path yields the same id.
        assert_eq!(a, vfs.intern(PathBuf::from("a.lua"), Dialect::Lua51));
        assert_eq!(vfs.file_id(Path::new("a.lua")), Some(a));

        // Content edits never change the id.
        vfs.set_disk_text(a, Some("x = 1".into()));
        vfs.set_overlay(a, "x = 2".into());
        vfs.clear_overlay(a);
        assert_eq!(a, vfs.intern(PathBuf::from("a.lua"), Dialect::Lua54));

        // Interning a new path keeps the dense sequence going.
        let c = vfs.intern(PathBuf::from("c.lua"), Dialect::Lua54);
        assert_eq!(c.index(), 2);
    }

    #[test]
    fn overlay_beats_disk() {
        let mut vfs = Vfs::new();
        let a = vfs.intern(PathBuf::from("a.lua"), Dialect::Lua54);
        vfs.set_disk_text(a, Some("from disk".into()));
        assert_eq!(vfs.effective_text(a), Some("from disk"));

        vfs.set_overlay(a, "from editor".into());
        assert_eq!(vfs.effective_text(a), Some("from editor"));
        assert!(vfs.has_overlay(a));

        vfs.clear_overlay(a);
        assert_eq!(vfs.effective_text(a), Some("from disk"));
        assert!(!vfs.has_overlay(a));
    }

    #[test]
    fn overlay_only_file_has_no_disk() {
        let mut vfs = Vfs::new();
        let a = vfs.intern(PathBuf::from("scratch.lua"), Dialect::Lua54);
        assert_eq!(vfs.effective_text(a), None);
        vfs.set_overlay(a, "print(1)".into());
        assert_eq!(vfs.effective_text(a), Some("print(1)"));
        vfs.clear_overlay(a);
        assert_eq!(vfs.effective_text(a), None);
    }
}
