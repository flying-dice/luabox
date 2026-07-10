//! Rule: bitwise operators `& | ~ << >>` and unary `~` (5.3+) →
//! `__luabox_rt` helper calls (SPEC.md §2.1).
//!
//! # Semantics-preservation argument
//!
//! Each operator maps to a named helper call — `a & b` →
//! `__luabox_rt.band(a, b)` — whose body is chosen per target by
//! [`crate::polyfill`]: `bit32` on 5.2, wrapped `bit` on LuaJIT, pure Lua
//! on 5.1. Operand evaluation order and count are unchanged (Lua evaluates
//! call arguments left to right, exactly as it evaluates binary operands).
//! Operands become call arguments, which accept any expression, so no
//! wrapping is ever needed; nesting is handled by recursive rendering
//! (`a & b | c` → `__luabox_rt.bor(__luabox_rt.band(a, b), c)`, preserving
//! the parse's association).
//!
//! Documented divergence (SPEC.md §2.1 tiers): 5.3 bit operations are
//! 64-bit integer ops with string/float coercion (floats must have integral
//! value); every available shim is 32-bit and coerces through doubles.
//! Operands with significant bits above 2^32 truncate — see the `LB0606`
//! explain page. The helper set is tree-shaken: only operators that occur
//! pull their helper (plus shared private cores) into the prelude.

use luabox_syntax::lua::ast::{AstNode, BinExpr, PrefixExpr};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::polyfill::Helper;
use crate::{Ctx, rewrite};

/// Binary bitwise operator kinds → their helper.
fn binary_helper(kind: SyntaxKind) -> Option<Helper> {
    Some(match kind {
        SyntaxKind::AMP => Helper::Band,
        SyntaxKind::PIPE => Helper::Bor,
        SyntaxKind::TILDE => Helper::Bxor,
        SyntaxKind::LT_LT => Helper::Shl,
        SyntaxKind::GT_GT => Helper::Shr,
        _ => return None,
    })
}

/// A `BIN_EXPR` with a bitwise operator, when the rule is active. `~=` is
/// its own token kind and never matches.
pub(crate) fn matches_binary(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    ctx.bitops
        && node.kind() == SyntaxKind::BIN_EXPR
        && BinExpr::cast(node.clone())
            .and_then(|bin| bin.op_token())
            .is_some_and(|op| binary_helper(op.kind()).is_some())
}

/// A unary `~a` (`PREFIX_EXPR` with `~`), when the rule is active. Unary
/// `-`, `#`, and `not` never match.
pub(crate) fn matches_unary(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    ctx.bitops
        && node.kind() == SyntaxKind::PREFIX_EXPR
        && PrefixExpr::cast(node.clone())
            .and_then(|prefix| prefix.op_token())
            .is_some_and(|op| op.kind() == SyntaxKind::TILDE)
}

/// `__luabox_rt.<op>(<lhs>, <rhs>)`, operands rendered recursively.
pub(crate) fn build_binary(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> Option<String> {
    let bin = BinExpr::cast(node.clone())?;
    let helper = binary_helper(bin.op_token()?.kind())?;
    let lhs = bin.lhs()?;
    let rhs = bin.rhs()?;
    ctx.helpers.insert(helper);
    let lhs = rewrite::render_or_text(lhs.syntax(), ctx);
    let rhs = rewrite::render_or_text(rhs.syntax(), ctx);
    Some(format!("__luabox_rt.{}({lhs}, {rhs})", helper.name()))
}

/// `__luabox_rt.bnot(<operand>)`, the operand rendered recursively.
pub(crate) fn build_unary(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> Option<String> {
    let prefix = PrefixExpr::cast(node.clone())?;
    let operand = prefix.operand()?;
    ctx.helpers.insert(Helper::Bnot);
    let operand = rewrite::render_or_text(operand.syntax(), ctx);
    Some(format!("__luabox_rt.bnot({operand})"))
}
