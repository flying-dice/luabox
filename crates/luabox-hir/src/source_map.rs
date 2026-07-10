//! The [`SourceMap`]: the side table tying each HIR node back to the syntax
//! range it was lowered from.
//!
//! Ranges live *here*, never on the HIR nodes themselves, so the HIR stays a
//! compact, position-free graph while diagnostics, the LSP, and lowering can
//! still recover the exact source span of any [`HirId`].

use std::collections::HashMap;

use rowan::TextRange;

use crate::hir::HirId;

/// `HirId -> TextRange` back-references for the whole file.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    ranges: HashMap<HirId, TextRange>,
}

impl SourceMap {
    pub(crate) fn insert(&mut self, id: HirId, range: TextRange) {
        self.ranges.insert(id, range);
    }

    /// The syntax range a HIR node was lowered from, if recorded.
    ///
    /// Synthesized nodes map to the most relevant token (e.g. a desugared
    /// field key maps to the field-name token).
    pub fn range(&self, id: HirId) -> Option<TextRange> {
        self.ranges.get(&id).copied()
    }

    /// Number of mapped nodes.
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}
