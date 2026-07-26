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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;
    use crate::arena::Idx;
    use crate::hir::Expr;

    #[test]
    fn a_default_source_map_is_empty_and_maps_nothing() {
        let map = SourceMap::default();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(
            map.range(HirId::expr(Idx::from_raw(0), Idx::from_raw(0))),
            None
        );
    }

    #[test]
    fn insert_is_last_write_wins_and_len_counts_distinct_ids() {
        let mut map = SourceMap::default();
        let id = HirId::expr(Idx::from_raw(0), Idx::from_raw(0));
        let other = HirId::expr(Idx::from_raw(0), Idx::from_raw(1));

        map.insert(id, TextRange::new(0.into(), 3.into()));
        assert!(!map.is_empty());
        assert_eq!(map.len(), 1);

        map.insert(id, TextRange::new(4.into(), 9.into()));
        assert_eq!(
            map.len(),
            1,
            "re-inserting the same id replaces, not appends"
        );
        assert_eq!(map.range(id), Some(TextRange::new(4.into(), 9.into())));

        // A stmt id with the same raw index is a distinct key from an expr id.
        map.insert(
            HirId::stmt(Idx::from_raw(0), Idx::from_raw(0)),
            TextRange::new(0.into(), 2.into()),
        );
        map.insert(other, TextRange::new(0.into(), 1.into()));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn lowering_maps_every_node_back_to_the_text_it_came_from() {
        let src = "local x = 1\nreturn x + 2\n";
        let file = crate::lower(&parse(src, Dialect::Lua54));
        let map = file.source_map();

        assert!(!map.is_empty());

        let mut nodes = 0;
        for (body_id, body) in file.bodies() {
            for (expr_id, expr) in body.exprs() {
                nodes += 1;
                let range = map
                    .range(HirId::expr(body_id, expr_id))
                    .expect("every lowered expr is mapped");
                assert!(usize::from(range.end()) <= src.len());
                if let Expr::Name(name) = expr {
                    assert_eq!(&src[range], name.as_str());
                }
            }
            for (stmt_id, _) in body.stmts() {
                nodes += 1;
                assert!(
                    map.range(HirId::stmt(body_id, stmt_id)).is_some(),
                    "every lowered stmt is mapped"
                );
            }
        }
        assert!(nodes > 0);
        assert_eq!(map.len(), nodes);
    }
}
