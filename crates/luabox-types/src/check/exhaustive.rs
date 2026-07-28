//! Union / enum exhaustiveness — `LB0315` (#121).
//!
//! An `if`/`elseif` chain that dispatches a *finite* discriminant (a union of
//! literal types, or an `---@enum`) and leaves a member uncovered with no
//! `else` branch is a fall-through the author almost certainly did not mean.
//! Everything here is deliberately narrow: the discriminant must be a plain
//! name compared against literals with `==`, and every member of its type must
//! be a literal — anything less certain reports nothing.

use luabox_syntax::lua;
use luabox_syntax::lua::ast::{AstNode, Expr};

use crate::codes::NON_EXHAUSTIVE_IF;
use crate::ty::Ty;

use super::{Checker, is_literal_ty, range};

impl Checker<'_> {
    /// Report `LB0315` when an `if`/`elseif` chain dispatches a *finite*
    /// discriminant (a union of literal types, or an `---@enum`) against
    /// literal cases, has no `else`, and leaves at least one member unhandled
    /// — the luals-style discriminated-union exhaustiveness check (#121).
    ///
    /// The analysis is deliberately narrow: every branch must be exactly
    /// `x == <literal>` (or `<literal> == x`) on the *same* discriminant `x`.
    /// Anything else — a compound `and`/`or` condition, a different variable,
    /// a non-literal comparand, a comparison against a value outside the
    /// finite domain — aborts the whole analysis, so a false positive is never
    /// risked. **Deferred** (SPEC has no obligation here): table-dispatch
    /// exhaustiveness (`{ a = ..., b = ... }` lookup) and `and`/`or` compound
    /// conditions are out of scope; only the simple `==`-chain is analysed.
    pub(super) fn check_exhaustive_if(&mut self, stmt: &lua::ast::IfStmt) {
        // An `else` clause is a catch-all: the chain is exhaustive by
        // construction, whatever the discriminant. Nothing to report.
        if stmt.else_clause().is_some() {
            return;
        }
        // The chain's conditions in order: the `if` test, then each `elseif`
        // test. A missing condition (parse error) aborts the analysis.
        let Some(first) = stmt.condition() else {
            return;
        };
        let mut conditions = vec![first];
        for clause in stmt.elseif_clauses() {
            let Some(cond) = clause.condition() else {
                return;
            };
            conditions.push(cond);
        }

        // Establish the discriminant and its finite member set from the first
        // condition — before any branch narrows it.
        let Some((discriminant, members)) = self.discriminant_of(&conditions[0]) else {
            return;
        };

        // Every branch must compare that same discriminant to a literal that
        // *is* one of the finite members. A branch that is not such a
        // comparison (or compares against a value outside the domain) means
        // the chain is not a pure discriminated dispatch — stay silent.
        let mut covered: Vec<Ty> = Vec::new();
        for cond in &conditions {
            let Some(lit) = self.branch_literal(cond, &discriminant) else {
                return;
            };
            if !members.iter().any(|(_, value)| *value == lit) {
                return;
            }
            covered.push(lit);
        }

        // The members no branch covers (there is provably no `else`).
        let uncovered: Vec<String> = members
            .iter()
            .filter(|(_, value)| !covered.contains(value))
            .map(|(name, _)| format!("`{name}`"))
            .collect();
        if uncovered.is_empty() {
            return; // every member handled — exhaustive
        }
        self.report_full(
            NON_EXHAUSTIVE_IF,
            range(stmt.syntax()),
            format!(
                "non-exhaustive `if` on `{discriminant}`: {} not handled",
                uncovered.join(", "),
            ),
            "add the missing branch(es) or an `else` clause".to_string(),
            Some("the chain dispatches on a finite type and has no `else`".to_string()),
        );
    }

    /// Read the first condition of a candidate chain as a discriminated
    /// dispatch: a binary `==` with one bare-`Name` operand whose type is
    /// finite and one literal operand. Returns the discriminant's name and its
    /// finite member set (`(display name, value type)` pairs).
    fn discriminant_of(&self, cond: &Expr) -> Option<(String, Vec<(String, Ty)>)> {
        let Expr::Bin(bin) = cond else {
            return None;
        };
        if bin.op_token()?.kind() != lua::SyntaxKind::EQ_EQ {
            return None;
        }
        let lhs = bin.lhs()?;
        let rhs = bin.rhs()?;
        // Try each side as the `Name` discriminant; the other must be a
        // literal. Left-hand side is preferred for determinism.
        for (name_side, lit_side) in [(&lhs, &rhs), (&rhs, &lhs)] {
            if let Expr::Name(name) = name_side
                && let Some(token) = name.name()
                && let Some(members) = self.finite_members(&self.expr_ty(name_side))
                && self.literal_ty(lit_side).is_some()
            {
                return Some((token.text().to_string(), members));
            }
        }
        None
    }

    /// The finite member set of a discriminant type, or `None` when the type
    /// is open-ended (contains `string`/`number`/`unknown`/`any` or any
    /// non-literal member) and so must never be diagnosed.
    ///
    /// Two finite shapes: (a) a `---@enum`, enumerated as `Enum.member` named
    /// cases; (b) a `Ty::Union` whose members are *all* literal types.
    fn finite_members(&self, ty: &Ty) -> Option<Vec<(String, Ty)>> {
        // A `---@enum` discriminant: enumerate its members by name so the
        // diagnostic can name the uncovered cases `Enum.member`.
        if let Ty::Named(name) = ty
            && let Some(members) = self.env.enum_members(name)
        {
            if members.is_empty() || !members.iter().all(|(_, v)| is_literal_ty(v)) {
                return None;
            }
            return Some(
                members
                    .into_iter()
                    .map(|(member, value)| (format!("{name}.{member}"), value))
                    .collect(),
            );
        }
        // Otherwise a union of literal types (an alias like `"a"|"b"|"c"`
        // expands to this). A named non-enum (a class) resolves to a table
        // shape, not a union, and is correctly rejected here.
        let resolved = match ty {
            Ty::Named(name) => self.env.resolve_named(name)?,
            other => other.clone(),
        };
        if let Ty::Union(union) = resolved
            && union.iter().all(is_literal_ty)
        {
            return Some(union.iter().map(|v| (v.to_string(), v.clone())).collect());
        }
        None
    }

    /// The literal case a branch covers: `discriminant == <literal>` (or the
    /// mirrored `<literal> == discriminant`) on the given discriminant name.
    /// `None` for any other condition shape.
    fn branch_literal(&self, cond: &Expr, discriminant: &str) -> Option<Ty> {
        let Expr::Bin(bin) = cond else {
            return None;
        };
        if bin.op_token()?.kind() != lua::SyntaxKind::EQ_EQ {
            return None;
        }
        let lhs = bin.lhs()?;
        let rhs = bin.rhs()?;
        let is_discriminant = |expr: &Expr| {
            matches!(expr, Expr::Name(n)
                if n.name().is_some_and(|t| t.text() == discriminant))
        };
        if is_discriminant(&lhs) {
            return self.literal_ty(&rhs);
        }
        if is_discriminant(&rhs) {
            return self.literal_ty(&lhs);
        }
        None
    }

    /// The type of `expr` when it is a literal type (`"a"` / `42` / `true`, or
    /// an `---@enum` member reached as `Enum.member`), else `None`.
    fn literal_ty(&self, expr: &Expr) -> Option<Ty> {
        let ty = self.expr_ty(expr);
        is_literal_ty(&ty).then_some(ty)
    }
}
