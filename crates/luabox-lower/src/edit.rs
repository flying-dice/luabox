//! Targeted text edits over the lossless tree (rust-analyzer style).
//!
//! Rules never reprint the file: they produce ranged replacements and
//! point insertions, and everything outside those ranges is preserved
//! byte-for-byte. Insertions at the same offset apply in push order
//! (`seq`), which the rules use to stack nested wrappers correctly (e.g.
//! two `<close>` scope-exit `end)` lines at the same block end).

use rowan::{TextRange, TextSize};

/// One edit: replace `range` (possibly empty — an insertion) with `text`.
pub(crate) struct Edit {
    pub range: TextRange,
    pub text: String,
    /// Push-order tiebreak for same-position edits.
    seq: usize,
}

/// Append an edit, assigning its push-order sequence number.
pub(crate) fn push(edits: &mut Vec<Edit>, range: TextRange, text: String) {
    let seq = edits.len();
    edits.push(Edit { range, text, seq });
}

/// An insertion at `offset`.
pub(crate) fn insert(edits: &mut Vec<Edit>, offset: TextSize, text: String) {
    push(edits, TextRange::empty(offset), text);
}

/// Apply the edits to `source`. Replacement ranges must be disjoint (the
/// rules guarantee this; violations are a lowering bug and are dropped in
/// release builds after a debug assertion).
#[expect(
    clippy::string_slice,
    reason = "edit offsets are TextRange byte positions from the syntax tree over `source`, so they always fall on char boundaries and within bounds; overlaps are dropped above"
)]
pub(crate) fn apply(source: &str, mut edits: Vec<Edit>) -> String {
    edits.sort_by_key(|e| (e.range.start(), e.range.end(), e.seq));
    let mut out = String::with_capacity(source.len());
    let mut last = 0usize;
    for edit in &edits {
        let start = usize::from(edit.range.start());
        let end = usize::from(edit.range.end());
        debug_assert!(start >= last, "overlapping lowering edits");
        if start < last {
            continue; // release-mode safety: drop the overlapping edit
        }
        out.push_str(&source[last..start]);
        out.push_str(&edit.text);
        last = end;
    }
    out.push_str(&source[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u32, end: u32) -> TextRange {
        TextRange::new(start.into(), end.into())
    }

    #[test]
    fn replacements_and_insertions_apply_in_order() {
        let mut edits = Vec::new();
        push(&mut edits, range(4, 7), "yyy".to_owned());
        insert(&mut edits, 0.into(), "A".to_owned());
        assert_eq!(apply("abc xxx def", edits), "Aabc yyy def");
    }

    #[test]
    fn same_offset_insertions_keep_push_order() {
        let mut edits = Vec::new();
        insert(&mut edits, 3.into(), "1".to_owned());
        insert(&mut edits, 3.into(), "2".to_owned());
        assert_eq!(apply("abcdef", edits), "abc12def");
    }

    #[test]
    fn no_edits_is_identity() {
        assert_eq!(apply("x = 1\n", Vec::new()), "x = 1\n");
    }
}
