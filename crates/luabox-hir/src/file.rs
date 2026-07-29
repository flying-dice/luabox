//! [`LoweredFile`] — the boundary type of the Semantics context: the result
//! of lowering one parsed Lua file.
//!
//! Consumers (`luabox-types`, `luabox-lower`, `luabox-bundle`) get the chunk
//! body, all function bodies, the resolution table, the source map, and the
//! `require` graph — and never see token-level details.

use std::collections::HashMap;

use crate::arena::Arena;
use crate::hir::{
    Binding, BindingId, Body, BodyId, DynamicRequire, HirId, Label, LabelId, RequireEdge,
    Resolution,
};
use crate::source_map::SourceMap;

/// The lowered form of one Lua source file.
#[derive(Debug, Clone)]
pub struct LoweredFile {
    bodies: Arena<Body>,
    bindings: Arena<Binding>,
    labels: Arena<Label>,
    chunk: BodyId,
    source_map: SourceMap,
    resolutions: HashMap<HirId, Resolution>,
    goto_names: HashMap<HirId, rowan::TextRange>,
    requires: Vec<RequireEdge>,
    dynamic_requires: Vec<DynamicRequire>,
}

impl LoweredFile {
    #[allow(clippy::too_many_arguments, reason = "internal constructor")]
    pub(crate) fn new(
        bodies: Arena<Body>,
        bindings: Arena<Binding>,
        labels: Arena<Label>,
        chunk: BodyId,
        source_map: SourceMap,
        resolutions: HashMap<HirId, Resolution>,
        goto_names: HashMap<HirId, rowan::TextRange>,
        requires: Vec<RequireEdge>,
        dynamic_requires: Vec<DynamicRequire>,
    ) -> Self {
        Self {
            bodies,
            bindings,
            labels,
            chunk,
            source_map,
            resolutions,
            goto_names,
            requires,
            dynamic_requires,
        }
    }

    /// The top-level chunk's body.
    pub fn chunk(&self) -> BodyId {
        self.chunk
    }

    pub fn body(&self, id: BodyId) -> &Body {
        &self.bodies[id]
    }

    /// All bodies (the chunk plus one per function), with their handles.
    pub fn bodies(&self) -> impl Iterator<Item = (BodyId, &Body)> {
        self.bodies.iter()
    }

    pub fn binding(&self, id: BindingId) -> &Binding {
        &self.bindings[id]
    }

    /// All value bindings in the file.
    pub fn bindings(&self) -> impl Iterator<Item = (BindingId, &Binding)> {
        self.bindings.iter()
    }

    pub fn label(&self, id: LabelId) -> &Label {
        &self.labels[id]
    }

    /// The resolution of a name expression (keyed by its [`HirId`]).
    /// `None` for ids that are not name expressions.
    pub fn resolution(&self, id: HirId) -> Option<&Resolution> {
        self.resolutions.get(&id)
    }

    /// The `HirId -> TextRange` back-reference table.
    pub fn source_map(&self) -> &SourceMap {
        &self.source_map
    }

    /// The range of a `goto`'s *name token*, keyed by the goto statement's
    /// [`HirId`]; `None` for ids that are not `goto` statements.
    ///
    /// The source map holds the whole `goto name` statement; this narrower
    /// range is what an unresolved-label diagnostic underlines.
    pub fn goto_name_range(&self, id: HirId) -> Option<rowan::TextRange> {
        self.goto_names.get(&id).copied()
    }

    /// Static `require("...")` edges, in source order.
    pub fn requires(&self) -> &[RequireEdge] {
        &self.requires
    }

    /// `require(<non-literal>)` call sites, in source order.
    pub fn dynamic_requires(&self) -> &[DynamicRequire] {
        &self.dynamic_requires
    }
}
