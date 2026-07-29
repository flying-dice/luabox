//! Branching and flow-sensitive narrowing.
//!
//! Branch arms are walked on cloned states and merged by union at the join; an
//! arm that ends in `return`/`break` contributes nothing, which is what makes
//! early-return narrowing fall out naturally. The predicates ([`Pred`]) cover
//! `type(x) == "..."`, truthiness, `x == nil` / `x ~= nil`, literal equality,
//! `not`, and `and`/`or` combinations — all union-based and intraprocedural.

use std::collections::HashMap;

use luabox_hir::{
    BinOp, BindingId, Block, BodyId, Expr, ExprId, IfBranch, Literal, Resolution, Stmt, UnOp,
};

use crate::ty::Ty;

use super::{
    ITy, Infer, Pred, ity_members, ity_union, literal_ty, narrow_type_is, type_base, type_name,
};

impl Infer<'_> {
    pub(super) fn walk_if(
        &mut self,
        body: BodyId,
        branches: &[IfBranch],
        else_block: Option<&Block>,
    ) {
        let mut outs: Vec<HashMap<BindingId, ITy>> = Vec::new();
        // The running "no branch so far was taken" state.
        let mut fallthrough = self.state.clone();
        for branch in branches {
            self.state = fallthrough.clone();
            self.eval(body, branch.cond);
            self.apply_narrows(body, branch.cond, true);
            self.walk_block(body, &branch.block);
            if !self.block_terminates(body, &branch.block) {
                outs.push(std::mem::take(&mut self.state));
            }
            // Later arms (and the code after the `if`) know this
            // condition was false.
            self.state = fallthrough;
            self.apply_narrows(body, branch.cond, false);
            fallthrough = std::mem::take(&mut self.state);
        }
        if let Some(block) = else_block {
            self.state = fallthrough;
            self.walk_block(body, block);
            if !self.block_terminates(body, block) {
                outs.push(std::mem::take(&mut self.state));
            }
        } else {
            outs.push(fallthrough);
        }
        self.merge_states(outs);
    }

    /// Union-merge branch-exit states into `self.state`.
    pub(super) fn merge_states(&mut self, outs: Vec<HashMap<BindingId, ITy>>) {
        let mut iter = outs.into_iter();
        let Some(mut merged) = iter.next() else {
            // Every path terminated: keep the entry state (dead code after).
            return;
        };
        for out in iter {
            for (id, ity) in out {
                match merged.get(&id) {
                    Some(existing) => {
                        let union = ity_union(vec![existing.clone(), ity]);
                        merged.insert(id, union);
                    }
                    None => {
                        merged.insert(id, ity);
                    }
                }
            }
        }
        self.state = merged;
    }

    fn block_terminates(&self, body: BodyId, block: &Block) -> bool {
        block.stmts.last().is_some_and(|&stmt| {
            matches!(
                self.body(body).stmt(stmt),
                Stmt::Return(_) | Stmt::Break | Stmt::Goto { .. }
            )
        })
    }

    pub(super) fn apply_narrows(&mut self, body: BodyId, cond: ExprId, positive: bool) {
        let mut preds: Vec<(BindingId, Pred)> = Vec::new();
        self.cond_narrows(body, cond, positive, &mut preds);
        for (binding, pred) in preds {
            if let Some(current) = self.state.get(&binding) {
                let narrowed = self.narrow(&current.clone(), &pred);
                self.state.insert(binding, narrowed);
            }
        }
    }

    /// Derive narrowing predicates from a condition (`positive` = the
    /// branch where the condition held).
    fn cond_narrows(
        &self,
        body: BodyId,
        cond: ExprId,
        positive: bool,
        out: &mut Vec<(BindingId, Pred)>,
    ) {
        match self.body(body).expr(cond) {
            Expr::Name(_) => {
                if let Some(binding) = self.name_binding(body, cond) {
                    out.push((binding, if positive { Pred::Truthy } else { Pred::Falsy }));
                }
            }
            Expr::Truncate(inner) => self.cond_narrows(body, *inner, positive, out),
            Expr::Unary {
                op: UnOp::Not,
                operand,
            } => self.cond_narrows(body, *operand, !positive, out),
            Expr::Binary { op, lhs, rhs } => match op {
                BinOp::And if positive => {
                    self.cond_narrows(body, *lhs, true, out);
                    self.cond_narrows(body, *rhs, true, out);
                }
                BinOp::Or if !positive => {
                    self.cond_narrows(body, *lhs, false, out);
                    self.cond_narrows(body, *rhs, false, out);
                }
                BinOp::Eq | BinOp::Ne => {
                    let holds = (*op == BinOp::Eq) == positive;
                    self.eq_narrows(body, *lhs, *rhs, holds, out);
                    self.eq_narrows(body, *rhs, *lhs, holds, out);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Narrowing from `subject ==/~= probe` where `probe` is a literal, or
    /// `subject` is a `type(x)` call compared to a type-name string.
    fn eq_narrows(
        &self,
        body: BodyId,
        subject: ExprId,
        probe: ExprId,
        holds: bool,
        out: &mut Vec<(BindingId, Pred)>,
    ) {
        // `type(x) == "string"`.
        if let Expr::Call { callee, args } = self.body(body).expr(subject)
            && matches!(self.body(body).expr(*callee), Expr::Name(n) if n == "type")
            && matches!(self.resolution(body, *callee), Some(Resolution::Global(_)))
            && let [arg] = args[..]
            && let Some(binding) = self.name_binding(body, arg)
            && let Expr::Literal(Literal::String(s)) = self.body(body).expr(probe)
            && let Some(name) = s.as_str().and_then(type_name)
        {
            out.push((
                binding,
                if holds {
                    Pred::TypeIs(name)
                } else {
                    Pred::NotTypeIs(name)
                },
            ));
            return;
        }
        // `x == nil` / `x == <literal>`.
        if let Some(binding) = self.name_binding(body, subject)
            && let Expr::Literal(lit) = self.body(body).expr(probe)
        {
            let pred = match (lit, holds) {
                (Literal::Nil, true) => Pred::Nil,
                (Literal::Nil, false) => Pred::NonNil,
                (other, true) => Pred::Lit(literal_ty(other)),
                (other, false) => Pred::NotLit(literal_ty(other)),
            };
            out.push((binding, pred));
        }
    }

    pub(super) fn name_binding(&self, body: BodyId, expr: ExprId) -> Option<BindingId> {
        if !matches!(self.body(body).expr(expr), Expr::Name(_)) {
            return None;
        }
        match self.resolution(body, expr) {
            Some(Resolution::Local(id)) => Some(*id),
            Some(Resolution::Upvalue { binding, .. }) => Some(*binding),
            _ => None,
        }
    }

    /// Apply a predicate to an inference type (union-filtering).
    pub(super) fn narrow(&self, ity: &ITy, pred: &Pred) -> ITy {
        let members = ity_members(ity);
        let mut kept: Vec<ITy> = Vec::new();
        for member in members {
            match pred {
                Pred::Truthy => match &member {
                    ITy::Ty(Ty::Nil | Ty::BoolLit(false)) => {}
                    ITy::Ty(Ty::Boolean) => kept.push(ITy::Ty(Ty::BoolLit(true))),
                    ITy::Ty(Ty::Union(inner)) => {
                        let inner: Vec<Ty> = inner
                            .iter()
                            .filter(|t| !matches!(t, Ty::Nil | Ty::BoolLit(false)))
                            .cloned()
                            .collect();
                        if !inner.is_empty() {
                            kept.push(ITy::Ty(Ty::union(inner)));
                        }
                    }
                    _ => kept.push(member),
                },
                Pred::Falsy => match &member {
                    ITy::Ty(Ty::Nil | Ty::BoolLit(false)) => kept.push(member),
                    ITy::Ty(Ty::Boolean) => kept.push(ITy::Ty(Ty::BoolLit(false))),
                    ITy::Ty(Ty::Unknown | Ty::Any) => {
                        kept.push(ITy::Ty(Ty::union(vec![Ty::Nil, Ty::BoolLit(false)])));
                    }
                    ITy::Ty(Ty::Union(inner)) => {
                        if inner.contains(&Ty::Nil) {
                            kept.push(ITy::Ty(Ty::Nil));
                        }
                        if inner.contains(&Ty::BoolLit(false)) || inner.contains(&Ty::Boolean) {
                            kept.push(ITy::Ty(Ty::BoolLit(false)));
                        }
                    }
                    _ => {}
                },
                Pred::Nil => match &member {
                    ITy::Ty(Ty::Nil | Ty::Unknown | Ty::Any) => kept.push(ITy::Ty(Ty::Nil)),
                    ITy::Ty(Ty::Union(inner)) if inner.contains(&Ty::Nil) => {
                        kept.push(ITy::Ty(Ty::Nil));
                    }
                    _ => {}
                },
                Pred::NonNil => match &member {
                    ITy::Ty(Ty::Nil) => {}
                    ITy::Ty(Ty::Union(inner)) => {
                        let inner: Vec<Ty> = inner
                            .iter()
                            .filter(|t| !matches!(t, Ty::Nil))
                            .cloned()
                            .collect();
                        if !inner.is_empty() {
                            kept.push(ITy::Ty(Ty::union(inner)));
                        }
                    }
                    _ => kept.push(member),
                },
                Pred::TypeIs(name) => {
                    if let Some(narrowed) = narrow_type_is(&member, name) {
                        kept.push(narrowed);
                    }
                }
                Pred::NotTypeIs(name) => {
                    if narrow_type_is(&member, name).is_none() || member.is_unknown() {
                        kept.push(member);
                    }
                }
                Pred::Lit(lit) => match &member {
                    ITy::Ty(Ty::Unknown | Ty::Any) => kept.push(ITy::Ty(lit.clone())),
                    ITy::Ty(ty) => {
                        if crate::assign::assignable(
                            self.env,
                            crate::assign::Exactness::Loose,
                            lit,
                            ty,
                        ) {
                            kept.push(ITy::Ty(lit.clone()));
                        }
                    }
                    _ => {}
                },
                Pred::NotLit(lit) => {
                    if member != ITy::Ty(lit.clone()) {
                        kept.push(member);
                    }
                }
            }
        }
        if kept.is_empty() {
            // The branch is (statically) impossible; degrade gracefully.
            match pred {
                Pred::Nil => ITy::Ty(Ty::Nil),
                Pred::TypeIs(name) => ITy::Ty(type_base(name)),
                _ => ITy::unknown(),
            }
        } else {
            ity_union(kept)
        }
    }
}
