//! Rule: integer/float divergence heuristics (5.3+ → double-only targets) —
//! warn-tier `LB0606` (SPEC.md §2.1 "diagnostic tiers: warn on observable
//! divergence, error on proven divergence"; §19's open question keeps the
//! default loudness at warn).
//!
//! No rewriting happens here: 5.3 integers ride on doubles after lowering,
//! which is value-exact up to 2^53 and integral by construction from
//! integral inputs. This pass flags the *observable* divergence surfaces,
//! deliberately conservatively (heuristics only fire on constructs whose
//! divergence is concrete, not on every arithmetic expression):
//!
//! - **Integer literals beyond 2^53** — the decoded value cannot be
//!   represented exactly in a double, so the constant itself already
//!   differs on the target.
//! - **`string.format` with `%d`** — 5.3+ raises on non-integral arguments
//!   and prints true 64-bit integers, while double-only targets coerce and
//!   saturate at 2^53 of precision; format output is the classic place
//!   integer/float representation becomes observable (`1` vs `1.0`).
//! - **Integer division** — the divergence caveat for lowered `//` chains
//!   (results beyond 2^53, division by zero) is emitted once per file by
//!   [`crate::floor_div`] when it actually lowers a `//`.
//!
//! Bitwise operands wider than 32 bits also diverge (the shims are 32-bit);
//! that caveat is documented on the `LB0606` explain page rather than
//! warned per operator — flagging every `&` would be noise, and 32-bit
//! usage dominates in code that targets 5.1/5.2 at all.

use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::SyntaxNode;
use luabox_syntax::lua::ast::{AstNode, CallExpr, FieldExpr, LiteralExpr, NameExpr};

use crate::Ctx;
use crate::diag::{self, LowerDiagnostic};

/// Doubles represent integers exactly up to 2^53.
const DOUBLE_EXACT_MAX: u64 = 1 << 53;

pub(crate) fn run(root: &SyntaxNode, ctx: &mut Ctx<'_>) {
    if !ctx.int_float {
        return;
    }
    for element in root.descendants_with_tokens() {
        match element {
            rowan::NodeOrToken::Token(token) => {
                if token.kind() == SyntaxKind::NUMBER
                    && let luabox_hir::Number::Int(value) =
                        luabox_hir::literal::parse_number(token.text())
                    && value.unsigned_abs() > DOUBLE_EXACT_MAX
                {
                    ctx.diags.push(LowerDiagnostic::warning(
                        diag::INT_FLOAT_DIVERGENCE,
                        format!(
                            "the integer literal `{}` exceeds 2^53 and cannot be represented \
                             exactly on a target where all numbers are doubles",
                            token.text()
                        ),
                        token.text_range(),
                    ));
                }
            }
            rowan::NodeOrToken::Node(node) => {
                if let Some(range) = format_d_call(&node) {
                    ctx.diags.push(LowerDiagnostic::warning(
                        diag::INT_FLOAT_DIVERGENCE,
                        "`string.format` with `%d` behaves differently on a double-only \
                         target: Lua 5.3+ rejects non-integral arguments and prints full \
                         64-bit integers; the target coerces doubles and loses precision \
                         beyond 2^53"
                            .to_owned(),
                        range,
                    ));
                }
            }
        }
    }
}

/// A `string.format(<literal containing %d>, ...)` call → the format
/// string's range.
fn format_d_call(node: &SyntaxNode) -> Option<rowan::TextRange> {
    if node.kind() != SyntaxKind::CALL_EXPR {
        return None;
    }
    let call = CallExpr::cast(node.clone())?;
    let callee = FieldExpr::cast(call.callee()?.syntax().clone())?;
    let base = NameExpr::cast(callee.base()?.syntax().clone())?;
    if base.name()?.text() != "string" || callee.field_name()?.text() != "format" {
        return None;
    }
    let args = call.args()?;
    let token = args.string_arg().or_else(|| {
        let first = args.expr_list()?.exprs().next()?;
        let literal = LiteralExpr::cast(first.syntax().clone())?;
        let token = literal.token()?;
        (token.kind() == SyntaxKind::STRING).then_some(token)
    })?;
    let decoded = luabox_hir::literal::decode_string(token.text());
    let text = decoded.as_str()?;
    // `%%d` is a literal percent + d, not a directive.
    let mut rest = text;
    while let Some(pos) = rest.find('%') {
        let after = &rest[pos + 1..];
        if let Some(stripped) = after.strip_prefix('%') {
            rest = stripped;
            continue;
        }
        // Skip flags/width to the conversion character.
        let conv = after.trim_start_matches(|c: char| {
            c.is_ascii_digit() || matches!(c, '-' | '+' | ' ' | '#' | '.')
        });
        if conv.starts_with('d') || conv.starts_with('i') {
            return Some(token.text_range());
        }
        rest = after;
    }
    None
}
