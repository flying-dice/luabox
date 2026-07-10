//! `.luab` shape support (SHAPES-V2.md): loading, lowering into the unified
//! IR, and the ambient package scope.
//!
//! Two front-ends, one IR (invariant 2 survives v2): shape declarations lower
//! into the same [`crate::ty::Ty`] the LuaCATS front-end produces — object
//! types become *sealed* structural tables, intersections merge, aliases
//! expand. Interop is total: a `.luab` type is usable from any standard
//! annotation position (`---@type` / `---@param` / `---@return` /
//! `---@field`), and there are **no** shape-specific tags.
//!
//! Conformance is positional and structural: a value is a `geometry.Shape`
//! exactly where one is demanded. The v1 binding tags (`---@use`,
//! `---@struct`, `---@impl`), the conformance registry, and the supertrait
//! pass are deleted, not replaced.

pub(crate) mod raw;
mod scope;
mod store;

pub use scope::{ShapeScope, TypeShape};
pub use store::{DepShapeExport, ShapeStore};

use std::path::PathBuf;

/// Where a package's ambient type scope loads from.
#[derive(Debug, Clone)]
pub struct ShapeOptions<'a> {
    /// The shared module store (parse cache + loading).
    pub store: &'a ShapeStore,
    /// `[types] shape-paths` directories, absolute, in manifest order.
    pub shape_paths: &'a [PathBuf],
    /// Dependencies that may export type surfaces (`[types] entry` in their
    /// own manifests). Empty when the project has no type-exporting
    /// dependencies; the store never reads dependency manifests itself —
    /// the CLI builds this list.
    pub dependencies: &'a [DepShapeExport],
}

impl ShapeOptions<'_> {
    /// Build (or fetch) the ambient package scope.
    #[must_use]
    pub fn scope(&self) -> std::sync::Arc<ShapeScope> {
        self.store
            .package_scope(self.shape_paths, self.dependencies)
    }
}
