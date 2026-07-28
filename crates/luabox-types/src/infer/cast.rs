//! `---@cast x T` and inline `--[[@as T]]` overrides (#73).
//!
//! LuaLS semantics: a `---@cast` attached to a statement takes effect from
//! that point in the flow and wins over the binding's own type, annotated or
//! not. Inline `--[[@as T]]` is the expression-level form — resolved here so
//! [`Infer::eval`] can apply it to whatever the expression evaluated to.

use luabox_hir::{BindingId, Block, BodyId, Expr, ExprId, Stmt, StmtId, TableEntry};
use luabox_syntax::luacats;

use crate::ty::Ty;

use super::{CastTarget, ITy, Infer, ity_union, remove_cast_member};

impl Infer<'_> {
    /// Apply any `---@cast var T` annotations attached to this statement
    /// before walking it (LuaLS semantics: the override holds from this
    /// point in the flow, even for annotated bindings).
    pub(super) fn apply_casts(&mut self, body: BodyId, stmt: StmtId) {
        let Some(key) = self.stmt_range(body, stmt) else {
            return;
        };
        let env = self.env;
        let Some(casts) = env.casts_at(key) else {
            return;
        };
        for cast in casts {
            let target = self.cast_target(body, stmt, &cast.var);
            let current = match &target {
                CastTarget::Binding(id) => self.state.get(id).cloned(),
                CastTarget::Global(name) => self.globals.get(name).cloned(),
            };
            let mut ity = current.unwrap_or_else(ITy::unknown);
            for (kind, ty) in &cast.ops {
                ity = match kind {
                    luacats::CastKind::Replace => ITy::Ty(ty.clone()),
                    luacats::CastKind::Add => ity_union(vec![ity, ITy::Ty(ty.clone())]),
                    luacats::CastKind::Remove => remove_cast_member(&ity, ty),
                };
            }
            match target {
                CastTarget::Binding(id) => {
                    self.state.insert(id, ity);
                }
                CastTarget::Global(name) => {
                    self.globals.insert(name, ity);
                }
            }
        }
    }

    /// Resolve the variable a `---@cast` names: a resolved use inside the
    /// annotated statement when one exists (precise), otherwise the most
    /// recently declared binding of that name (approximate), otherwise a
    /// global.
    fn cast_target(&self, body: BodyId, stmt: StmtId, var: &str) -> CastTarget {
        if let Some(id) = self.find_name_in_stmt(body, stmt, var) {
            return CastTarget::Binding(id);
        }
        let best = self
            .state
            .keys()
            .filter(|id| self.binding(**id).name == var)
            .max_by_key(|id| self.binding(**id).range.start());
        match best {
            Some(&id) => CastTarget::Binding(id),
            None => CastTarget::Global(var.to_string()),
        }
    }

    /// The inline `--[[@as T]]` cast anchored to this expression, if any.
    pub(super) fn as_override(&self, body: BodyId, expr: ExprId) -> Option<Ty> {
        let (_, end) = self.expr_range(body, expr)?;
        self.env.as_cast_at(end).cloned()
    }

    fn find_name_in_stmt(&self, body: BodyId, stmt: StmtId, var: &str) -> Option<BindingId> {
        match self.body(body).stmt(stmt) {
            Stmt::Local { init, .. } => self.find_name_in_exprs(body, init, var),
            Stmt::LocalFunction { func, .. } => self.find_name_in_expr(body, *func, var),
            Stmt::Assign { targets, values } => self
                .find_name_in_exprs(body, targets, var)
                .or_else(|| self.find_name_in_exprs(body, values, var)),
            Stmt::ExprStmt(e) => self.find_name_in_expr(body, *e, var),
            Stmt::Return(exprs) => self.find_name_in_exprs(body, exprs, var),
            Stmt::If {
                branches,
                else_block,
            } => branches
                .iter()
                .find_map(|b| {
                    self.find_name_in_expr(body, b.cond, var)
                        .or_else(|| self.find_name_in_block(body, &b.block, var))
                })
                .or_else(|| {
                    else_block
                        .as_ref()
                        .and_then(|b| self.find_name_in_block(body, b, var))
                }),
            Stmt::While { cond, body: block } => self
                .find_name_in_expr(body, *cond, var)
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::Repeat { body: block, cond } => self
                .find_name_in_block(body, block, var)
                .or_else(|| self.find_name_in_expr(body, *cond, var)),
            Stmt::NumericFor {
                start,
                end,
                step,
                body: block,
                ..
            } => self
                .find_name_in_expr(body, *start, var)
                .or_else(|| self.find_name_in_expr(body, *end, var))
                .or_else(|| step.and_then(|s| self.find_name_in_expr(body, s, var)))
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::GenericFor {
                exprs, body: block, ..
            } => self
                .find_name_in_exprs(body, exprs, var)
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::Do { body: block } => self.find_name_in_block(body, block, var),
            Stmt::Break | Stmt::Goto { .. } | Stmt::Label { .. } | Stmt::Error => None,
        }
    }

    fn find_name_in_block(&self, body: BodyId, block: &Block, var: &str) -> Option<BindingId> {
        block
            .stmts
            .iter()
            .find_map(|&s| self.find_name_in_stmt(body, s, var))
    }

    fn find_name_in_exprs(&self, body: BodyId, exprs: &[ExprId], var: &str) -> Option<BindingId> {
        exprs
            .iter()
            .find_map(|&e| self.find_name_in_expr(body, e, var))
    }

    fn find_name_in_expr(&self, body: BodyId, expr: ExprId, var: &str) -> Option<BindingId> {
        match self.body(body).expr(expr) {
            Expr::Name(name) if name == var => self.name_binding(body, expr),
            Expr::Name(_) | Expr::Literal(_) | Expr::Vararg | Expr::Error => None,
            Expr::Index { base, index, .. } => self
                .find_name_in_expr(body, *base, var)
                .or_else(|| self.find_name_in_expr(body, *index, var)),
            Expr::Call { callee, args } => self
                .find_name_in_expr(body, *callee, var)
                .or_else(|| self.find_name_in_exprs(body, args, var)),
            Expr::MethodCall { receiver, args, .. } => self
                .find_name_in_expr(body, *receiver, var)
                .or_else(|| self.find_name_in_exprs(body, args, var)),
            Expr::Function(fn_body) => {
                // An upvalue use inside a closure resolves to the same
                // binding — still a precise hit.
                let fn_body = *fn_body;
                let block = &self.body(fn_body).block;
                self.find_name_in_block(fn_body, block, var)
            }
            Expr::Table { entries } => entries.iter().find_map(|entry| match entry {
                TableEntry::Positional(v) => self.find_name_in_expr(body, *v, var),
                TableEntry::Named { value, .. } => self.find_name_in_expr(body, *value, var),
                TableEntry::Keyed { key, value } => self
                    .find_name_in_expr(body, *key, var)
                    .or_else(|| self.find_name_in_expr(body, *value, var)),
            }),
            Expr::Binary { lhs, rhs, .. } => self
                .find_name_in_expr(body, *lhs, var)
                .or_else(|| self.find_name_in_expr(body, *rhs, var)),
            Expr::Unary { operand, .. } => self.find_name_in_expr(body, *operand, var),
            Expr::Truncate(inner) => self.find_name_in_expr(body, *inner, var),
        }
    }
}
