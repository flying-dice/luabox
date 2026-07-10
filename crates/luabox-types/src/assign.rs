//! Assignability: may a value of type `V` flow into a slot of type `T`?
//!
//! Decisions (documented per the ticket):
//!
//! - `any` is assignable both ways at every strictness.
//! - `unknown` (the type of unannotated code) is assignable both ways in
//!   warn mode; in strict mode `unknown -> T` is an error (untyped =
//!   `unknown`, not `any`, per SPEC.md §3). `T -> unknown` is always fine.
//! - Literal types are subtypes of their base (`"x" -> string`,
//!   `2 -> number`, integral literals also `-> integer`), and `integer ->
//!   number`.
//! - Tables use **structural width subtyping**: every required
//!   (non-optional, non-nilable) target field must be present with an
//!   assignable type; *extra* fields on the value are fine here. The
//!   "unknown field" diagnostic (LB0303) is only raised for table
//!   *literals* checked against a class shape (LuaLS behaviour), by the
//!   checker — plain assignability stays width-based.
//! - Function-to-function is always accepted — TODO(P1): parameter
//!   contravariance / return covariance.
//! - Named-vs-named comparisons are coinductive (recursive classes
//!   terminate via a seen-pair set).

use crate::env::TypeEnv;
use crate::ty::{TableTy, Ty};

/// Whether `value` may flow into a slot of type `target`.
pub fn assignable(env: &TypeEnv, strict: bool, value: &Ty, target: &Ty) -> bool {
    Ctx {
        env,
        strict,
        seen: Vec::new(),
    }
    .check(value, target)
}

struct Ctx<'a> {
    env: &'a TypeEnv,
    strict: bool,
    /// Named-vs-named pairs already in flight (coinduction guard).
    seen: Vec<(String, String)>,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_lines)]
    fn check(&mut self, value: &Ty, target: &Ty) -> bool {
        // Dynamic escapes first.
        if matches!(value, Ty::Any) || matches!(target, Ty::Any | Ty::Unknown) {
            return true;
        }
        if matches!(value, Ty::Unknown) {
            return !self.strict;
        }

        // Same-name fast path + coinduction guard for recursive classes.
        if let (Ty::Named(v), Ty::Named(t)) = (value, target) {
            if v == t {
                return true;
            }
            let pair = (v.clone(), t.clone());
            if self.seen.contains(&pair) {
                return true;
            }
            self.seen.push(pair);
        }

        // A union value must fit wholly; a union target accepts any member.
        if let Ty::Union(members) = value {
            return members.iter().all(|m| self.check(m, target));
        }
        if let Ty::Union(members) = target {
            return members.iter().any(|m| self.check(value, m));
        }

        // Resolve named references to their structural types.
        if let Ty::Named(name) = value {
            return match self.env.resolve_named(name) {
                Some(resolved) => self.check(&resolved, target),
                None => !self.strict, // undeclared: treated as unknown
            };
        }
        if let Ty::Named(name) = target {
            return match self.env.resolve_named(name) {
                Some(resolved) => self.check(value, &resolved),
                None => true,
            };
        }

        match (value, target) {
            // Identical primitives / literals.
            (v, t) if v == t => true,
            // Literal subtyping — plus integer -> number widening and,
            // TODO(P1) function subtyping (param contravariance, return
            // covariance): any function satisfies a function slot for now.
            (Ty::BoolLit(_), Ty::Boolean)
            | (Ty::StringLit(_), Ty::String)
            | (Ty::NumberLit(_) | Ty::Integer, Ty::Number)
            | (Ty::Function(_), Ty::Function(_)) => true,
            (Ty::NumberLit(text), Ty::Integer) => is_integral_literal(text),
            (Ty::NumberLit(a), Ty::NumberLit(b)) => numeric_eq(a, b),
            (Ty::Table(v), Ty::Table(t)) => self.check_table(v, t),
            _ => false,
        }
    }

    /// Structural width subtyping for tables.
    fn check_table(&mut self, value: &TableTy, target: &TableTy) -> bool {
        for (name, field) in &target.fields {
            match value.fields.get(name) {
                Some(actual) => {
                    let mut expected = field.ty.clone();
                    if field.optional {
                        expected = expected.optional();
                    }
                    if !self.check(&actual.ty, &expected) {
                        return false;
                    }
                }
                None => {
                    if !field.optional && !field.ty.admits_nil() {
                        return false;
                    }
                }
            }
        }
        if let (Some(velem), Some(telem)) = (&value.array, &target.array)
            && !self.check(velem, telem)
        {
            return false;
        }
        // Like-for-like indexer check: a value indexer must satisfy any
        // target indexer its keys can reach. (Extra value fields are fine —
        // width subtyping; literal-level strictness lives in the checker.)
        for (tkey, tvalue) in &target.indexers {
            for (vkey, vvalue) in &value.indexers {
                if self.check(vkey, tkey) && !self.check(vvalue, tvalue) {
                    return false;
                }
            }
        }
        true
    }
}

