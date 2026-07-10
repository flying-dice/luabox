//! Rule: `<const>`/`<close>` attributes (5.4) → pre-5.4 targets
//! (SPEC.md §2.1: "scope-exit rewrite via pcall wrapper / plain local +
//! const-check at compile time").
//!
//! # `<const>` — semantics-preservation argument
//!
//! `<const>` has *no runtime semantics* in 5.4: it is a compile-time
//! reassignment ban (plus enabling upvalue-to-constant folding, an
//! optimization, not an observable behaviour). Dropping the attribute is
//! therefore exact — provided the ban still holds. The rule enforces it
//! itself: any assignment to the constant's name in its scope (including
//! captured-upvalue assignments inside nested functions, which 5.4 also
//! rejects) is a hard `LB0602`, with lexical shadowing respected — a
//! rebinding `local` of the same name ends the check for the rest of that
//! scope, just as it ends the constant's visibility.
//!
//! # `<close>` — semantics-preservation argument
//!
//! `local h <close> = v` runs `getmetatable(v).__close(v, err)` when the
//! variable goes out of scope, normally or on error. The rewrite drops the
//! attribute and wraps the rest of the block — the variable's scope tail —
//! in the runtime helper:
//!
//! ```lua
//! local h = open()
//! __luabox_rt.close_scope(h, function()
//! <scope tail>
//! end)
//! ```
//!
//! `close_scope` (see [`crate::polyfill`]) is the ticket's
//! `do local ok, err = pcall(function() … end) <call __close> if not ok
//! then error(err, 0) end end` pattern, hoisted into the shared rt module:
//! the tail runs under `pcall`; `__close(v, err)` fires afterwards with
//! the error object (or `nil` on the normal path), matching 5.4's
//! protocol, including the `nil`/`false` no-op; the error is then
//! re-raised with `error(err, 0)`, preserving the error object unmodified.
//! Closure capture is by reference (upvalues), so reads *and writes* of
//! enclosing locals inside the tail behave identically. Multiple `<close>`
//! in one block nest, closing in reverse declaration order as 5.4 does.
//!
//! **Error tier (`LB0603`, hard):** the tail becomes a function body, so a
//! tail containing `return`, `break`, a `goto`/label crossing the tail
//! boundary, or `...` (all of which cannot cross a closure boundary) is
//! not lowerable this way. Multi-name `local` with `<close>` (already
//! dubious 5.4) is rejected too.
//!
//! **Warn tier (`LB0603`, suppressible):** SPEC.md §2.1 acknowledges true
//! `<close>` semantics under error inside 5.1 coroutines are non-lowerable
//! — if the coroutine dies while suspended inside the tail, 5.4 closes the
//! variable when the coroutine is collected or explicitly closed, while
//! the `pcall` wrapper never resumes. The warning documents this delta and
//! `---@luabox-allow lossy-lowering` on the declaration acknowledges it.

use luabox_syntax::lua::ast::{
    AstNode, FunctionName, GenericForStmt, LocalName, LocalStmt, NumericForStmt, Param, ParamList,
};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::diag::{self, LowerDiagnostic};
use crate::edit::{self, Edit};
use crate::polyfill::Helper;
use crate::{Ctx, gotos};

pub(crate) fn run(root: &SyntaxNode, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    if !ctx.attribs {
        return;
    }
    // Collect first: `<close>` closers stack at shared block ends and must
    // be pushed innermost-first (reverse source order).
    let mut closes: Vec<(SyntaxNode, String)> = Vec::new();
    for local_name in root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LOCAL_NAME)
    {
        if ctx.is_replaced(local_name.text_range()) {
            continue; // consumed by a statement-level rule (e.g. `_ENV`)
        }
        let Some(name) = LocalName::cast(local_name.clone()) else {
            continue;
        };
        let Some(attrib) = name.attrib() else {
            continue;
        };
        let Some(ident) = name.name() else { continue };
        // Drop the attribute text (everything between the name and the
        // closing `>`), keeping the plain local.
        edit::push(
            edits,
            rowan::TextRange::new(ident.text_range().end(), attrib.syntax().text_range().end()),
            String::new(),
        );
        let Some(local_stmt) = local_name.parent() else {
            continue;
        };
        // Both attributes make the variable constant in 5.4.
        check_reassignments(ident.text(), &local_stmt, ctx);
        if attrib.is_close() {
            closes.push((local_stmt, ident.text().to_owned()));
        }
    }
    for (local_stmt, name) in closes.iter().rev() {
        rewrite_close(local_stmt, name, ctx, edits);
    }
}

