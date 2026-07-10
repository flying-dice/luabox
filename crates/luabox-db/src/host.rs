//! The public boundary: [`AnalysisHost`] (owns the mutable world) and
//! [`Analysis`] (an immutable snapshot the queries run against).
//!
//! This is the exact surface the LSP server (P1, ticket #14) consumes:
//! - the server keeps one [`AnalysisHost`], feeds editor/disk edits in through
//!   [`AnalysisHost::apply_change`], and
//! - for each request takes a cheap [`AnalysisHost::snapshot`] and answers from
//!   it ([`Analysis::diagnostics`], [`Analysis::parse`], …), so analysis can
//!   proceed concurrently with further edits.
//!
//! Nothing else in the workspace touches the salsa database directly; the
//! `check`, `lint`, and `fmt` front-ends will consume the same two types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{self, Dialect};
use luabox_types::Strictness;
use salsa::Setter;

use crate::db::RootDatabase;
use crate::input::{Project, SourceFile};
use crate::query;
use crate::value::{Annotations, BindingTypes, LoweredHandle, ParsedModule, TypeEnvHandle};
use crate::vfs::{FileId, Vfs};

/// One atomic edit to apply to the world. Batch several with
/// [`AnalysisHost::apply_changes`].
#[derive(Debug, Clone)]
pub enum Change {
    /// Set (or replace) a file's on-disk text. Interns the path if new.
    SetFileText {
        /// The file path.
        path: PathBuf,
        /// The dialect to parse it under.
        dialect: Dialect,
        /// The new on-disk text.
        text: String,
    },
    /// Set an editor overlay for a file — shadows disk until cleared.
    SetOverlay {
        /// The file path (interned under the host's default dialect if new).
        path: PathBuf,
        /// The editor buffer contents.
        text: String,
    },
    /// Drop a file's overlay, reverting to its on-disk text.
    ClearOverlay {
        /// The file path.
        path: PathBuf,
    },
    /// Change the dialect a file is parsed under.
    SetDialect {
        /// The file path.
        path: PathBuf,
        /// The new dialect.
        dialect: Dialect,
    },
    /// Change the project-wide strictness level.
    SetStrictness(Strictness),
}

/// Owns the incremental database and the VFS; the single mutable entry point.
pub struct AnalysisHost {
    db: RootDatabase,
    vfs: Vfs,
    inputs: HashMap<FileId, SourceFile>,
    project: Project,
    default_dialect: Dialect,
}

impl AnalysisHost {
    /// A host with the given defaults for files whose dialect/strictness is not
    /// otherwise specified.
    #[must_use]
    pub fn new(default_dialect: Dialect, strictness: Strictness) -> Self {
        let db = RootDatabase::default();
        let project = Project::new(&db, strictness, Vec::new());
        Self {
            db,
            vfs: Vfs::new(),
            inputs: HashMap::new(),
            project,
            default_dialect,
        }
    }

    /// The read-only VFS (path interning, overlay state).
    #[must_use]
    pub fn vfs(&self) -> &Vfs {
        &self.vfs
    }

    /// Apply one [`Change`], updating the VFS and the affected salsa inputs.
    pub fn apply_change(&mut self, change: Change) {
        match change {
            Change::SetFileText {
                path,
                dialect,
                text,
            } => {
                let id = self.vfs.intern(path, dialect);
                self.vfs.set_disk_text(id, Some(text));
                self.sync_file(id);
            }
            Change::SetOverlay { path, text } => {
                let id = self.vfs.intern(path, self.default_dialect);
                self.vfs.set_overlay(id, text);
                self.sync_file(id);
            }
            Change::ClearOverlay { path } => {
                if let Some(id) = self.vfs.file_id(&path) {
                    self.vfs.clear_overlay(id);
                    self.sync_file(id);
                }
            }
            Change::SetDialect { path, dialect } => {
                let id = self.vfs.intern(path, dialect);
                self.vfs.set_dialect(id, dialect);
                self.sync_file(id);
            }
            Change::SetStrictness(strictness) => {
                self.project.set_strictness(&mut self.db).to(strictness);
            }
        }
    }

    /// Apply a batch of changes in order.
    pub fn apply_changes(&mut self, changes: impl IntoIterator<Item = Change>) {
        for change in changes {
            self.apply_change(change);
        }
    }