/// A short human explanation of why `value` does not fit `target`, when
/// both resolve to table shapes: the missing required members, then the
/// members whose types mismatch (SHAPES-V2.md: assignability errors must
/// name the members, not just the types). `None` when there is no
/// table-level story to tell.
pub(crate) fn explain_mismatch(
    env: &TypeEnv,
    strict: bool,
    value: &Ty,
    target: &Ty,
) -> Option<String> {
    let value_table = resolve_table(env, value)?;
    let target_table = resolve_table(env, target)?;
    let mut missing: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    for (name, field) in &target_table.fields {
        match value_table.fields.get(name) {
            None => {
                if !field.optional && !field.ty.admits_nil() {
                    missing.push(format!("`{name}`"));
                }
            }
            Some(actual) => {
                let mut expected = field.ty.clone();
                if field.optional {
                    expected = expected.optional();
                }
                if !assignable(env, strict, &actual.ty, &expected) {
                    wrong.push(format!(
                        "`{name}` (expected `{expected}`, found `{}`)",
                        actual.ty
                    ));
                }
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing {}", missing.join(", ")));
    }
    if !wrong.is_empty() {
        parts.push(format!("mismatched {}", wrong.join(", ")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// Resolve a type to its structural table shape, unwrapping `T?` optionals
/// and following `Ty::Named` through the environment.
fn resolve_table(env: &TypeEnv, ty: &Ty) -> Option<TableTy> {
    match ty {
        Ty::Table(table) => Some((**table).clone()),
        Ty::Named(name) => match env.resolve_named(name)? {
            Ty::Table(table) => Some(*table),
            _ => None,
        },
        Ty::Union(members) => {
            let non_nil: Vec<&Ty> = members.iter().filter(|m| **m != Ty::Nil).collect();
            match non_nil[..] {
                [single] => resolve_table(env, single),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Whether a number-literal's text denotes an integer value.
pub(crate) fn is_integral_literal(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if let Some(hex) = lower
        .strip_prefix("0x")
        .or_else(|| lower.strip_prefix("-0x"))
    {
        !hex.contains('.') && !hex.contains('p')
    } else {
        !lower.contains('.') && !lower.contains('e')
    }
}

/// Numeric equality of two number-literal texts (decimal), falling back to
/// text equality (covers hex and exotic forms).
fn numeric_eq(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::ty::{FieldTy, FunctionTy, TableTy};

    fn env() -> TypeEnv {
        TypeEnv::default()
    }

    fn ok(value: &Ty, target: &Ty) {
        assert!(
            assignable(&env(), true, value, target),
            "{value} should be assignable to {target}"
        );
    }

    fn no(value: &Ty, target: &Ty) {
        assert!(
            !assignable(&env(), true, value, target),
            "{value} should NOT be assignable to {target}"
        );
    }

    fn table(fields: &[(&str, Ty, bool)]) -> Ty {
        let fields: BTreeMap<String, FieldTy> = fields
            .iter()
            .map(|(name, ty, optional)| {
                (
                    (*name).to_string(),
                    FieldTy {
                        ty: ty.clone(),
                        optional: *optional,
                    },
                )
            })
            .collect();
        Ty::Table(Box::new(TableTy {
            fields,
            ..TableTy::default()
        }))
    }

    #[test]
    fn any_flows_both_ways() {
        ok(&Ty::Any, &Ty::Number);
        ok(&Ty::Number, &Ty::Any);
    }

    #[test]
    fn unknown_depends_on_strictness() {
        // Strict: unknown -> T is an error; T -> unknown is fine.
        no(&Ty::Unknown, &Ty::Number);
        ok(&Ty::Number, &Ty::Unknown);
        // Warn: both ways.
        assert!(assignable(&env(), false, &Ty::Unknown, &Ty::Number));
    }

    #[test]
    fn literal_subtyping() {
        ok(&Ty::StringLit("x".into()), &Ty::String);
        ok(&Ty::NumberLit("2".into()), &Ty::Number);
        ok(&Ty::NumberLit("2".into()), &Ty::Integer);
        no(&Ty::NumberLit("2.5".into()), &Ty::Integer);
        ok(&Ty::NumberLit("0x1F".into()), &Ty::Integer);
        ok(&Ty::BoolLit(true), &Ty::Boolean);
        ok(&Ty::Integer, &Ty::Number);
        no(&Ty::Number, &Ty::Integer);
        no(&Ty::String, &Ty::StringLit("x".into()));
        ok(&Ty::NumberLit("2.0".into()), &Ty::NumberLit("2".into()));
    }

    #[test]
    fn unions() {
        let num_or_str = Ty::Union(vec![Ty::Number, Ty::String]);
        ok(&Ty::Number, &num_or_str);
        ok(&Ty::StringLit("s".into()), &num_or_str);
        no(&Ty::Boolean, &num_or_str);
        // A union value fits only if every member fits.
        no(&num_or_str, &Ty::Number);
        ok(
            &num_or_str,
            &Ty::Union(vec![Ty::String, Ty::Number, Ty::Nil]),
        );
        // nil into an optional.
        ok(&Ty::Nil, &Ty::Number.optional());
        no(&Ty::Nil, &Ty::Number);
    }

    #[test]
    fn width_subtyping_extra_fields_ok() {
        let target = table(&[("x", Ty::Number, false)]);
        let value = table(&[
            ("x", Ty::NumberLit("1".into()), false),
            ("y", Ty::String, false),
        ]);
        ok(&value, &target);
    }

    #[test]
    fn missing_required_field_fails() {
        let target = table(&[("x", Ty::Number, false), ("y", Ty::Number, false)]);
        let value = table(&[("x", Ty::Number, false)]);
        no(&value, &target);
    }

    #[test]
    fn missing_optional_field_ok() {
        let target = table(&[("x", Ty::Number, false), ("y", Ty::Number, true)]);
        let value = table(&[("x", Ty::Number, false)]);
        ok(&value, &target);
    }

    #[test]
    fn field_type_mismatch_fails() {
        let target = table(&[("x", Ty::Number, false)]);
        let value = table(&[("x", Ty::String, false)]);
        no(&value, &target);
    }

    #[test]
    fn any_table_accepts_every_table() {
        ok(&table(&[("x", Ty::Number, false)]), &Ty::any_table());
        no(&Ty::Number, &Ty::any_table());
    }

    #[test]
    fn array_element_types_checked() {
        let strings = Ty::Table(Box::new(TableTy {
            array: Some(Ty::String),
            ..TableTy::default()
        }));
        let string_lits = Ty::Table(Box::new(TableTy {
            array: Some(Ty::StringLit("a".into())),
            ..TableTy::default()
        }));
        let numbers = Ty::Table(Box::new(TableTy {
            array: Some(Ty::Number),
            ..TableTy::default()
        }));
        ok(&string_lits, &strings);
        no(&numbers, &strings);
        // An empty table is an empty array.
        ok(&Ty::Table(Box::default()), &strings);
    }

    #[test]
    fn functions_are_mutually_assignable_mvp() {
        let f = Ty::Function(Box::new(FunctionTy::opaque()));
        let g = Ty::Function(Box::default());
        ok(&f, &g);
        no(&f, &Ty::Number);
        no(&Ty::Number, &f);
    }
}