/// Wrap the scope tail after `local_stmt` in `__luabox_rt.close_scope`.
fn rewrite_close(local_stmt: &SyntaxNode, name: &str, ctx: &mut Ctx<'_>, edits: &mut Vec<Edit>) {
    let Some(local) = LocalStmt::cast(local_stmt.clone()) else {
        return;
    };
    if local.names().count() != 1 {
        ctx.diags.push(LowerDiagnostic::error(
            diag::CLOSE_FIDELITY,
            "cannot lower `<close>` in a multi-name `local`; declare the to-be-closed \
             variable on its own"
                .to_owned(),
            local_stmt.text_range(),
        ));
        return;
    }
    let Some(block) = local_stmt.parent() else {
        return;
    };
    let stmts: Vec<SyntaxNode> = block.children().collect();
    let Some(decl_idx) = stmts.iter().position(|s| s == local_stmt) else {
        return;
    };
    let tail = &stmts[decl_idx + 1..];

    // The tail becomes a closure body: nothing may cross that boundary.
    let mut lowerable = true;
    for stmt in tail {
        check_tail_stmt(stmt, tail, ctx, &mut lowerable);
    }
    // Goto rewrite regions must lie entirely before the declaration or
    // entirely inside the tail; a straddling region would interleave its
    // wrapper with the closure.
    let straddled = ctx.goto_intervals.get(&block).is_some_and(|intervals| {
        intervals
            .iter()
            .any(|&(min, max)| min <= decl_idx && decl_idx < max)
    });
    if straddled {
        ctx.diags.push(LowerDiagnostic::error(
            diag::CLOSE_FIDELITY,
            "cannot lower `<close>`: a goto rewrite region straddles the declaration".to_owned(),
            local_stmt.text_range(),
        ));
        lowerable = false;
    }
    if !lowerable {
        return;
    }

    if !has_allow_annotation(local_stmt) {
        ctx.diags.push(LowerDiagnostic::warning(
            diag::CLOSE_FIDELITY,
            "`<close>` lowered via a pcall scope-exit wrapper: if a coroutine suspended in \
             this scope is discarded, the close action never runs (unlike Lua 5.4); \
             annotate the local with `---@luabox-allow lossy-lowering` to acknowledge"
                .to_owned(),
            local_stmt.text_range(),
        ));
    }

    ctx.helpers.insert(Helper::CloseScope);
    let indent = ctx.indent_at(local_stmt.text_range().start());
    edit::insert(
        edits,
        local_stmt.text_range().end(),
        format!("\n{indent}__luabox_rt.close_scope({name}, function()"),
    );
    let close_at = tail
        .last()
        .map_or(local_stmt.text_range().end(), |s| s.text_range().end());
    edit::insert(edits, close_at, format!("\n{indent}end)"));
}

/// Reject tail statements that cannot move into a closure body.
fn check_tail_stmt(stmt: &SyntaxNode, tail: &[SyntaxNode], ctx: &mut Ctx<'_>, ok: &mut bool) {
    let mut reject = |node: &SyntaxNode, what: &str| {
        ctx.diags.push(LowerDiagnostic::error(
            diag::CLOSE_FIDELITY,
            format!(
                "cannot lower `<close>`: the scope tail contains {what}, which cannot cross \
                 the pcall wrapper's function boundary"
            ),
            node.text_range(),
        ));
        *ok = false;
    };
    let mut stack = vec![stmt.clone()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            // Function bodies are their own boundary: anything inside them
            // is unaffected by the wrapper.
            SyntaxKind::FUNCTION_EXPR
            | SyntaxKind::FUNCTION_DECL_STMT
            | SyntaxKind::LOCAL_FUNCTION_STMT => {}
            SyntaxKind::RETURN_STMT => reject(&node, "a `return`"),
            SyntaxKind::VARARG_EXPR => reject(&node, "`...`"),
            SyntaxKind::BREAK_STMT => {
                if !loop_between(&node, stmt) {
                    reject(&node, "a `break` bound to an outer loop");
                }
            }
            SyntaxKind::GOTO_STMT => {
                // A goto whose label lives inside the tail stays inside the
                // closure; anything else crosses the boundary. (For a 5.1
                // target the goto rule has already restructured or rejected
                // these; this covers 5.2/5.3 targets where goto survives.)
                let target = gotos::resolve_label(&node);
                let inside = target.is_some_and(|label| {
                    tail.iter()
                        .any(|t| t.text_range().contains_range(label.text_range()))
                });
                if !inside {
                    reject(&node, "a `goto` that jumps out of the scope");
                }
            }
            _ => stack.extend(node.children()),
        }
    }
}

