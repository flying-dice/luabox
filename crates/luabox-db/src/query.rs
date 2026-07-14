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

use std::collections::HashMap;

use luabox_types::ty::Ty;
use luabox_types::{ExternalTypes, TypeEnv, check_file, infer_display_types, stdlib_defs};

use crate::db::Db;
use crate::input::{Project, SourceFile};
use crate::value::{
    Annotations, BindingTypes, Diagnostics, LoweredHandle, ModuleExport, ModuleSurfaceChecked,
    OutgoingCalls, ParsedModule, TypeEnvHandle,
};

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

/// The outgoing-call arguments of one file: what it passes, by callee
/// name, to functions it does not define — the parameter seeds it
/// contributes to the files it requires. Computed *standalone* (no
/// cross-file inputs; depends only on this file's parse), which is what
/// keeps the cross-file query graph acyclic under require cycles.
#[salsa::tracked]
pub fn outgoing_calls(db: &dyn Db, file: SourceFile) -> OutgoingCalls {
    db.push_log(format!("outgoing_calls({})", display(db, file)));
    let parsed = parse(db, file);
    let name = display(db, file);
    let ambient = stdlib_defs(file.dialect(db));
    OutgoingCalls::new(
        infer_display_types(parsed.parse(), &name, Some(ambient), None).outgoing_calls,
    )
}

/// The inferred module export of one file (the chunk's `return` value),
/// with its exported functions' parameters seeded from dependent files'
/// observed call arguments — so exported signatures carry real parameter
/// *and* return types. The file's own requires are not followed (their
/// exports would recurse); acyclic because [`outgoing_calls`] is
/// standalone.
#[salsa::tracked]
pub fn module_export(db: &dyn Db, file: SourceFile, project: Project) -> ModuleExport {
    db.push_log(format!("module_export({})", display(db, file)));
    let parsed = parse(db, file);
    let name = display(db, file);
    let ambient = stdlib_defs(file.dialect(db));
    let externals = ExternalTypes {
        requires: HashMap::new(),
        fn_param_seeds: dependent_seeds(db, file, project),
    };
    ModuleExport::new(
        infer_display_types(parsed.parse(), &name, Some(ambient), Some(&externals)).module_export,
    )
}

/// The display-mode inference for one file — the LSP inlay-hint surface:
/// binding types and inferred function returns, with call-site parameter
/// seeding, the file's require exports in scope, and exported functions'
/// parameters seeded from every dependent file's observed call arguments.
/// The stdlib definition layer for the file's dialect is merged beneath
/// the file's own annotations.
#[salsa::tracked]
pub fn binding_types(db: &dyn Db, file: SourceFile, project: Project) -> BindingTypes {
    db.push_log(format!("binding_types({})", display(db, file)));
    let parsed = parse(db, file);
    let name = display(db, file);
    let ambient = stdlib_defs(file.dialect(db));
    let externals = ExternalTypes {
        requires: require_exports(db, file, project),
        fn_param_seeds: dependent_seeds(db, file, project),
    };
    BindingTypes::new(infer_display_types(
        parsed.parse(),
        &name,
        Some(ambient),
        Some(&externals),
    ))
}

/// The **check-mode** module surface of one file (#85): the type a
/// consumer's `require` of this module evaluates to for type *checking* —
/// annotations authoritative, no call-site parameter seeding (unlike
/// [`module_export`], the display-mode inlay-hint surface, which seeds
/// exported functions' parameters from dependents' call sites) — plus the
/// file's workspace-global `---@class`/`---@enum` declarations (luals
/// parity: classes declared in any checked file, including their
/// `function Class:method` member attachments, resolve from every other
/// file). The file's own requires are not followed, so the cross-file
/// graph stays acyclic even when modules require each other. Depends only
/// on the file's parse (and dialect), not on `Project`.
#[salsa::tracked]
pub fn module_surface_checked(db: &dyn Db, file: SourceFile) -> ModuleSurfaceChecked {
    db.push_log(format!("module_surface_checked({})", display(db, file)));
    let parsed = parse(db, file);
    let name = display(db, file);
    let ambient = stdlib_defs(file.dialect(db));
    ModuleSurfaceChecked::new(luabox_types::module_surface(
        parsed.parse(),
        &name,
        Some(ambient),
    ))
}

