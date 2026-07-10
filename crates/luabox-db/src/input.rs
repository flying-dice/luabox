//! Salsa inputs — the leaves of the query graph.
//!
//! Everything the analysis reads bottoms out in these. Changing an input field
//! (via a [`salsa::Setter`]) starts a new revision and invalidates exactly the
//! queries that read that field; untouched files are never re-analysed.

use std::path::PathBuf;

use luabox_syntax::lua::Dialect;
use luabox_types::Strictness;

/// One source file: its path, current text, and dialect.
///
/// The *text* is the effective content the [`Vfs`](crate::Vfs) resolves — an
/// editor overlay when one is set, otherwise the on-disk bytes. Editing it is
/// how a keystroke reaches the query engine.
#[salsa::input(debug)]
pub struct SourceFile {
    /// The file's path (a logical name for diagnostics; need not exist on disk).
    #[returns(ref)]
    pub path: PathBuf,
    /// The current source text.
    #[returns(ref)]
    pub text: String,
    /// The dialect this file is parsed under.
    pub dialect: Dialect,
}

/// Project-level configuration: the strictness ladder and the file set.
///
/// `strictness` is read by every file's `diagnostics` query, so changing it
/// re-checks the whole project; `files` is read only by
/// [`project_diagnostics`](crate::project_diagnostics), so adding a file does
/// not disturb the per-file memos of the others.
#[salsa::input(debug)]
pub struct Project {
    /// The active strictness level (SPEC.md §3).
    pub strictness: Strictness,
    /// Every file in the project.
    #[returns(ref)]
    pub files: Vec<SourceFile>,
}
