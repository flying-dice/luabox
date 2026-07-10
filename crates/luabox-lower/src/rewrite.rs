//! The expression rewrite pass: one walk that applies [`crate::floor_div`],
//! [`crate::bitops`], [`crate::jit_ext`], and the `_ENV`-read rewrite
//! ([`crate::env`]) as targeted edits.
//!
//! For each *outermost* matching expression the pass emits a single edit
//! whose replacement is produced by [`render`], which recurses through the
//! subtree applying every expression rule to nested matches and splicing
//! unchanged text verbatim (tokens and non-matching nodes are copied
//! byte-for-byte, so formatting inside untouched subexpressions survives).
//! Statement-level rules use [`render`] too when their replacement embeds
//! an expression (e.g. the `until not (<cond>)` of a backward-goto rewrite),
//! and mark those statements in [`Ctx::replaced`] so this pass skips them.

use luabox_syntax::lua::SyntaxNode;

use crate::edit::{self, Edit};
use crate::{Ctx, bitops, env, floor_div, jit_ext};

/// Walk the tree and emit one edit per outermost matching expression.
pub(crate) fn run(root: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    if !(ctx.floor_div || ctx.bitops || ctx.jit_bit || ctx.env) {
        return;
    }
    walk(root, ctx, edits);
}

fn walk(node: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    for child in node.children() {
        if ctx.is_replaced(child.text_range()) {
            continue;
        }
        if is_match(&child, ctx)
            && let Some(text) = render(&child, ctx)
        {
            edit::push(edits, child.text_range(), text);
            continue;
        }
        walk(&child, ctx, edits);
    }
}

/// Does any expression rule rewrite this node directly?
fn is_match(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    floor_div::matches(node, ctx)
        || bitops::matches_binary(node, ctx)
        || bitops::matches_unary(node, ctx)
        || jit_ext::matches_bit_member(node, ctx)
        || jit_ext::matches_require_bit(node, ctx)
        || env::matches_env_read(node, ctx)
}

/// Render `node` with every expression rule applied throughout its subtree.
/// `None` means nothing inside changed (callers keep the original text).
pub(crate) fn render(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> Option<String> {
    if floor_div::matches(node, ctx)
        && let Some(text) = floor_div::build(node, ctx)
    {
        return Some(text);
    }
    if bitops::matches_binary(node, ctx)
        && let Some(text) = bitops::build_binary(node, ctx)
    {
        return Some(text);
    }
    if bitops::matches_unary(node, ctx)
        && let Some(text) = bitops::build_unary(node, ctx)
    {
        return Some(text);
    }
    if jit_ext::matches_bit_member(node, ctx) {
        return Some(jit_ext::build_bit_member(node, ctx));
    }
    if jit_ext::matches_require_bit(node, ctx) {
        return Some(jit_ext::build_require_bit(ctx));
    }
    if env::matches_env_read(node, ctx) {
        return Some("getfenv(1)".to_owned());
    }
    // Generic splice: copy tokens verbatim, recurse into child nodes.
    let mut out = String::new();
    let mut changed = false;
    for element in node.children_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => out.push_str(token.text()),
            rowan::NodeOrToken::Node(child) => match render(&child, ctx) {
                Some(text) => {
                    changed = true;
                    out.push_str(&text);
                }
                None => out.push_str(&child.text().to_string()),
            },
        }
    }
    changed.then_some(out)
}

/// [`render`] or the node's original text — for building replacement text
/// around operands.
pub(crate) fn render_or_text(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> String {
    render(node, ctx).unwrap_or_else(|| node.text().to_string())
}
