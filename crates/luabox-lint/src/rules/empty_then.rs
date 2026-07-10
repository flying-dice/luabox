//! `empty-then` (suspicious): an `if`/`elseif` branch with an empty,
//! comment-free body.

use std::ops::Range;

use luabox_diag::Code;
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::context::LintContext;
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// An `if cond then end` (or `elseif`) whose body is empty and carries no
/// comment (SPEC.md §9). A single explanatory comment documents intent and
/// suppresses the rule. Works over the lossless tree, since an empty block
/// leaves no HIR statement to hang a span on.
pub struct EmptyThen;

impl Rule for EmptyThen {
    fn id(&self) -> &'static str {
        "empty-then"
    }

    fn tier(&self) -> Tier {
        Tier::Suspicious
    }

    fn code(&self) -> Code {
        Code::new(508)
    }

    fn description(&self) -> &'static str {
        "an `if ... then` branch has an empty body"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut out = Vec::new();
        for node in ctx.parse.syntax().descendants() {
            match node.kind() {
                SyntaxKind::IF_STMT => {
                    if let Some(diag) = check_if(&node, ctx.source) {
                        out.push(diag);
                    }
                }
                SyntaxKind::ELSEIF_CLAUSE => {
                    if let Some(diag) = check_elseif(&node, ctx.source) {
                        out.push(diag);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// The primary `if ... then <body>` branch. Its body region runs from the
/// first `then` to the first `elseif`/`else`/`end`.
fn check_if(node: &SyntaxNode, source: &str) -> Option<LintDiagnostic> {
    let mut then_kw: Option<Range<usize>> = None;
    let mut boundary: Option<usize> = None;
    for child in node.children_with_tokens() {
        let kind = child.kind();
        if then_kw.is_none() && kind == SyntaxKind::THEN_KW {
            then_kw = Some(to_range(child.text_range()));
            continue;
        }
        if then_kw.is_some()
            && matches!(
                kind,
                SyntaxKind::ELSEIF_CLAUSE | SyntaxKind::ELSE_CLAUSE | SyntaxKind::END_KW
            )
        {
            boundary = Some(usize::from(child.text_range().start()));
            break;
        }
    }
    emit_if_empty(then_kw?, boundary?, source)
}

/// An `elseif cond then <body>` clause: its body runs from its `then` to the
/// end of the clause node.
fn check_elseif(node: &SyntaxNode, source: &str) -> Option<LintDiagnostic> {
    let then_kw = node
        .children_with_tokens()
        .find(|c| c.kind() == SyntaxKind::THEN_KW)
        .map(|c| to_range(c.text_range()))?;
    let boundary = usize::from(node.text_range().end());
    emit_if_empty(then_kw, boundary, source)
}

/// Emit when the region after `then` up to `boundary` is entirely whitespace
/// (no statements *and* no comment — a comment is non-whitespace).
fn emit_if_empty(then_kw: Range<usize>, boundary: usize, source: &str) -> Option<LintDiagnostic> {
    let region = source.get(then_kw.end..boundary)?;
    if !region.trim().is_empty() {
        return None;
    }
    Some(
        LintDiagnostic::new(then_kw, "this `if` branch has an empty body")
            .with_note("fill in the body, invert the condition, or delete the branch"),
    )
}

fn to_range(range: rowan::TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}
