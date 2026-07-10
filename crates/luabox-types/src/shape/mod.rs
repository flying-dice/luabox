//! `.luab` shape support (SHAPES.md): loading, lowering into the unified IR,
//! `---@use` resolution, and the `LB2xxx` binding checks.
//!
//! Two front-ends, one IR (SHAPES.md invariant 2): shape declarations lower
//! into the same [`crate::ty::Ty`] the LuaCATS front-end produces — structs
//! become *sealed* structural tables, traits become method-set interfaces,
//! aliases expand. Interop is total: a `.luab` struct is usable from
//! `---@param`/`---@field`, and a `---@class` table can satisfy a `.luab`
//! trait via `---@impl`.

mod check;
pub(crate) mod raw;
mod scope;
mod store;

pub use scope::{AliasShape, GenericParamDef, ShapeScope, StructShape, TraitFnSig, TraitShape};
pub use store::{DepShapeExport, ShapeStore};

use std::path::{Path, PathBuf};

use luabox_diag::Diagnostic;
use luabox_syntax::lua;
use luabox_syntax::luacats::{AnnotatedItem, Tag};

use crate::Strictness;
use crate::env::TypeEnv;

/// Where a `.lua` file's shape imports resolve from.
#[derive(Debug, Clone, Copy)]
pub struct ShapeOptions<'a> {
    /// The shared module store (parse cache + resolution).
    pub store: &'a ShapeStore,
    /// The directory of the `.lua` file being checked (sibling tier).
    pub file_dir: &'a Path,
    /// `[types] shape-paths` directories, absolute, in manifest order.
    pub shape_paths: &'a [PathBuf],
    /// Dependencies that may export shape modules (SHAPES.md §6, tier 3).
    /// Empty when the project has no shape-exporting dependencies; the store
    /// never reads dependency manifests itself — the CLI builds this list.
    pub dependencies: &'a [DepShapeExport],
}

/// Whether the harvested annotations use any shape binding tag at all —
/// the zero-cost guarantee (SHAPES.md invariant 4): files without tags
/// never touch the shape machinery.
pub(crate) fn uses_shapes(items: &[AnnotatedItem]) -> bool {
    items.iter().any(|item| {
        item.block
            .tags
            .iter()
            .any(|tag| matches!(tag, Tag::Use(_) | Tag::Struct(_) | Tag::Impl(_)))
    })
}

/// Resolve every `---@use` in the file and build the merged scope.
/// Unresolved or ambiguous imports become `LB2005` diagnostics at the tag.
pub(crate) fn resolve_uses(
    items: &[AnnotatedItem],
    opts: &ShapeOptions<'_>,
    file: &str,
) -> (ShapeScope, Vec<Diagnostic>) {
    let mut diags = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    for item in items {
        for tag in &item.block.tags {
            let Tag::Use(use_tag) = tag else { continue };
            if use_tag.module.is_empty() {
                continue;
            }
            let outcome = store::resolve(
                &use_tag.module,
                opts.file_dir,
                opts.shape_paths,
                opts.dependencies,
            );
            match outcome {
                store::ResolveOutcome::Found(path) => roots.push(path),
                other => diags.push(opts.store.unresolved_use(
                    &use_tag.module,
                    file,
                    use_tag.span.start..use_tag.span.end,
                    &other,
                )),
            }
        }
    }
    let scope = opts
        .store
        .scope_from(&roots, opts.shape_paths, opts.dependencies);
    (scope, diags)
}

/// Run the shape binding checks for one `.lua` file (hard errors at every
/// strictness — `---@struct` is the opt-in). `inferred` carries the
/// inference engine's expression types keyed by byte range, so annotated
/// parameter values (`{ radius = radius }` in a constructor) type-check
/// instead of degrading to `unknown` (#73).
pub(crate) fn check_bindings(
    parse: &lua::Parse,
    items: &[AnnotatedItem],
    scope: &ShapeScope,
    env: &TypeEnv,
    file: &str,
    strictness: Strictness,
    inferred: &std::collections::HashMap<(usize, usize), crate::ty::Ty>,
) -> Vec<Diagnostic> {
    check::run(parse, items, scope, env, file, strictness, inferred)
}
