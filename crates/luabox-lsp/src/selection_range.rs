//! Selection ranges: the syntax-tree expand chain from the token at a
//! position out through each ancestor node, one [`SelectionRange`] per
//! requested position (LSP requires the response array to line up
//! positionally with the request).

use lsp_types::{Position, SelectionRange};
use luabox_syntax::lua::{SyntaxNode, SyntaxToken};
use rowan::{TextRange, TextSize, TokenAtOffset};

use crate::sema::FileSema;

/// The expand chain for each requested position, in the same order.
#[must_use]
pub fn selection_ranges(sema: &FileSema, positions: &[Position]) -> Vec<SelectionRange> {
    positions
        .iter()
        .map(|&pos| selection_range_at(sema, pos))
        .collect()
}

/// The nested expand chain for one position: the innermost token, then every
/// enclosing node's range outward to the source file, skipping any ancestor
/// whose range coincides with the one just emitted (a node that wraps a
/// single child with no extra syntax of its own contributes nothing new).
fn selection_range_at(sema: &FileSema, position: Position) -> SelectionRange {
    let offset = sema.index.offset(position);
    let leaf_range = |r: std::ops::Range<usize>| SelectionRange {
        range: sema.index.range(r),
        parent: None,
    };
    let Some(offset) = u32::try_from(offset).ok().map(TextSize::new) else {
        return leaf_range(offset..offset);
    };
    let Some(token) = token_at(&sema.root, offset) else {
        let at = usize::from(offset);
        return leaf_range(at..at);
    };

    let mut ranges: Vec<TextRange> = vec![token.text_range()];
    if let Some(parent) = token.parent() {
        for ancestor in parent.ancestors() {
            let r = ancestor.text_range();
            if ranges.last() != Some(&r) {
                ranges.push(r);
            }
        }
    }
    build_chain(sema, &ranges)
}

/// The innermost token at `offset`: at a boundary between two tokens, the
/// non-trivia one (whitespace never carries a meaningful selection).
fn token_at(root: &SyntaxNode, offset: TextSize) -> Option<SyntaxToken> {
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(l, r) => Some(if r.kind().is_trivia() { l } else { r }),
    }
}

/// Nest `ranges` (innermost first) into a nested [`SelectionRange`], each
/// wrapping the next as its `parent`.
fn build_chain(sema: &FileSema, ranges: &[TextRange]) -> SelectionRange {
    let mut node: Option<SelectionRange> = None;
    for r in ranges.iter().rev() {
        node = Some(SelectionRange {
            range: sema
                .index
                .range(usize::from(r.start())..usize::from(r.end())),
            parent: node.map(Box::new),
        });
    }
    node.expect("ranges always holds at least the token's own range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    fn analyze(text: &str) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let path = Path::new(if cfg!(windows) {
            r"C:\ws\main.lua"
        } else {
            "/ws/main.lua"
        })
        .to_path_buf();
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: text.to_string(),
        });
        (host.snapshot(), path)
    }

    /// Flatten a chain into its ranges, innermost first.
    fn chain(range: &SelectionRange) -> Vec<lsp_types::Range> {
        let mut out = vec![range.range];
        let mut current = &range.parent;
        while let Some(parent) = current {
            out.push(parent.range);
            current = &parent.parent;
        }
        out
    }

    #[test]
    fn expands_from_token_through_ancestors_to_the_file() {
        let src = "local x = 1 + 2\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // Cursor inside the `1` literal (columns 10..11).
        let result = selection_ranges(&sema, &[Position::new(0, 10)]);
        assert_eq!(result.len(), 1);
        let ranges = chain(&result[0]);
        // Innermost: the `1` token itself.
        assert_eq!(
            ranges[0],
            lsp_types::Range {
                start: Position::new(0, 10),
                end: Position::new(0, 11),
            }
        );
        // Outermost: the whole file.
        let last = *ranges.last().unwrap();
        assert_eq!(last.start, Position::new(0, 0));
        assert_eq!(last.end, Position::new(1, 0));
        // Strictly widening: each range contains the previous one.
        for pair in ranges.windows(2) {
            let (inner, outer) = (pair[0], pair[1]);
            assert!(
                outer.start <= inner.start && inner.end <= outer.end,
                "{ranges:?}"
            );
            assert!(outer != inner, "{ranges:?}");
        }
        // The binary expression `1 + 2` shows up somewhere in the middle.
        assert!(
            ranges.contains(&lsp_types::Range {
                start: Position::new(0, 10),
                end: Position::new(0, 15),
            }),
            "{ranges:?}"
        );
    }

    #[test]
    fn one_range_per_requested_position_in_order() {
        let src = "local a = 1\nlocal b = 2\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let result = selection_ranges(&sema, &[Position::new(0, 6), Position::new(1, 6)]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].range.start, Position::new(0, 6));
        assert_eq!(result[1].range.start, Position::new(1, 6));
    }
}
