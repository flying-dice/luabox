//! `shadowed-local` (suspicious): a `local` that shadows a live binding in an
//! enclosing scope.

use std::collections::HashMap;
use std::ops::Range;

use luabox_diag::Code;
use luabox_hir::{BindingId, Block, BodyId, Expr, ExprId, LoweredFile, Stmt, TableEntry};

use crate::context::{LintContext, to_range};
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// A `local`/loop variable that shadows a still-live same-name binding in an
/// *enclosing* block or function (SPEC.md §9). Re-declaring in the **same**
/// block (`local x = 1; local x = f(x)`) is idiomatic and is not flagged.
pub struct ShadowedLocal;

impl Rule for ShadowedLocal {
    fn id(&self) -> &'static str {
        "shadowed-local"
    }

    fn tier(&self) -> Tier {
        Tier::Suspicious
    }

    fn code(&self) -> Code {
        Code::new(503)
    }

    fn description(&self) -> &'static str {
        "a local shadows a binding from an enclosing scope"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        let mut walker = Walker {
            lowered: ctx.lowered,
            out: Vec::new(),
            stack: Vec::new(),
        };
        walker.walk_function(ctx.lowered.chunk());
        walker.out
    }
}

/// One lexical scope frame: declared name → its declaration range.
type Frame = HashMap<String, Range<usize>>;

struct Walker<'a> {
    lowered: &'a LoweredFile,
    out: Vec<LintDiagnostic>,
    stack: Vec<Frame>,
}

impl Walker<'_> {
    /// Enter a function: push a frame seeded with its parameters (registered
    /// silently — parameters are not `local` declarations), then walk the body.
    fn walk_function(&mut self, body_id: BodyId) {
        let body = self.lowered.body(body_id);
        let mut frame = Frame::new();
        for &param in &body.params {
            let binding = self.lowered.binding(param);
            if !binding.name.is_empty() {
                frame
                    .entry(binding.name.clone())
                    .or_insert_with(|| to_range(binding.range));
            }
        }
        self.stack.push(frame);
        self.walk_block(body_id, &body.block);
        self.stack.pop();
    }

    fn walk_block(&mut self, body_id: BodyId, block: &Block) {
        self.stack.push(Frame::new());
        for &stmt in &block.stmts {
            self.walk_stmt(body_id, stmt);
        }
        self.stack.pop();
    }

    fn walk_stmt(&mut self, body_id: BodyId, stmt: luabox_hir::StmtId) {
        let body = self.lowered.body(body_id);
        match body.stmt(stmt) {
            Stmt::Local { names, init } => {
                for &expr in init {
                    self.walk_expr(body_id, expr);
                }
                for local in names {
                    self.declare(local.binding);
                }
            }
            Stmt::LocalFunction { binding, func } => {
                // The name is in scope inside the body (recursion).
                self.declare(*binding);
                self.walk_expr(body_id, *func);
            }
            Stmt::Assign { targets, values } => {
                for &expr in targets.iter().chain(values) {
                    self.walk_expr(body_id, expr);
                }
            }
            Stmt::ExprStmt(expr) => self.walk_expr(body_id, *expr),
            Stmt::Return(exprs) => {
                for &expr in exprs {
                    self.walk_expr(body_id, expr);
                }
            }
            Stmt::If {
                branches,
                else_block,
            } => {
                for branch in branches {
                    self.walk_expr(body_id, branch.cond);
                    self.walk_block(body_id, &branch.block);
                }
                if let Some(block) = else_block {
                    self.walk_block(body_id, block);
                }
            }
            Stmt::While { cond, body: inner } => {
                self.walk_expr(body_id, *cond);
                self.walk_block(body_id, inner);
            }
            Stmt::Repeat { body: inner, cond } => {
                // The `until` sees the body's locals: one shared frame.
                self.stack.push(Frame::new());
                for &st in &inner.stmts {
                    self.walk_stmt(body_id, st);
                }
                self.walk_expr(body_id, *cond);
                self.stack.pop();
            }
            Stmt::NumericFor {
                var,
                start,
                end,
                step,
                body: inner,
            } => {
                self.walk_expr(body_id, *start);
                self.walk_expr(body_id, *end);
                if let Some(step) = step {
                    self.walk_expr(body_id, *step);
                }
                self.stack.push(Frame::new());
                self.declare(*var);
                for &st in &inner.stmts {
                    self.walk_stmt(body_id, st);
                }
                self.stack.pop();
            }
            Stmt::GenericFor {
                vars,
                exprs,
                body: inner,
            } => {
                for &expr in exprs {
                    self.walk_expr(body_id, expr);
                }
                self.stack.push(Frame::new());
                for &var in vars {
                    self.declare(var);
                }
                for &st in &inner.stmts {
                    self.walk_stmt(body_id, st);
                }
                self.stack.pop();
            }
            Stmt::Do { body: inner } => self.walk_block(body_id, inner),
            Stmt::Break | Stmt::Goto { .. } | Stmt::Label { .. } | Stmt::Error => {}
        }
    }

    fn walk_expr(&mut self, body_id: BodyId, expr: ExprId) {
        let body = self.lowered.body(body_id);
        match body.expr(expr) {
            Expr::Function(child) => self.walk_function(*child),
            other => {
                for child in expr_children(other) {
                    self.walk_expr(body_id, child);
                }
            }
        }
    }

    /// Declare a name in the current (top) frame, flagging a shadow of any
    /// enclosing frame first.
    fn declare(&mut self, binding: BindingId) {
        let decl = self.lowered.binding(binding);
        if decl.name.is_empty() {
            return;
        }
        let name = decl.name.clone();
        let range = to_range(decl.range);
        if let Some(outer) = self.find_outer(&name) {
            self.out.push(
                LintDiagnostic::new(
                    range.clone(),
                    format!("`{name}` shadows a binding from an enclosing scope"),
                )
                .with_secondary(outer, format!("outer `{name}` declared here"))
                .with_note("rename one of them, or reuse the outer binding"),
            );
        }
        self.stack
            .last_mut()
            .expect("a frame is always open")
            .entry(name)
            .or_insert(range);
    }

    /// The declaration range of `name` in any *enclosing* frame (not the
    /// current one — same-block re-locals are allowed).
    fn find_outer(&self, name: &str) -> Option<Range<usize>> {
        let n = self.stack.len();
        if n < 2 {
            return None;
        }
        self.stack[..n - 1]
            .iter()
            .rev()
            .find_map(|frame| frame.get(name).cloned())
    }
}

/// The immediate sub-expressions of `expr` (function bodies are handled
/// separately by the walker, so `Function` yields none here).
fn expr_children(expr: &Expr) -> Vec<ExprId> {
    match expr {
        Expr::Literal(_) | Expr::Name(_) | Expr::Vararg | Expr::Error | Expr::Function(_) => {
            Vec::new()
        }
        Expr::Index { base, index, .. } => vec![*base, *index],
        Expr::Call { callee, args } => {
            let mut v = vec![*callee];
            v.extend(args);
            v
        }
        Expr::MethodCall { receiver, args, .. } => {
            let mut v = vec![*receiver];
            v.extend(args);
            v
        }
        Expr::Table { entries } => entries
            .iter()
            .flat_map(|entry| match entry {
                TableEntry::Positional(v) | TableEntry::Named { value: v, .. } => vec![*v],
                TableEntry::Keyed { key, value } => vec![*key, *value],
            })
            .collect(),
        Expr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Expr::Unary { operand, .. } => vec![*operand],
        Expr::Truncate(inner) => vec![*inner],
    }
}