/// Is there a loop between `node` and `root` (exclusive) that `break`
/// binds to?
fn loop_between(node: &SyntaxNode, root: &SyntaxNode) -> bool {
    if node == root {
        return false; // the break *is* the tail statement: binds outside
    }
    let mut current = node.parent();
    while let Some(n) = current {
        if n == *root {
            return matches!(
                root.kind(),
                SyntaxKind::WHILE_STMT
                    | SyntaxKind::REPEAT_STMT
                    | SyntaxKind::NUMERIC_FOR_STMT
                    | SyntaxKind::GENERIC_FOR_STMT
            );
        }
        if matches!(
            n.kind(),
            SyntaxKind::WHILE_STMT
                | SyntaxKind::REPEAT_STMT
                | SyntaxKind::NUMERIC_FOR_STMT
                | SyntaxKind::GENERIC_FOR_STMT
        ) {
            return true;
        }
        current = n.parent();
    }
    false
}

/// Does a `---@luabox-allow lossy-lowering` comment ride on this statement
/// (in the trivia run immediately above it, or trailing on the same line)?
fn has_allow_annotation(stmt: &SyntaxNode) -> bool {
    const MARKER: &str = "@luabox-allow lossy-lowering";
    // Preceding trivia run (walked token-wise: trivia may attach to an
    // enclosing node in the lossless tree, not as a sibling).
    let mut prev = stmt.first_token().and_then(|t| t.prev_token());
    while let Some(token) = prev {
        if !token.kind().is_trivia() {
            break;
        }
        if token.kind() == SyntaxKind::COMMENT && token.text().contains(MARKER) {
            return true;
        }
        prev = token.prev_token();
    }
    // Trailing comment on the same line.
    let mut next = stmt.last_token().and_then(|t| t.next_token());
    while let Some(token) = next {
        match token.kind() {
            SyntaxKind::WHITESPACE if !token.text().contains('\n') => {
                next = token.next_token();
            }
            SyntaxKind::COMMENT => return token.text().contains(MARKER),
            _ => break,
        }
    }
    false
}

/// Compile-time `<const>` reassignment check (`LB0602`), shadowing-aware.
fn check_reassignments(name: &str, local_stmt: &SyntaxNode, ctx: &mut Ctx<'_>) {
    let Some(block) = local_stmt.parent() else {
        return;
    };
    let after: Vec<SyntaxNode> = block
        .children()
        .skip_while(|s| s != local_stmt)
        .skip(1)
        .collect();
    scan_stmts(&after, name, ctx);
}

