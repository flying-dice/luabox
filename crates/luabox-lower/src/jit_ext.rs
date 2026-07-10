//! Rule: LuaJIT extensions → 5.1 (SPEC.md §2.1).
//!
//! Active only when lowering *from* LuaJIT. `bit.*` calls targeting LuaJIT
//! itself are left alone (the library is native there); this rule fires for
//! the LuaJIT → 5.1 downgrade.
//!
//! # Semantics-preservation argument
//!
//! - `bit.<member>` field accesses on the global name `bit` are rewritten
//!   to `__luabox_rt.<member>`; the helpers ([`crate::polyfill`], signed
//!   family) reproduce `bit`'s documented semantics — signed 32-bit results
//!   via `tobit` normalization, shift counts masked to 5 bits — so
//!   `bit.band(-1, -1) == -1` and `bit.lshift(x, 33) == bit.lshift(x, 1)`
//!   keep holding. Rewriting the *field expression* (not just calls) also
//!   covers first-class uses like `local f = bit.band`.
//! - `require("bit")` (and `require "bit"`) rewrites to `__luabox_rt`
//!   itself, so `local bit = require("bit")` aliases keep working; this
//!   pulls in the full helper family (member-level tree-shaking is
//!   impossible once the module table escapes).
//! - `ffi` — `require("ffi")` — is a hard `LB0605`: there is nothing to
//!   polyfill C data with (SPEC.md §2.1 "`ffi` use = hard diagnostic, not
//!   lowerable"). So are unknown `bit` members and LuaJIT `LL`/`ULL`/`i`
//!   number literals (64-bit cdata boxes have no double representation).
//!
//! Known, documented limitation: `bit` and `ffi` are recognized by name;
//! a user local that shadows the global `bit` with an unrelated table will
//! be rewritten anyway. Real name resolution for this rule can ride on
//! `luabox-hir` later; shadowing the JIT built-ins is rare enough to accept
//! the textual match for now.

use luabox_syntax::lua::ast::{AstNode, CallExpr, FieldExpr, LiteralExpr, NameExpr};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::Ctx;
use crate::diag::{self, LowerDiagnostic};
use crate::polyfill::Helper;

/// The `bit.*` members with a polyfill helper (same rt member name).
fn member_helper(name: &str) -> Option<Helper> {
    Some(match name {
        "band" => Helper::Band,
        "bor" => Helper::Bor,
        "bxor" => Helper::Bxor,
        "bnot" => Helper::Bnot,
        "tobit" => Helper::Tobit,
        "lshift" => Helper::Lshift,
        "rshift" => Helper::Rshift,
        "arshift" => Helper::Arshift,
        "rol" => Helper::Rol,
        "ror" => Helper::Ror,
        "bswap" => Helper::Bswap,
        "tohex" => Helper::Tohex,
        _ => return None,
    })
}

/// A `bit.<known member>` field access, when the rule is active.
pub(crate) fn matches_bit_member(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    ctx.jit_bit && known_bit_member(node).is_some()
}

fn known_bit_member(node: &SyntaxNode) -> Option<Helper> {
    if node.kind() != SyntaxKind::FIELD_EXPR {
        return None;
    }
    let field = FieldExpr::cast(node.clone())?;
    let base = field.base()?;
    let base = NameExpr::cast(base.syntax().clone())?;
    if base.name()?.text() != "bit" {
        return None;
    }
    member_helper(field.field_name()?.text())
}

/// `__luabox_rt.<member>`.
pub(crate) fn build_bit_member(node: &SyntaxNode, ctx: &mut Ctx<'_>) -> String {
    let helper = known_bit_member(node).unwrap_or_else(|| unreachable!("checked by matches"));
    ctx.helpers.insert(helper);
    format!("__luabox_rt.{}", helper.name())
}

/// A `require("bit")` call (any string-literal quoting), when active.
pub(crate) fn matches_require_bit(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    ctx.jit_bit && required_module(node).as_deref() == Some("bit")
}

/// The whole call becomes the rt table itself.
pub(crate) fn build_require_bit(ctx: &mut Ctx<'_>) -> String {
    ctx.helpers.extend(Helper::JIT_BIT_MODULE);
    "__luabox_rt".to_owned()
}

/// The module name of a literal `require("...")`/`require "..."` call.
fn required_module(node: &SyntaxNode) -> Option<String> {
    if node.kind() != SyntaxKind::CALL_EXPR {
        return None;
    }
    let call = CallExpr::cast(node.clone())?;
    let callee = NameExpr::cast(call.callee()?.syntax().clone())?;
    if callee.name()?.text() != "require" {
        return None;
    }
    let args = call.args()?;
    let string_token = args.string_arg().or_else(|| {
        let list = args.expr_list()?;
        let mut exprs = list.exprs();
        let first = exprs.next()?;
        if exprs.next().is_some() {
            return None;
        }
        let literal = LiteralExpr::cast(first.syntax().clone())?;
        let token = literal.token()?;
        (token.kind() == SyntaxKind::STRING).then_some(token)
    })?;
    luabox_hir::literal::decode_string(string_token.text())
        .as_str()
        .map(str::to_owned)
}

/// Diagnostics-only sweep: hard `LB0605` for `ffi`, unknown `bit.*`
/// members, and LuaJIT number-literal extensions.
pub(crate) fn scan_diags(root: &SyntaxNode, ctx: &mut Ctx<'_>) {
    if !ctx.jit_bit {
        return;
    }
    for element in root.descendants_with_tokens() {
        match element {
            rowan::NodeOrToken::Node(node) => {
                if required_module(&node).as_deref() == Some("ffi") {
                    ctx.diags.push(LowerDiagnostic::error(
                        diag::JIT_NOT_LOWERABLE,
                        "`ffi` cannot be lowered: C data has no Lua 5.1 polyfill (SPEC.md §2.1)"
                            .to_owned(),
                        node.text_range(),
                    ));
                } else if node.kind() == SyntaxKind::FIELD_EXPR
                    && known_bit_member(&node).is_none()
                    && is_bit_field(&node)
                {
                    ctx.diags.push(LowerDiagnostic::error(
                        diag::JIT_NOT_LOWERABLE,
                        format!(
                            "`{}` has no polyfill: only the documented `bit.*` members can be \
                             lowered",
                            node.text()
                        ),
                        node.text_range(),
                    ));
                }
            }
            rowan::NodeOrToken::Token(token) => {
                if token.kind() == SyntaxKind::NUMBER
                    && matches!(
                        luabox_hir::literal::parse_number(token.text()),
                        luabox_hir::Number::I64(_)
                            | luabox_hir::Number::U64(_)
                            | luabox_hir::Number::Imaginary(_)
                    )
                {
                    ctx.diags.push(LowerDiagnostic::error(
                        diag::JIT_NOT_LOWERABLE,
                        format!(
                            "the LuaJIT number literal `{}` cannot be lowered: 64-bit/imaginary \
                             cdata boxes have no double representation",
                            token.text()
                        ),
                        token.text_range(),
                    ));
                }
            }
        }
    }
}

/// A `<something>.<member>` where the base is the plain name `bit`.
fn is_bit_field(node: &SyntaxNode) -> bool {
    FieldExpr::cast(node.clone())
        .and_then(|f| f.base())
        .and_then(|b| NameExpr::cast(b.syntax().clone()))
        .and_then(|n| n.name())
        .is_some_and(|t| t.text() == "bit")
}