/// Module string → **check-mode** export type, for every static `require`
/// in `file` that resolves to another project file — the registry
/// [`luabox_types::check_file_with_requires`] threads into checking so a
/// consumer types its `require` results from the module's annotations (#85).
pub(crate) fn require_exports_checked(
    db: &dyn Db,
    file: SourceFile,
    project: Project,
) -> HashMap<String, Ty> {
    let mut map = HashMap::new();
    for edge in lower(db, file).file().requires() {
        if let Some(target) = resolve_require(db, project, &edge.module)
            && target != file
            && let Some(ty) = module_surface_checked(db, target).export()
        {
            map.insert(edge.module.clone(), ty.clone());
        }
    }
    map
}

/// Every project file's workspace-global class/enum contribution — the
/// input [`luabox_types::Ambient::with_project_types`] merges beneath each
/// file's own declarations so a class declared anywhere in the project
/// resolves everywhere (luals parity, #85).
pub(crate) fn project_types_checked(db: &dyn Db, project: Project) -> Vec<luabox_types::FileTypes> {
    project
        .files(db)
        .iter()
        .map(|&file| module_surface_checked(db, file).types().clone())
        .filter(|types| !types.is_empty())
        .collect()
}

/// Module string → export type, for every static `require` in `file` that
/// resolves to a project file.
fn require_exports(db: &dyn Db, file: SourceFile, project: Project) -> HashMap<String, Ty> {
    let mut map = HashMap::new();
    for edge in lower(db, file).file().requires() {
        if let Some(target) = resolve_require(db, project, &edge.module)
            && target != file
            && let Some(ty) = module_export(db, target, project).ty()
        {
            map.insert(edge.module.clone(), ty.clone());
        }
    }
    map
}

/// Parameter seeds for `file`'s exported functions: the positional
/// argument-type unions observed in every project file that requires it.
fn dependent_seeds(db: &dyn Db, file: SourceFile, project: Project) -> HashMap<String, Vec<Ty>> {
    let mut seeds: HashMap<String, Vec<Ty>> = HashMap::new();
    for &other in project.files(db) {
        if other == file {
            continue;
        }
        let requires_me = lower(db, other)
            .file()
            .requires()
            .iter()
            .any(|edge| resolve_require(db, project, &edge.module) == Some(file));
        if !requires_me {
            continue;
        }
        for (name, tys) in outgoing_calls(db, other).calls() {
            let entry = seeds.entry(name.clone()).or_default();
            for (i, ty) in tys.iter().enumerate() {
                if matches!(ty, Ty::Unknown) {
                    continue;
                }
                while entry.len() <= i {
                    entry.push(Ty::Unknown);
                }
                entry[i] = if matches!(entry[i], Ty::Unknown) {
                    ty.clone()
                } else {
                    Ty::union(vec![entry[i].clone(), ty.clone()])
                };
            }
        }
    }
    seeds
}

/// Resolve a `require` module string to a project file using the bundler's
/// exact SPEC.md §7 path-mapping ([`luabox_bundle::resolve_candidates`]) over
/// the in-memory project file set — the *same* resolution ordering
/// `luabox check` applies on disk, so the editor and CI never disagree on which
/// file `require("a.b")` names. `Project::root` anchors the candidate paths
/// (`<root>/a/b.lua`, `<root>/src/a/b.lua`, …); the first candidate that is a
/// known project file wins. Path equality is component-wise, so `/` vs `\`
/// separators are irrelevant.
fn resolve_require(db: &dyn Db, project: Project, module: &str) -> Option<SourceFile> {
    let root = project.root(db);
    let files = project.files(db);
    luabox_bundle::resolve_candidates(root, module)
        .into_iter()
        .find_map(|cand| {
            files
                .iter()
                .copied()
                .find(|file| file.path(db).as_path() == cand.as_path())
        })
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
    Diagnostics::new(check_file(
        parsed.parse(),
        &name,
        project.strictness(db),
        file.dialect(db),
    ))
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
