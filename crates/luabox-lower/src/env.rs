//! Rule: explicit `_ENV` (5.2+) → `setfenv`/`getfenv` on 5.1/LuaJIT
//! (SPEC.md §2.1).
//!
//! # Semantics-preservation argument
//!
//! In 5.2+ every chunk compiles against an `_ENV` upvalue and "globals"
//! are sugar for `_ENV.x`; 5.1 instead gives every function an environment
//! table reachable via `getfenv`/`setfenv`. The lowerable idioms map
//! one-to-one:
//!
//! - **`local _ENV = <expr>`** as a direct statement of the chunk or of a
//!   function body → `setfenv(1, <expr>)`. From that statement on, every
//!   global access in the rest of that function resolves through the new
//!   table — exactly what the fresh `_ENV` local means for the remainder
//!   of the block, *provided the block is the whole function body/chunk*,
//!   which is why the rule requires that position. (In a nested `do` block
//!   the 5.2 local would go out of scope at `end` while `setfenv` would
//!   not — that shape is `LB0604` instead.)
//! - **`_ENV = <expr>`** (single target, single value, same position
//!   requirement) → `setfenv(1, <expr>)`: rebinding the environment for
//!   the rest of the function.
//! - **`_ENV` read as an expression** → `getfenv(1)`: both denote "the
//!   current environment table".
//!
//! Documented delta: in 5.2+ closures share the `_ENV` upvalue with their
//! enclosing function, so rebinding it affects already-created closures;
//! `setfenv(1, t)` affects only the current function. Code that rebinds
//! `_ENV` *and* leans on upvalue sharing across closures is exotic by any
//! measure; the common module-preamble idiom (`local _ENV = t` at the top,
//! before any closure exists) is exact.
//!
//! Everything else — `_ENV` in a multi-name local or multi-target
//! assignment, `local _ENV` in a nested block, an `_ENV` function
//! parameter — is a hard `LB0604` (SPEC.md §2.1 "exotic uses").

use luabox_syntax::lua::ast::{AssignStmt, AstNode, Expr, LocalStmt, NameExpr, Param};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};

use crate::diag::{self, LowerDiagnostic};
use crate::edit::{self, Edit};
use crate::{Ctx, rewrite};

/// Statement-level `_ENV` rewrites and the exotic-use diagnostics.
pub(crate) fn run(root: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    if !ctx.env {
        return;
    }
    for node in root.descendants() {
        match node.kind() {
            SyntaxKind::LOCAL_STMT => lower_local(&node, ctx, edits),
            SyntaxKind::ASSIGN_STMT => lower_assign(&node, ctx, edits),
            SyntaxKind::PARAM => {
                if Param::cast(node.clone())
                    .and_then(|p| p.name())
                    .is_some_and(|t| t.text() == "_ENV")
                {
                    ctx.diags.push(LowerDiagnostic::error(
                        diag::ENV_NOT_LOWERABLE,
                        "an `_ENV` parameter cannot be lowered to setfenv/getfenv; \
                         restructure to pass the table under another name (LB0604)"
                            .to_owned(),
                        node.text_range(),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// `local _ENV = t` → `setfenv(1, t)` (or `LB0604` for exotic shapes).
fn lower_local(node: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    let Some(local) = LocalStmt::cast(node.clone()) else {
        return;
    };
    let names: Vec<_> = local.names().collect();
    if !names
        .iter()
        .any(|n| n.name().is_some_and(|t| t.text() == "_ENV"))
    {
        return;
    }
    let sole_value = local.values().and_then(|values| {
        let mut exprs = values.exprs();
        let first = exprs.next()?;
        exprs.next().is_none().then_some(first)
    });
    if names.len() == 1
        && let Some(value) = sole_value
        && is_function_body_or_chunk_stmt(node)
    {
        let value = rewrite::render_or_text(value.syntax(), ctx);
        edit::push(edits, node.text_range(), format!("setfenv(1, {value})"));
        ctx.replaced.push(node.text_range());
        return;
    }
    ctx.diags.push(LowerDiagnostic::error(
        diag::ENV_NOT_LOWERABLE,
        "only `local _ENV = <expr>` directly in a chunk or function body lowers to \
         `setfenv(1, <expr>)`; this `_ENV` declaration is not lowerable"
            .to_owned(),
        node.text_range(),
    ));
}

/// `_ENV = t` → `setfenv(1, t)` (or `LB0604` for exotic shapes).
fn lower_assign(node: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    let Some(assign) = AssignStmt::cast(node.clone()) else {
        return;
    };
    let targets: Vec<_> = assign
        .targets()
        .map(|t| t.exprs().collect())
        .unwrap_or_default();
    if !targets.iter().any(is_env_name) {
        return;
    }
    let sole_value = assign.values().and_then(|values| {
        let mut exprs = values.exprs();
        let first = exprs.next()?;
        exprs.next().is_none().then_some(first)
    });
    if targets.len() == 1
        && let Some(value) = sole_value
        && is_function_body_or_chunk_stmt(node)
    {
        let value = rewrite::render_or_text(value.syntax(), ctx);
        edit::push(edits, node.text_range(), format!("setfenv(1, {value})"));
        ctx.replaced.push(node.text_range());
        return;
    }
    ctx.diags.push(LowerDiagnostic::error(
        diag::ENV_NOT_LOWERABLE,
        "only a plain `_ENV = <expr>` directly in a chunk or function body lowers to \
         `setfenv(1, <expr>)`; this `_ENV` assignment is not lowerable"
            .to_owned(),
        node.text_range(),
    ));
}

fn is_env_name(expr: &Expr) -> bool {
    NameExpr::cast(expr.syntax().clone())
        .and_then(|n| n.name())
        .is_some_and(|t| t.text() == "_ENV")
}

/// Is `stmt` a direct child of the chunk block or of a function body block?
fn is_function_body_or_chunk_stmt(stmt: &SyntaxNode) -> bool {
    let Some(block) = stmt.parent() else {
        return false;
    };
    if block.kind() != SyntaxKind::BLOCK {
        return false;
    }
    block.parent().is_some_and(|owner| {
        matches!(
            owner.kind(),
            SyntaxKind::SOURCE_FILE
                | SyntaxKind::FUNCTION_EXPR
                | SyntaxKind::FUNCTION_DECL_STMT
                | SyntaxKind::LOCAL_FUNCTION_STMT
        )
    })
}

/// An `_ENV` read in expression position (the [`crate::rewrite`] pass turns
/// it into `getfenv(1)`). Assignment *targets* are handled or diagnosed at
/// statement level and never match here.
pub(crate) fn matches_env_read(node: &SyntaxNode, ctx: &Ctx<'_>) -> bool {
    if !ctx.env || node.kind() != SyntaxKind::NAME_EXPR {
        return false;
    }
    if NameExpr::cast(node.clone())
        .and_then(|n| n.name())
        .is_none_or(|t| t.text() != "_ENV")
    {
        return false;
    }
    !is_assign_target(node)
}

/// Is this expression one of the targets (left side) of an assignment?
fn is_assign_target(node: &SyntaxNode) -> bool {
    let Some(list) = node.parent() else {
        return false;
    };
    if list.kind() != SyntaxKind::EXPR_LIST {
        return false;
    }
    let Some(assign) = list.parent() else {
        return false;
    };
    assign.kind() == SyntaxKind::ASSIGN_STMT
        && assign
            .children()
            .find(|c| c.kind() == SyntaxKind::EXPR_LIST)
            .is_some_and(|first| first == list)
}
