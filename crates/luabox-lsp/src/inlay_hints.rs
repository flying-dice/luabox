//! Inlay hints: the display-mode inference rendered into the source, so
//! unannotated Lua reads like a statically typed language.
//!
//! Three hint sites, all fed by [`luabox_db::BindingTypes`] (the
//! `infer_display_types` query, which runs the rich table inference with
//! call-site parameter seeding):
//!
//! - **Bindings** — `local point`&nbsp;`: Point`; locals, for-vars, and —
//!   because parameters are seeded from their call sites — function
//!   parameters: `function area(w`&nbsp;`: integer`&nbsp;`, h`&nbsp;`: integer`&nbsp;`)`.
//! - **Returns** — `function f(...)`&nbsp;`: integer` after the parameter
//!   list, from the inferred union across the body's `return` statements.
//!
//! Annotated bindings hint too — a `---@param width number` lives in the
//! doc block *above* the signature, so the inline `width`&nbsp;`: number`
//! still carries its weight (the annotation is authoritative and becomes
//! the hint). What is skipped: bindings whose type stayed `unknown`
//! (noise), and `local function f` names and the implicit `self` (their
//! "types" are the signature — hover territory).
//!
//! Literal types are widened to their primitives for display
//! ([`Ty::widened`]): a binding initialised from `42` hints as
//! `: integer`, the annotation a user would have written. Long renderings
//! are elided at a fixed budget: a hint is a glance surface; hover carries
//! the full type.

use lsp_types::{InlayHint, InlayHintKind, InlayHintLabel};
use luabox_hir::BindingKind;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::luacats::Tag;
use luabox_types::ty::Ty;
use luabox_types::{InferredBinding, InferredReturn};

use crate::sema::{self, FileSema};

/// Label budget in bytes; inferred table shapes can render arbitrarily wide.
const MAX_LABEL: usize = 60;

/// Compute the inlay hints for the byte range `start..end` of a document.
#[must_use]
pub fn inlay_hints(
    sema: &FileSema,
    bindings: &[InferredBinding],
    fn_returns: &[InferredReturn],
    start: usize,
    end: usize,
) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    for binding in bindings {
        if binding.range.start >= end || binding.range.end < start {
            continue;
        }
        if matches!(
            binding.kind,
            BindingKind::LocalFunction | BindingKind::SelfParam
        ) {
            continue;
        }
        if matches!(binding.ty, Ty::Unknown) {
            continue;
        }
        hints.push(hint(
            sema,
            binding.range.end,
            format!(": {}", binding.ty.widened()),
        ));
    }
    for ret in fn_returns {
        if ret.range.start >= end || ret.range.end < start {
            continue;
        }
        let Some(offset) = param_list_end(sema, ret.range.start, ret.range.end) else {
            continue;
        };
        let rendered: Vec<String> = ret
            .returns
            .iter()
            .map(|ty| ty.widened().to_string())
            .collect();
        hints.push(hint(sema, offset, format!(": {}", rendered.join(", "))));
    }
    // Annotated functions: render the `---@return` tags verbatim (their
    // type names may not resolve in the per-file environment — `.luab`
    // shapes, cross-file classes — but the annotation text is exact).
    // Inference skips these functions, so the two sources never overlap.
    for item in sema.items() {
        let Some(target) = item.target else {
            continue;
        };
        if target.start >= end || target.end < start {
            continue;
        }
        let rendered: Vec<String> = item
            .block
            .tags
            .iter()
            .filter_map(|tag| match tag {
                Tag::Return(r) => Some(r),
                _ => None,
            })
            .flat_map(|r| r.items.iter())
            .map(|it| sema::render_type(&it.ty))
            .collect();
        if rendered.is_empty() {
            continue;
        }
        let Some(offset) = param_list_end(sema, target.start, target.end) else {
            continue;
        };
        hints.push(hint(sema, offset, format!(": {}", rendered.join(", "))));
    }
    hints.sort_by_key(|h| (h.position.line, h.position.character));
    hints
}

fn hint(sema: &FileSema, offset: usize, label: String) -> InlayHint {
    InlayHint {
        position: sema.index.position(offset),
        label: InlayHintLabel::String(elide(label)),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    }
}

/// The end offset of the parameter list (just after `)`) of the function
/// spanning exactly `start..end`: the anchor for a return-type hint.
/// `LOCAL_STMT` is accepted for annotation targets (`local f = function`).
fn param_list_end(sema: &FileSema, start: usize, end: usize) -> Option<usize> {
    let node = sema.root.descendants().find(|node| {
        let range = node.text_range();
        usize::from(range.start()) == start
            && usize::from(range.end()) == end
            && matches!(
                node.kind(),
                SyntaxKind::FUNCTION_EXPR
                    | SyntaxKind::FUNCTION_DECL_STMT
                    | SyntaxKind::LOCAL_FUNCTION_STMT
                    | SyntaxKind::LOCAL_STMT
            )
    })?;
    // The first parameter list in preorder is the function's own (nested
    // functions come after it in the body).
    let params = node
        .descendants()
        .find(|n| n.kind() == SyntaxKind::PARAM_LIST)?;
    Some(usize::from(params.text_range().end()))
}

/// Truncate a label to [`MAX_LABEL`] bytes on a char boundary, marking the
/// cut with an ellipsis.
fn elide(mut label: String) -> String {
    if label.len() <= MAX_LABEL {
        return label;
    }
    let mut cut = MAX_LABEL;
    while !label.is_char_boundary(cut) {
        cut -= 1;
    }
    label.truncate(cut);
    label.push('…');
    label
}
