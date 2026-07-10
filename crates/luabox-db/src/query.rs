//! The memoized query graph over the [`SourceFile`]/[`Project`] inputs.
//!
//! ```text
//!   SourceFile.text ─┐
//!   SourceFile.dialect ─▶ parse ─┬─▶ annotations
//!                                ├─▶ type_env
//!                                └─▶ diagnostics ◀── Project.strictness
//!                                         │
//!                     Project.files ─▶ project_diagnostics
//! ```
//!
//! Each `#[salsa::tracked]` function runs at most once per `(inputs, revision)`
//! and is served from cache otherwise. Every function logs when it *executes*
//! (via [`Db::push_log`]) so tests can prove which queries re-ran after an
//! edit.

use luabox_types::{TypeEnv, check_file};

use crate::db::Db;
use crate::input::{Project, SourceFile};
use crate::value::{Annotations, Diagnostics, LoweredHandle, ParsedModule, TypeEnvHandle};

/// Parse a file into a lossless syntax tree.
///
/// Depends only on the file's `text` and `dialect`; a project with a hundred
/// untouched files re-parses none of them when one file changes.
#[salsa::tracked]
pub fn parse(db: &dyn Db, file: SourceFile) -> ParsedModule {
    db.push_log(format!("parse({})", display(db, file)));
    ParsedModule::new(luabox_syntax::lua::parse(file.text(db), file.dialect(db)))
}

/// Harvest the LuaCATS annotation blocks for a file.
#[salsa::tracked]
pub fn annotations(db: &dyn Db, file: SourceFile) -> Annotations {
    db.push_log(format!("annotations({})", display(db, file)));
    let parsed = parse(db, file);
    Annotations::new(luabox_syntax::luacats::harvest(parsed.parse()))
}

/// Build the per-file type environment (declared classes, enums, aliases,
/// annotated signatures).
///
/// `no_eq`: [`TypeEnv`] is not comparable, so this query never backdates — a
/// re-parse always yields a fresh environment. See [`TypeEnvHandle`].
#[salsa::tracked(no_eq)]
pub fn type_env(db: &dyn Db, file: SourceFile) -> TypeEnvHandle {
    db.push_log(format!("type_env({})", display(db, file)));
    let parsed = parse(db, file);
    TypeEnvHandle::new(TypeEnv::build(parsed.parse()))
}

/// Lower a file to HIR: desugared bodies plus name resolution
/// (local/upvalue/global), the source map, and the `require` graph.
///
/// This is exactly [`luabox_hir::lower`] over the memoized parse — the query
/// behind LSP goto-definition and scope-aware completion.
///
/// `no_eq`: [`luabox_hir::LoweredFile`] is not comparable, so this query never
/// backdates — a re-parse always yields a fresh lowering. See
/// [`LoweredHandle`].
#[salsa::tracked(no_eq)]
pub fn lower(db: &dyn Db, file: SourceFile) -> LoweredHandle {
    db.push_log(format!("lower({})", display(db, file)));
    let parsed = parse(db, file);
    LoweredHandle::new(luabox_hir::lower(parsed.parse()))
}

/// Typecheck a file against its own annotations at the project strictness.
///
/// This is exactly [`luabox_types::check_file`] over the memoized parse: the
/// value the LSP and `luabox check` consume per file. Depends on the file's
/// parse and on `Project.strictness` only — not on the other files.
#[salsa::tracked]
pub fn diagnostics(db: &dyn Db, file: SourceFile, project: Project) -> Diagnostics {
    db.push_log(format!("diagnostics({})", display(db, file)));
    let parsed = parse(db, file);
    let name = display(db, file);
    Diagnostics::new(check_file(parsed.parse(), &name, project.strictness(db)))
}

/// Aggregate every file's diagnostics into one project-wide set.
///
/// Reads `Project.files` and each file's [`diagnostics`]; editing one file
/// re-runs that file's `diagnostics` and this aggregator, but no other file's.
#[salsa::tracked]
pub fn project_diagnostics(db: &dyn Db, project: Project) -> Diagnostics {
    db.push_log("project_diagnostics()".to_string());
    let mut all = Vec::new();
    for &file in project.files(db) {
        all.extend(diagnostics(db, file, project).diagnostics().iter().cloned());
    }
    Diagnostics::new(all)
}

/// The diagnostic file name for a source file (its path, lossily rendered).
fn display(db: &dyn Db, file: SourceFile) -> String {
    file.path(db).to_string_lossy().into_owned()
}