/// Scan a statement list for assignments to `name`, stopping when a new
/// declaration shadows it.
fn scan_stmts(stmts: &[SyntaxNode], name: &str, ctx: &mut Ctx<'_>) {
    for stmt in stmts {
        match stmt.kind() {
            SyntaxKind::LOCAL_STMT => {
                scan_exprs(stmt, name, ctx); // initializers first (old scope)
                let declares = stmt
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::LOCAL_NAME)
                    .filter_map(|c| c.children_with_tokens().find_map(as_ident))
                    .any(|t| t.text() == name);
                if declares {
                    return; // shadowed for the rest of this scope
                }
            }
            SyntaxKind::LOCAL_FUNCTION_STMT => {
                let fn_name = stmt.children_with_tokens().find_map(as_ident);
                if fn_name.is_some_and(|t| t.text() == name) {
                    return; // shadowed (the local is in scope in its own body)
                }
                scan_function_body(stmt, name, ctx);
            }
            SyntaxKind::ASSIGN_STMT => {
                let mut lists = stmt
                    .children()
                    .filter(|c| c.kind() == SyntaxKind::EXPR_LIST);
                if let Some(targets) = lists.next() {
                    for target in targets.children() {
                        if target.kind() == SyntaxKind::NAME_EXPR
                            && target
                                .children_with_tokens()
                                .find_map(as_ident)
                                .is_some_and(|t| t.text() == name)
                        {
                            ctx.diags.push(LowerDiagnostic::error(
                                diag::CONST_REASSIGNED,
                                format!("assignment to constant `{name}` (declared `<const>`)"),
                                target.text_range(),
                            ));
                        } else {
                            scan_exprs(&target, name, ctx);
                        }
                    }
                }
                if let Some(values) = lists.next() {
                    scan_exprs(&values, name, ctx);
                }
            }
            SyntaxKind::FUNCTION_DECL_STMT => {
                // `function name() … end` is sugar for `name = function`.
                let decl_name = stmt.children().find_map(FunctionName::cast);
                if let Some(decl_name) = decl_name {
                    let segments: Vec<SyntaxToken> = decl_name.segments().collect();
                    if segments.len() == 1 && segments[0].text() == name {
                        ctx.diags.push(LowerDiagnostic::error(
                            diag::CONST_REASSIGNED,
                            format!("assignment to constant `{name}` (declared `<const>`)"),
                            decl_name.syntax().text_range(),
                        ));
                    }
                }
                scan_function_body(stmt, name, ctx);
            }
            SyntaxKind::NUMERIC_FOR_STMT => {
                scan_non_block_exprs(stmt, name, ctx);
                let shadows = NumericForStmt::cast(stmt.clone())
                    .and_then(|f| f.var())
                    .is_some_and(|v| v.text() == name);
                if !shadows {
                    scan_child_blocks(stmt, name, ctx);
                }
            }
            SyntaxKind::GENERIC_FOR_STMT => {
                scan_non_block_exprs(stmt, name, ctx);
                let shadows = GenericForStmt::cast(stmt.clone())
                    .is_some_and(|f| f.vars().any(|v| v.text() == name));
                if !shadows {
                    scan_child_blocks(stmt, name, ctx);
                }
            }
            _ => {
                scan_non_block_exprs(stmt, name, ctx);
                scan_child_blocks(stmt, name, ctx);
            }
        }
    }
}

fn as_ident(element: rowan::NodeOrToken<SyntaxNode, SyntaxToken>) -> Option<SyntaxToken> {
    element
        .into_token()
        .filter(|t| t.kind() == SyntaxKind::IDENT)
}

/// Scan the direct child blocks of a compound statement (fresh scopes —
/// shadowing inside them ends at their `end`).
fn scan_child_blocks(stmt: &SyntaxNode, name: &str, ctx: &mut Ctx<'_>) {
    for child in stmt.children() {
        match child.kind() {
            SyntaxKind::BLOCK => scan_stmts(&child.children().collect::<Vec<_>>(), name, ctx),
            SyntaxKind::ELSEIF_CLAUSE | SyntaxKind::ELSE_CLAUSE => {
                scan_child_blocks(&child, name, ctx);
            }
            _ => {}
        }
    }
}

/// Scan the expression children of a statement (conditions, values, call
/// arguments) for nested function bodies that assign to `name`.
fn scan_non_block_exprs(stmt: &SyntaxNode, name: &str, ctx: &mut Ctx<'_>) {
    for child in stmt.children() {
        if !matches!(
            child.kind(),
            SyntaxKind::BLOCK | SyntaxKind::ELSEIF_CLAUSE | SyntaxKind::ELSE_CLAUSE
        ) {
            scan_exprs(&child, name, ctx);
        }
    }
}

/// Walk an expression subtree; on nested `function` expressions whose
/// parameters don't shadow `name`, scan the body (upvalue assignments to a
/// 5.4 constant are illegal too).
fn scan_exprs(node: &SyntaxNode, name: &str, ctx: &mut Ctx<'_>) {
    for child in node.children() {
        if child.kind() == SyntaxKind::FUNCTION_EXPR {
            scan_function_body(&child, name, ctx);
        } else {
            scan_exprs(&child, name, ctx);
        }
    }
}

/// Scan a function's body unless one of its parameters shadows `name`.
fn scan_function_body(function: &SyntaxNode, name: &str, ctx: &mut Ctx<'_>) {
    let params_shadow = function
        .children()
        .find_map(ParamList::cast)
        .is_some_and(|list| {
            list.params()
                .filter_map(|p: Param| p.name())
                .any(|t| t.text() == name)
        });
    if params_shadow {
        return;
    }
    if let Some(body) = function.children().find(|c| c.kind() == SyntaxKind::BLOCK) {
        scan_stmts(&body.children().collect::<Vec<_>>(), name, ctx);
    }
}
