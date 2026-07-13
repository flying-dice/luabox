//! Document highlight: every occurrence of the symbol under the cursor,
//! narrowed to the current file — [`references`] restricted to one file's
//! locations, since locals are already file-scoped and globals/members only
//! need the URI filter applied. Each occurrence is tagged [`DocumentHighlightKind::Write`]
//! when its identifier is an assignment target (the LHS of an `AssignStmt`,
//! or a `local` binding's own declaration) and [`DocumentHighlightKind::Read`]
//! otherwise — including anything we can't cheaply classify, such as a
//! `---@field` declaration span with no token at its start.

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Range};
use luabox_db::Analysis;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxNode, SyntaxToken};

use crate::references::references;
use crate::sema::FileSema;
use crate::uri::path_to_uri;

/// Every occurrence of the symbol at `offset`, restricted to `target`'s own
/// file, each tagged as a read or a write.
#[must_use]
pub fn document_highlight(
    analysis: &Analysis,
    target: &FileSema,
    offset: usize,
) -> Option<Vec<DocumentHighlight>> {
    let uri = path_to_uri(&target.path);
    let mut out: Vec<DocumentHighlight> = references(analysis, target, offset, true)?
        .into_iter()
        .filter(|loc| loc.uri.as_str() == uri.as_str())
        .map(|loc| DocumentHighlight {
            range: loc.range,
            kind: Some(classify(target, loc.range)),
        })
        .collect();
    out.sort_by_key(|h| (h.range.start.line, h.range.start.character));
    Some(out)
}

/// Read vs write for one occurrence, re-deriving the identifier token from
/// its range (the location only carries a byte-position range, not the
/// token that produced it).
fn classify(sema: &FileSema, range: Range) -> DocumentHighlightKind {
    let offset = sema.index.offset(range.start);
    match sema.ident_at(offset) {
        Some(token) if is_write(&token) => DocumentHighlightKind::WRITE,
        _ => DocumentHighlightKind::READ,
    }
}

/// Whether `token`'s identifier is a write: the name of a `local` binding
/// declaration, or an entry in an `AssignStmt`'s target list.
fn is_write(token: &SyntaxToken) -> bool {
    let Some(parent) = token.parent() else {
        return false;
    };
    parent.kind() == SyntaxKind::LOCAL_NAME || is_assign_target(&parent)
}

/// Whether `expr_node` — the `NAME_EXPR`/`FIELD_EXPR` naming the occurrence —
/// is directly one of an `AssignStmt`'s targets, not merely part of the
/// assigned value (or nested inside a compound target expression).
fn is_assign_target(expr_node: &SyntaxNode) -> bool {
    let Some(list) = expr_node
        .parent()
        .filter(|p| p.kind() == SyntaxKind::EXPR_LIST)
    else {
        return false;
    };
    let Some(assign) = list.parent().and_then(ast::AssignStmt::cast) else {
        return false;
    };
    assign.targets().is_some_and(|t| t.syntax() == &list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut first = None;
        for (rel, text) in files {
            let path = root.join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    #[test]
    fn local_write_then_reassign_then_read() {
        let src = "local x = 1\nx = 2\nprint(x)\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).unwrap();
        // Cursor on the declaration.
        let offset = offset_of(src, "x", 0);
        let hits = document_highlight(&analysis, &sema, offset).expect("highlights");
        assert_eq!(hits.len(), 3, "{hits:?}");
        let kind_at = |line: u32| {
            hits.iter()
                .find(|h| h.range.start.line == line)
                .unwrap_or_else(|| panic!("no highlight on line {line}: {hits:?}"))
                .kind
        };
        assert_eq!(kind_at(0), Some(DocumentHighlightKind::WRITE)); // declaration
        assert_eq!(kind_at(1), Some(DocumentHighlightKind::WRITE)); // reassignment
        assert_eq!(kind_at(2), Some(DocumentHighlightKind::READ)); // print(x)
    }

    #[test]
    fn member_field_write_on_assignment_target() {
        let src = "local t = {}\nt.x = 1\nprint(t.x)\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).unwrap();
        // Cursor on the `x` of `t.x = 1`.
        let offset = offset_of(src, ".x", 0) + 1;
        let hits = document_highlight(&analysis, &sema, offset).expect("highlights");
        assert_eq!(hits.len(), 2, "{hits:?}");
        let assign = hits.iter().find(|h| h.range.start.line == 1).unwrap();
        assert_eq!(assign.kind, Some(DocumentHighlightKind::WRITE));
        let read = hits.iter().find(|h| h.range.start.line == 2).unwrap();
        assert_eq!(read.kind, Some(DocumentHighlightKind::READ));
    }

    #[test]
    fn highlight_is_scoped_to_the_current_file() {
        let files = &[
            ("a.lua", "function greet() return 1 end\n"),
            ("b.lua", "greet()\ngreet()\n"),
        ];
        let (analysis, _) = analyze(files);
        let b = analysis
            .files()
            .find(|p| p.ends_with("b.lua"))
            .unwrap()
            .to_path_buf();
        let sema = FileSema::new(&analysis, &b).unwrap();
        let offset = offset_of("greet()\ngreet()\n", "greet", 0);
        let hits = document_highlight(&analysis, &sema, offset).expect("highlights");
        // Two calls in b.lua; the declaration in a.lua is excluded.
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(
            hits.iter()
                .all(|h| h.kind == Some(DocumentHighlightKind::READ))
        );
    }
}
