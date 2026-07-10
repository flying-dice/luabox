//! Rule: integer division `//` (5.3+) → `math.floor(a / b)` (SPEC.md §2.1).
//!
//! # Semantics-preservation argument
//!
//! Lua 5.3 defines `a // b` as "the quotient rounded towards minus
//! infinity", i.e. exactly `math.floor(a / b)` over the same operands; for
//! float operands the two are identical by definition. For *integer*
//! operands 5.3 computes the quotient in 64-bit integer arithmetic and
//! yields an integer, while the lowered form computes it in double
//! arithmetic and yields a double — but on the 5.1/5.2/LuaJIT targets this
//! rule fires for, *every* number is a double, so "yields a double" is the
//! only faithful reading of the program there. Values are equal as long as
//! the operands and quotient are exactly representable in a double, i.e.
//! for magnitudes up to 2^53; beyond that the double division can round
//! before flooring and diverge. That caveat is surfaced as a warn-tier
//! `LB0606` diagnostic, once per file (SPEC.md §2.1 diagnostic tiers, and
//! §19's open question on divergence loudness).
//!
//! `//` by zero: for floats, `math.floor(a / b)` inherits IEEE semantics
//! (`inf`/`nan`) exactly as 5.3 float `//` does. 5.3 *integer* `//` by zero
//! raises an error where the lowered double form yields `inf`/`nan` — this
//! sits under the same `LB0606` divergence tier (an erroring program is not
//! semantics anyone relies on for value equality).
//!
//! # Precedence
//!
//! The whole `a // b` expression becomes a call — a primary expression —
//! so no wrapping is ever needed *around* the rewrite. Inside it, `/` has
//! exactly the same precedence and associativity as `//` in 5.3, so the
//! operand texts carry over verbatim without extra parentheses:
//! `a * b // c` → `math.floor(a * b / c)` computes `floor((a*b)/c)` either
//! way, and `a // b // c` recursively lowers the left operand first.

use luabox_syntax::lua::ast::{AstNode, BinExpr};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::diag::{self, LowerDiagnostic};
use crate::{Ctx, rewrite};

/// A `BIN_EXPR` whose operator is `//`, when the rule is active.
pub(crate) fn matches(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    ctx.floor_div
        && node.kind() == SyntaxKind::BIN_EXPR
        && BinExpr::cast(node.clone())
            .and_then(|bin| bin.op_token())
            .is_some_and(|op| op.kind() == SyntaxKind::SLASH_SLASH)
}

/// `math.floor(<lhs> / <rhs>)`, operands rendered recursively. `None` if
/// the recovered tree is missing an operand (parse-error inputs never get
/// this far in practice).
pub(crate) fn build(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> Option<String> {
    let bin = BinExpr::cast(node.clone())?;
    let lhs = bin.lhs()?;
    let rhs = bin.rhs()?;
    if !ctx.floor_div_warned {
        ctx.floor_div_warned = true;
        ctx.diags.push(LowerDiagnostic::warning(
            diag::INT_FLOAT_DIVERGENCE,
            format!(
                "`//` lowered to `math.floor` division for {}: all numbers are doubles there, \
                 so integer results beyond 2^53 (and integer division by zero) diverge from \
                 Lua 5.3 integer semantics",
                target_name(ctx)
            ),
            node.text_range(),
        ));
    }
    let lhs = rewrite::render_or_text(lhs.syntax(), ctx);
    let rhs = rewrite::render_or_text(rhs.syntax(), ctx);
    Some(format!("math.floor({lhs} / {rhs})"))
}

fn target_name(ctx: &Ctx<'_>) -> &'static str {
    match ctx.to {
        luabox_syntax::Dialect::Lua51 => "Lua 5.1",
        luabox_syntax::Dialect::Lua52 => "Lua 5.2",
        luabox_syntax::Dialect::Lua53 => "Lua 5.3",
        luabox_syntax::Dialect::Lua54 => "Lua 5.4",
        luabox_syntax::Dialect::LuaJit => "LuaJIT",
    }
}
