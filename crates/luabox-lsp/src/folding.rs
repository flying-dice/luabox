//! Folding ranges: pure syntax-tree geometry, no semantic analysis needed.
//!
//! Foldable code spans are the block-bearing statement/expression nodes
//! (`do`/`while`/`repeat`/`if`+`elseif`+`else`/`for`/function bodies) and
//! table constructors — each folds over its own full node range (opening
//! keyword/brace through the matching close), the same "whole node span"
//! convention [`crate::symbols`] already uses for a symbol's `range`.
//!
//! Comments fold too: a single long-bracket comment (`--[[ ... ]]`) that
//! spans multiple lines is one region, and a run of adjacent `--` line
//! comments (a doc-comment block, or an ordinary multi-line comment written
//! as consecutive line comments — Lua has no other multi-line line-comment
//! syntax) folds as one region together.

use lsp_types::{FoldingRange, FoldingRangeKind};
use luabox_syntax::lua::{SyntaxKind, SyntaxToken};
use rowan::TextRange;

use crate::sema::FileSema;

/// Every foldable region in the file: code blocks/tables first, then
/// comment runs, ordered by start then end line.
#[must_use]
pub fn folding_ranges(sema: &FileSema) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    for node in sema.root.descendants() {
        if is_foldable_block(node.kind()) {
            push_range(sema, node.text_range(), None, &mut out);
        }
    }
    push_comment_folds(sema, &mut out);
    out.sort_by_key(|r| (r.start_line, r.end_line));
    out
}

/// Node kinds whose full span is a foldable code region.
fn is_foldable_block(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::DO_STMT
            | SyntaxKind::WHILE_STMT
            | SyntaxKind::REPEAT_STMT
            | SyntaxKind::IF_STMT
            | SyntaxKind::ELSEIF_CLAUSE
            | SyntaxKind::ELSE_CLAUSE
            | SyntaxKind::NUMERIC_FOR_STMT
            | SyntaxKind::GENERIC_FOR_STMT
            | SyntaxKind::FUNCTION_DECL_STMT
            | SyntaxKind::FUNCTION_EXPR
            | SyntaxKind::LOCAL_FUNCTION_STMT
            | SyntaxKind::TABLE_EXPR
    )
}

/// Comment folds: one long-bracket comment token spanning multiple lines is
/// its own region; a run of two or more adjacent single-line `--` comments
/// (no blank line between them) folds as one region over the whole run.
fn push_comment_folds(sema: &FileSema, out: &mut Vec<FoldingRange>) {
    let tokens: Vec<SyntaxToken> = sema
        .root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::COMMENT)
        .collect();

    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].text().contains('\n') {
            push_range(
                sema,
                tokens[i].text_range(),
                Some(FoldingRangeKind::Comment),
                out,
            );
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut last = &tokens[i];
        while j < tokens.len()
            && !tokens[j].text().contains('\n')
            && adjacent(sema, last, &tokens[j])
        {
            last = &tokens[j];
            j += 1;
        }
        if j - i > 1 {
            let range = TextRange::new(tokens[i].text_range().start(), last.text_range().end());
            push_range(sema, range, Some(FoldingRangeKind::Comment), out);
        }
        i = j;
    }
}

/// Whether `next` starts on the line immediately after `prev` ends — the
/// "no blank line between them" test for grouping a run of line comments.
fn adjacent(sema: &FileSema, prev: &SyntaxToken, next: &SyntaxToken) -> bool {
    let prev_line = sema
        .index
        .position(usize::from(prev.text_range().end()))
        .line;
    let next_line = sema
        .index
        .position(usize::from(next.text_range().start()))
        .line;
    next_line == prev_line + 1
}

/// Push a fold for `range` if it spans more than one line.
fn push_range(
    sema: &FileSema,
    range: TextRange,
    kind: Option<FoldingRangeKind>,
    out: &mut Vec<FoldingRange>,
) {
    let start_line = sema.index.position(usize::from(range.start())).line;
    let end_line = sema.index.position(usize::from(range.end())).line;
    if end_line > start_line {
        out.push(FoldingRange {
            start_line,
            start_character: None,
            end_line,
            end_character: None,
            kind,
            collapsed_text: None,
        });
    }
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

    fn folds(text: &str) -> Vec<FoldingRange> {
        let (analysis, path) = analyze(text);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        folding_ranges(&sema)
    }

    #[test]
    fn function_body_folds() {
        let src = "local function f()\n  return 1\nend\n";
        let ranges = folds(src);
        assert!(
            ranges
                .iter()
                .any(|r| r.start_line == 0 && r.end_line == 2 && r.kind.is_none()),
            "{ranges:?}"
        );
    }

    #[test]
    fn if_elseif_else_fold_as_separate_regions() {
        let src = "if a then\n  f()\nelseif b then\n  g()\nelse\n  h()\nend\n";
        let ranges = folds(src);
        // The whole `if`, plus its `elseif` and `else` clauses, each fold.
        assert!(ranges.iter().any(|r| r.start_line == 0 && r.end_line == 6));
        assert!(ranges.iter().any(|r| r.start_line == 2 && r.end_line == 3));
        assert!(ranges.iter().any(|r| r.start_line == 4 && r.end_line == 5));
    }

    #[test]
    fn multiline_table_constructor_folds() {
        let src = "local t = {\n  1,\n  2,\n}\n";
        let ranges = folds(src);
        assert!(
            ranges.iter().any(|r| r.start_line == 0 && r.end_line == 3),
            "{ranges:?}"
        );
    }

    #[test]
    fn single_line_table_does_not_fold() {
        let src = "local t = { 1, 2 }\n";
        let ranges = folds(src);
        assert!(ranges.is_empty(), "{ranges:?}");
    }

    #[test]
    fn long_bracket_comment_folds() {
        let src = "--[[\nhello\nworld\n]]\nlocal x = 1\n";
        let ranges = folds(src);
        let comment = ranges
            .iter()
            .find(|r| r.kind == Some(FoldingRangeKind::Comment))
            .expect("comment fold");
        assert_eq!((comment.start_line, comment.end_line), (0, 3));
    }

    #[test]
    fn adjacent_line_comments_fold_together() {
        let src = "---@class Point\n---@field x number\n---@field y number\nlocal x\n";
        let ranges = folds(src);
        let comment = ranges
            .iter()
            .find(|r| r.kind == Some(FoldingRangeKind::Comment))
            .expect("comment fold");
        assert_eq!((comment.start_line, comment.end_line), (0, 2));
    }

    #[test]
    fn lone_line_comment_does_not_fold() {
        let src = "-- just one line\nlocal x = 1\n";
        let ranges = folds(src);
        assert!(ranges.is_empty(), "{ranges:?}");
    }

    #[test]
    fn separated_comment_runs_stay_separate() {
        let src = "-- a\n-- b\n\n-- c\n-- d\n";
        let ranges = folds(src);
        let comments: Vec<_> = ranges
            .iter()
            .filter(|r| r.kind == Some(FoldingRangeKind::Comment))
            .collect();
        assert_eq!(comments.len(), 2, "{ranges:?}");
        assert!(
            comments
                .iter()
                .any(|r| (r.start_line, r.end_line) == (0, 1))
        );
        assert!(
            comments
                .iter()
                .any(|r| (r.start_line, r.end_line) == (3, 4))
        );
    }
}