    /// Take a cheap, immutable snapshot to answer queries against.
    #[must_use]
    pub fn snapshot(&self) -> Analysis {
        let files = self
            .inputs
            .iter()
            .map(|(&id, &input)| (self.vfs.path(id).to_path_buf(), input))
            .collect();
        Analysis {
            db: self.db.clone(),
            files,
            project: self.project,
        }
    }

    /// Drain the query execution trace collected since the last call — the
    /// list of queries that actually *ran* (cache misses). Test/tracing aid.
    #[must_use]
    pub fn take_execution_log(&self) -> Vec<String> {
        self.db.take_logs()
    }

    /// Reconcile the effective VFS text/dialect for `id` into salsa, creating
    /// the [`SourceFile`] input on first sight and updating fields only when
    /// they actually change (so no spurious revisions).
    fn sync_file(&mut self, id: FileId) {
        let text = self.vfs.effective_text(id).unwrap_or("").to_owned();
        let dialect = self.vfs.dialect(id);
        if let Some(input) = self.inputs.get(&id).copied() {
            if input.text(&self.db) != &text {
                input.set_text(&mut self.db).to(text);
            }
            if input.dialect(&self.db) != dialect {
                input.set_dialect(&mut self.db).to(dialect);
            }
        } else {
            let path = self.vfs.path(id).to_path_buf();
            let input = SourceFile::new(&self.db, path, text, dialect);
            self.inputs.insert(id, input);
            let mut files = self.project.files(&self.db).clone();
            files.push(input);
            self.project.set_files(&mut self.db).to(files);
        }
    }
}

/// An immutable snapshot of the analysis world. Cheap to create (a structural
/// database clone); safe to hold and query while the host applies more edits.
pub struct Analysis {
    db: RootDatabase,
    files: HashMap<PathBuf, SourceFile>,
    project: Project,
}

impl Analysis {
    /// The typecheck diagnostics for `path`, or `None` if it is not a known
    /// file. Equal to [`luabox_types::check_file`] over the file's source.
    #[must_use]
    pub fn diagnostics(&self, path: &Path) -> Option<Vec<Diagnostic>> {
        let file = *self.files.get(path)?;
        Some(query::diagnostics(&self.db, file, self.project).to_vec())
    }

    /// Every file's diagnostics, aggregated across the project.
    #[must_use]
    pub fn project_diagnostics(&self) -> Vec<Diagnostic> {
        query::project_diagnostics(&self.db, self.project).to_vec()
    }

    /// The parsed module for `path` — lossless tree plus parse errors.
    #[must_use]
    pub fn parse(&self, path: &Path) -> Option<ParsedModule> {
        let file = *self.files.get(path)?;
        Some(query::parse(&self.db, file))
    }

    /// The root syntax node for `path`, for tree walking (fmt/lint/LSP).
    #[must_use]
    pub fn syntax(&self, path: &Path) -> Option<lua::SyntaxNode> {
        self.parse(path).map(|p| p.syntax())
    }

    /// The harvested LuaCATS annotations for `path`.
    #[must_use]
    pub fn annotations(&self, path: &Path) -> Option<Annotations> {
        let file = *self.files.get(path)?;
        Some(query::annotations(&self.db, file))
    }

    /// The per-file type environment for `path`.
    #[must_use]
    pub fn type_env(&self, path: &Path) -> Option<TypeEnvHandle> {
        let file = *self.files.get(path)?;
        Some(query::type_env(&self.db, file))
    }

    /// The HIR lowering for `path` — name resolution, source map, requires.
    #[must_use]
    pub fn lower(&self, path: &Path) -> Option<LoweredHandle> {
        let file = *self.files.get(path)?;
        Some(query::lower(&self.db, file))
    }

    /// The display-mode inference for `path` — the inlay-hint surface
    /// (binding types + inferred returns, cross-file aware).
    #[must_use]
    pub fn binding_types(&self, path: &Path) -> Option<BindingTypes> {
        let file = *self.files.get(path)?;
        Some(query::binding_types(&self.db, file, self.project))
    }

    /// The current effective text of `path` (overlay when set, disk
    /// otherwise), or `None` if it is not a known file.
    #[must_use]
    pub fn file_text(&self, path: &Path) -> Option<String> {
        let file = *self.files.get(path)?;
        Some(file.text(&self.db).clone())
    }

    /// Every file path this snapshot knows about, in unspecified order.
    pub fn files(&self) -> impl Iterator<Item = &Path> {
        self.files.keys().map(PathBuf::as_path)
    }
}
