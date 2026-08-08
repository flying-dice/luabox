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
//! - Function-to-function is real subtyping: parameters are contravariant
//!   (with Lua call semantics — a value may declare fewer parameters, never
//!   more *required* ones), returns are covariant. `unknown` parameters and
//!   unannotated returns stay lenient in both directions; `FunctionTy::opaque`
//!   is universally assignable. See [`Ctx::check_function`].
//! - Named-vs-named comparisons are coinductive (recursive classes
//!   terminate via a seen-pair set).

use crate::env::TypeEnv;
use crate::ty::{FunctionTy, TableTy, Ty};

/// How exactly a value must fit its target slot — the assignability knob the
/// strictness ladder's strict flag selects. Distinct from [`crate::Strictness`]
/// (which also has a `None` level that never reaches assignability): here the
/// only question is whether `unknown` is treated as `any` (loose) or as a
/// genuine unknown that does not flow into a typed slot (strict).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exactness {
    /// Warn mode: `unknown` is assignable both ways, undeclared names pass.
    Loose,
    /// Strict mode: `unknown -> T` is a mismatch, undeclared value names fail.
    Strict,
}

impl Exactness {
    /// The exactness the strictness ladder's boolean strict flag selects.
    #[must_use]
    pub(crate) fn from_strict(strict: bool) -> Self {
        if strict {
            Exactness::Strict
        } else {
            Exactness::Loose
        }
    }

    /// Whether this is the strict end of the ladder.
    pub(crate) fn is_strict(self) -> bool {
        matches!(self, Exactness::Strict)
    }
}

/// Whether `value` may flow into a slot of type `target`.
pub fn assignable(env: &TypeEnv, exact: Exactness, value: &Ty, target: &Ty) -> bool {
    Ctx {
        env,
        exact,
        seen: Vec::new(),
    }
    .check(value, target)
}

struct Ctx<'a> {
    env: &'a TypeEnv,
    exact: Exactness,
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
            return !self.exact.is_strict();
        }

        // Same-name fast path + coinduction guard for recursive classes.
        if let (Ty::Named(v), Ty::Named(t)) = (value, target) {
            if v == t {
                return true;
            }
            // Declared ancestry is nominal: `---@class Child : Parent` makes
            // a `Child` flow wherever a `Parent` is demanded, whatever the
            // merged shapes say. Without this the two are compared
            // structurally, so a subclass that *overrides* an inherited
            // member at an incompatible type — the whole point of overriding
            // — fails the upcast its own declaration promises.
            //
            // Measured against lua-language-server 3.13.5, which resolves
            // this nominally: `Child : Parent` with `---@field v number` over
            // `Parent`'s `---@field v string` passes `takes(child)` there and
            // is rejected here. This restores that half.
            //
            // Strictly a loosening — it only ever turns a rejection into an
            // acceptance, so it cannot introduce a false positive. The
            // *tightening* half of nominality (luals also rejects an
            // undeclared class that happens to match structurally, where
            // luabox accepts it) is a core semantic change to every LB0300
            // and is deliberately NOT made here — see #68.
            //
            // The declaration-site obligation is unaffected:
            // `Checker::check_class_conformance` still verifies a `---@class
            // Name : Parent` carrier against every member `Parent` declares,
            // so an override that does not satisfy its base is still
            // reported — at the declaration, which is where luals-style
            // nominality wants it, rather than at every use.
            //
            // Ordered AFTER the coinduction guard, not before it (local
            // merge-gate finding): `is_subclass` runs its own
            // `walk_ancestor_names` and memoises nothing across calls, so
            // asking it first made every re-entry on an already-in-flight
            // pair re-walk the ancestry that the `seen` short-circuit answers
            // in O(1). Structurally recursive named types (`A.next: B`,
            // `B.next: A`, unrelated by declared ancestry) re-paid that walk
            // at every nesting level. The guard is a pure short-circuit —
            // returning `true` for a pair already in flight — so consulting
            // it first cannot change any answer, only skip work.
            let pair = (v.clone(), t.clone());
            if self.seen.contains(&pair) {
                return true;
            }
            if self.env.is_subclass(v, t) {
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
                None => !self.exact.is_strict(), // undeclared: treated as unknown
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
            // Literal subtyping plus integer -> number widening.
            (Ty::BoolLit(_), Ty::Boolean)
            | (Ty::StringLit(_), Ty::String)
            | (Ty::NumberLit(_) | Ty::Integer, Ty::Number) => true,
            (Ty::NumberLit(text), Ty::Integer) => is_integral_literal(text),
            (Ty::NumberLit(a), Ty::NumberLit(b)) => numeric_eq(a, b),
            (Ty::Function(v), Ty::Function(t)) => self.check_function(v, t),
            (Ty::Table(v), Ty::Table(t)) => self.check_table(v, t),
            _ => false,
        }
    }

    /// Function subtyping: the `value` function may flow where a `target`
    /// function is demanded. Contravariant parameters, covariant returns,
    /// Lua call semantics. The value fits if its primary signature fits or
    /// any of its overloads fits the target's primary (target overloads are
    /// ignored — conservative).
    fn check_function(&mut self, value: &FunctionTy, target: &FunctionTy) -> bool {
        if self.check_function_sig(value, target) {
            return true;
        }
        value
            .overloads
            .iter()
            .any(|overload| self.check_function_sig(overload, target))
    }

    /// One value signature against the target's primary signature.
    fn check_function_sig(&mut self, value: &FunctionTy, target: &FunctionTy) -> bool {
        // Parameters are contravariant: for each positional parameter the
        // value declares, the corresponding target parameter (its fixed
        // parameter, else its vararg element) must be assignable *to* the
        // value's parameter (target -> value). `unknown` parameters are
        // lenient in both directions regardless of strict mode.
        for (i, vparam) in value.params.iter().enumerate() {
            let Some(tty) = target
                .params
                .get(i)
                .map(|p| &p.ty)
                .or(target.varargs.as_ref())
            else {
                continue;
            };
            if lenient_param(&vparam.ty) || lenient_param(tty) {
                continue;
            }
            if !self.check(tty, &vparam.ty) {
                return false;
            }
        }
        // The value must not *require* more than the target supplies. Lua
        // silently drops extra call arguments, so the value declaring fewer
        // (or optional / nil-admitting) parameters is safe; a required value
        // parameter beyond the target's fixed arity — with no target vararg
        // to feed it — is not.
        if target.varargs.is_none() {
            for vparam in value.params.iter().skip(target.params.len()) {
                if !vparam.optional && !vparam.ty.admits_nil() {
                    return false;
                }
            }
        }
        // A value vararg absorbs any target parameters past the value's fixed
        // list: each must be assignable to the vararg element (an `unknown` /
        // `any` element accepts everything).
        if let Some(velem) = &value.varargs
            && !lenient_param(velem)
        {
            for tparam in target.params.iter().skip(value.params.len()) {
                if !self.check(&tparam.ty, velem) {
                    return false;
                }
            }
        }
        // Returns are covariant. An unannotated value (`has_return_annotation
        // == false`) is not checked — mirrors the checker's stance that
        // returns without `---@return` are unconstrained. Extra value returns
        // are ignored (Lua callers drop them); a target return the value does
        // not provide must admit nil, unless the target return list is
        // open-ended (`returns_vararg`).
        if value.has_return_annotation {
            for (i, tret) in target.returns.iter().enumerate() {
                match value.returns.get(i) {
                    Some(vret) => {
                        if !self.check(vret, tret) {
                            return false;
                        }
                    }
                    None => {
                        if !target.returns_vararg && !tret.admits_nil() {
                            return false;
                        }
                    }
                }
            }
        }
        true
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
/// members whose types mismatch (assignability errors name the members, not
/// just the types). `None` when there is no table-level story to tell.
pub(crate) fn explain_mismatch(
    env: &TypeEnv,
    exact: Exactness,
    value: &Ty,
    target: &Ty,
) -> Option<String> {
    let value_table = resolve_table(env, value)?;
    let target_table = resolve_table(env, target)?;
    let attached = attached_member_names(env, target);
    let mut missing: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    for (name, field) in &target_table.fields {
        match value_table.fields.get(name) {
            None => {
                if !field.optional && !field.ty.admits_nil() && !attached.contains(name) {
                    missing.push(format!("`{name}`"));
                }
            }
            Some(actual) => {
                let mut expected = field.ty.clone();
                if field.optional {
                    expected = expected.optional();
                }
                if !assignable(env, exact, &actual.ty, &expected) {
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

/// How a table *literal* immediately conforms to a target object type — the
/// discriminator behind whole-carrier `---@type` deferral.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiteralConformance {
    /// Every required member present and correctly typed, no undeclared keys.
    Conforms,
    /// The *only* gap is missing required members — a carrier still being
    /// built. Whole-carrier conformance is deferred to the final shape.
    MissingOnly,
    /// A present member has the wrong type, or a closed target rejects an
    /// undeclared key — a real mistake, reported immediately (never deferred).
    Other,
}

/// Classify a table literal's immediate conformance to `target`, resolved to
/// its object shape. `None` when `target` is not an object type (no deferral
/// question to answer). Mirrors [`super::check`]'s literal-freshness split:
/// missing required fields, mismatched present fields, and undeclared keys
/// (excess) on a closed target.
pub(crate) fn classify_literal(
    env: &TypeEnv,
    exact: Exactness,
    literal: &TableTy,
    target: &Ty,
) -> Option<LiteralConformance> {
    let attached = attached_member_names(env, target);
    let target = resolve_table(env, target)?;
    let mut missing = false;
    let mut other = false;
    for (name, field) in &target.fields {
        match literal.fields.get(name) {
            None => {
                if !field.optional && !field.ty.admits_nil() && !attached.contains(name) {
                    missing = true;
                }
            }
            Some(actual) => {
                let mut expected = field.ty.clone();
                if field.optional {
                    expected = expected.optional();
                }
                if !assignable(env, exact, &actual.ty, &expected) {
                    other = true;
                }
            }
        }
    }
    // Undeclared keys the closed target neither declares nor reaches by an
    // indexer are freshness violations (excess) — an immediate error.
    for name in literal.fields.keys() {
        if target.fields.contains_key(name) {
            continue;
        }
        let key = Ty::StringLit(name.clone());
        if !target
            .indexers
            .iter()
            .any(|(k, _)| assignable(env, exact, &key, k))
        {
            other = true;
        }
    }
    Some(if other {
        LiteralConformance::Other
    } else if missing {
        LiteralConformance::MissingOnly
    } else {
        LiteralConformance::Conforms
    })
}

/// The carrier-attached member names of a (possibly named) target type —
/// the members [`TypeEnv::class_method_names`] exempts from table-literal
/// obligations (luals `missing-fields` parity). Empty for structural
/// targets; unions defer to their non-`nil` named member, mirroring
/// [`resolve_table`]'s unwrapping.
fn attached_member_names(env: &TypeEnv, ty: &Ty) -> std::collections::HashSet<String> {
    match ty {
        Ty::Named(name) => env.class_method_names(name),
        Ty::Union(members) => {
            let non_nil: Vec<&Ty> = members.iter().filter(|m| **m != Ty::Nil).collect();
            match non_nil[..] {
                [single] => attached_member_names(env, single),
                _ => std::collections::HashSet::new(),
            }
        }
        _ => std::collections::HashSet::new(),
    }
}

/// Resolve a type to its structural table shape, unwrapping `T?` optionals
/// and following `Ty::Named` through the environment.
///
/// Deliberately the raw (non-erasing) resolution: [`classify_literal`] runs
/// `field.optional`/`field.ty.admits_nil()` against this shape to decide
/// whether a table literal is genuinely missing a required member, and
/// erasing an unbound generic parameter to `unknown` here would make it
/// `admits_nil() == true` — silently reclassifying a literal that is
/// actually missing a required member as fully conforming (production
/// readiness review finding 5's first attempt, measured and reverted; see
/// `check::conformance`'s and `Checker::table_shape`'s doc comments for the
/// same regression measured at their own call sites). `explain_mismatch`'s
/// own text never actually names this leaked parameter: its "missing" list
/// prints field names only, and its "wrong type" list can't trigger on a
/// leaf gated by an unbound parameter — `assignable` already treats an
/// undeclared bare name exactly like `unknown`, matching either way.
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

/// A function parameter that never constrains conformance: `unknown`
/// (unannotated Lua) and `any` are lenient in both directions, at every
/// strictness.
fn lenient_param(ty: &Ty) -> bool {
    matches!(ty, Ty::Unknown | Ty::Any)
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
    use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy};

    fn env() -> TypeEnv {
        TypeEnv::default()
    }

    fn ok(value: &Ty, target: &Ty) {
        assert!(
            assignable(&env(), Exactness::Strict, value, target),
            "{value} should be assignable to {target}"
        );
    }

    fn no(value: &Ty, target: &Ty) {
        assert!(
            !assignable(&env(), Exactness::Strict, value, target),
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
        assert!(assignable(
            &env(),
            Exactness::Loose,
            &Ty::Unknown,
            &Ty::Number
        ));
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

    /// Build a function type: `(name, type, optional)` params, return types,
    /// and whether a `---@return` was written.
    fn func(params: &[(&str, Ty, bool)], returns: &[Ty], has_return: bool) -> Ty {
        Ty::Function(Box::new(FunctionTy {
            params: params
                .iter()
                .map(|(name, ty, optional)| ParamTy {
                    name: (*name).to_string(),
                    ty: ty.clone(),
                    optional: *optional,
                })
                .collect(),
            returns: returns.to_vec(),
            has_return_annotation: has_return,
            ..FunctionTy::default()
        }))
    }

    #[test]
    fn functions_are_not_other_types() {
        let f = Ty::Function(Box::new(FunctionTy::opaque()));
        no(&f, &Ty::Number);
        no(&Ty::Number, &f);
    }

    #[test]
    fn function_param_contravariance() {
        // A wider value parameter accepts the target's narrower one.
        ok(
            &func(&[("x", Ty::Number, false)], &[], false),
            &func(&[("x", Ty::Integer, false)], &[], false),
        );
        // A narrower value parameter cannot accept the target's wider one.
        no(
            &func(&[("x", Ty::Integer, false)], &[], false),
            &func(&[("x", Ty::Number, false)], &[], false),
        );
    }

    #[test]
    fn function_fewer_value_params_ok() {
        // Lua ignores extra call arguments: the value may take fewer.
        ok(
            &func(&[], &[], false),
            &func(&[("self", Ty::Named("C".into()), false)], &[], false),
        );
    }

    #[test]
    fn function_extra_required_value_param_fails() {
        // A required value parameter beyond the target's arity is unsafe.
        no(
            &func(
                &[("a", Ty::Number, false), ("b", Ty::Number, false)],
                &[],
                false,
            ),
            &func(&[("a", Ty::Number, false)], &[], false),
        );
        // …but an *optional* extra value parameter is fine.
        ok(
            &func(
                &[("a", Ty::Number, false), ("b", Ty::Number, true)],
                &[],
                false,
            ),
            &func(&[("a", Ty::Number, false)], &[], false),
        );
    }

    #[test]
    fn function_return_covariance() {
        // string is not a number in return position.
        no(
            &func(&[], &[Ty::String], true),
            &func(&[], &[Ty::Number], true),
        );
        // A literal return widens to the target's primitive.
        ok(
            &func(&[], &[Ty::NumberLit("2".into())], true),
            &func(&[], &[Ty::Number], true),
        );
    }

    #[test]
    fn function_unannotated_returns_are_lenient() {
        // Without `---@return`, the value's returns are not checked.
        ok(
            &func(&[], &[Ty::String], false),
            &func(&[], &[Ty::Number], true),
        );
    }

    #[test]
    fn function_missing_target_return_must_admit_nil() {
        // The value provides no second return; the target's must admit nil.
        ok(
            &func(&[], &[Ty::Number], true),
            &func(&[], &[Ty::Number, Ty::Number.optional()], true),
        );
        no(
            &func(&[], &[Ty::Number], true),
            &func(&[], &[Ty::Number, Ty::String], true),
        );
    }

    #[test]
    fn function_varargs() {
        // A value vararg must accept the target's excess fixed parameters.
        ok(
            &Ty::Function(Box::new(FunctionTy {
                varargs: Some(Ty::Number),
                ..FunctionTy::default()
            })),
            &func(
                &[("a", Ty::Number, false), ("b", Ty::Integer, false)],
                &[],
                false,
            ),
        );
        no(
            &Ty::Function(Box::new(FunctionTy {
                varargs: Some(Ty::String),
                ..FunctionTy::default()
            })),
            &func(&[("a", Ty::Number, false)], &[], false),
        );
        // An `any` vararg accepts anything; a required value param past the
        // target arity is absorbed by the target's own vararg.
        ok(
            &Ty::Function(Box::new(FunctionTy {
                varargs: Some(Ty::Any),
                ..FunctionTy::default()
            })),
            &func(&[("a", Ty::String, false)], &[], false),
        );
        ok(
            &func(&[("a", Ty::Number, false)], &[], false),
            &Ty::Function(Box::new(FunctionTy {
                varargs: Some(Ty::Any),
                ..FunctionTy::default()
            })),
        );
    }

    #[test]
    fn opaque_function_is_universal() {
        let opaque = Ty::Function(Box::new(FunctionTy::opaque()));
        let concrete = func(&[("x", Ty::Number, false)], &[Ty::String], true);
        ok(&opaque, &concrete);
        ok(&concrete, &opaque);
        ok(&opaque, &Ty::Function(Box::default()));
    }

    #[test]
    fn function_overload_fallback() {
        // The primary returns string (fails); an overload returns number.
        let value = Ty::Function(Box::new(FunctionTy {
            returns: vec![Ty::String],
            has_return_annotation: true,
            overloads: vec![FunctionTy {
                returns: vec![Ty::Number],
                has_return_annotation: true,
                ..FunctionTy::default()
            }],
            ..FunctionTy::default()
        }));
        ok(&value, &func(&[], &[Ty::Number], true));
    }

    #[test]
    fn function_unknown_params_are_lenient_both_ways() {
        // An unannotated (unknown) value parameter accepts any target param.
        ok(
            &func(&[("x", Ty::Unknown, false)], &[], false),
            &func(&[("x", Ty::Number, false)], &[], false),
        );
        // An unknown *target* parameter is lenient even in strict mode.
        ok(
            &func(&[("x", Ty::Number, false)], &[], false),
            &func(&[("x", Ty::Unknown, false)], &[], false),
        );
    }

    // --- number-literal identity -----------------------------------------

    #[test]
    fn number_literals_compare_numerically_not_textually() {
        // `1` and `1.0` are the same value, so the literal types are mutually
        // assignable even though the source texts differ.
        ok(&Ty::NumberLit("1".into()), &Ty::NumberLit("1.0".into()));
        ok(&Ty::NumberLit("1.0".into()), &Ty::NumberLit("1".into()));
        ok(&Ty::NumberLit("2".into()), &Ty::NumberLit("2".into()));
        no(&Ty::NumberLit("1".into()), &Ty::NumberLit("2".into()));
        // A hex literal has no decimal parse; equality falls back to text.
        ok(&Ty::NumberLit("0x10".into()), &Ty::NumberLit("0x10".into()));
        no(&Ty::NumberLit("0x10".into()), &Ty::NumberLit("16".into()));
    }

    // --- indexers ---------------------------------------------------------

    fn indexed(key: Ty, value: Ty) -> Ty {
        Ty::Table(Box::new(TableTy {
            indexers: vec![(key, value)],
            ..TableTy::default()
        }))
    }

    #[test]
    fn indexer_values_must_satisfy_a_reachable_target_indexer() {
        // Same key domain, wrong value type.
        no(
            &indexed(Ty::String, Ty::Number),
            &indexed(Ty::String, Ty::String),
        );
        ok(
            &indexed(Ty::String, Ty::Integer),
            &indexed(Ty::String, Ty::Number),
        );
        // A value indexer whose keys cannot reach the target's key domain
        // imposes no obligation.
        ok(
            &indexed(Ty::String, Ty::Number),
            &indexed(Ty::Integer, Ty::String),
        );
    }

    #[test]
    fn optional_target_fields_admit_nil_from_the_value() {
        let value = table(&[("a", Ty::union(vec![Ty::Number, Ty::Nil]), false)]);
        let target = table(&[("a", Ty::Number, true)]);
        ok(&value, &target);
        // …but a non-optional target field does not.
        no(&value, &table(&[("a", Ty::Number, false)]));
    }

    // --- nominal ancestry -------------------------------------------------

    /// Build a `TypeEnv` from one file of LuaCATS source, the way the
    /// recursion-guard test below does.
    fn env_from(src: &str) -> TypeEnv {
        let mut env = TypeEnv::default();
        env.merge_file_types(&{
            let parsed = luabox_syntax::lua::parse(src, luabox_syntax::lua::Dialect::Lua54);
            let items = luabox_syntax::luacats::harvest(&parsed);
            let inner = TypeEnv::build_from_items(&parsed, &items, None);
            crate::env::FileTypes::collect(
                &items,
                &inner,
                &std::collections::HashMap::new(),
                "nom.lua",
            )
        });
        env
    }

    /// A subclass flows where its declared ancestor is demanded even when it
    /// overrides an inherited member at an incompatible type — the nominal
    /// half of luals 3.13.5's class assignability, measured against the
    /// pinned binary. Structural comparison rejects this, which made
    /// overriding a member break every upcast the declaration promises.
    #[test]
    fn a_subclass_upcasts_to_its_declared_ancestor_despite_an_incompatible_override() {
        let env = env_from(
            "\
---@class NomP
---@field v string
---@class NomChild : NomP
---@field v number
---@class NomGrand : NomChild
",
        );
        // Direct parent, and transitively through a grandparent.
        for child in ["NomChild", "NomGrand"] {
            assert!(
                assignable(
                    &env,
                    Exactness::Strict,
                    &Ty::Named(child.into()),
                    &Ty::Named("NomP".into())
                ),
                "{child} should upcast to its declared ancestor NomP"
            );
        }
        // The other direction is still refused: a parent is not a child.
        assert!(!assignable(
            &env,
            Exactness::Strict,
            &Ty::Named("NomP".into()),
            &Ty::Named("NomChild".into())
        ));
    }

    /// The nominal rule is ancestry, not name-similarity: an unrelated class
    /// gets no free pass, and (until #68) still resolves structurally.
    #[test]
    fn an_unrelated_class_does_not_upcast_by_the_nominal_rule() {
        let env = env_from(
            "\
---@class NomBase
---@field v string
---@class NomOther
---@field v number
",
        );
        assert!(!assignable(
            &env,
            Exactness::Strict,
            &Ty::Named("NomOther".into()),
            &Ty::Named("NomBase".into())
        ));
    }

    // --- recursion guard --------------------------------------------------

    #[test]
    fn mutually_recursive_named_classes_terminate() {
        // `A = { next: B }`, `B = { next: A }`: the coinduction guard must
        // stop the walk instead of recursing forever.
        let mut env = TypeEnv::default();
        env.merge_file_types(&{
            let parsed = luabox_syntax::lua::parse(
                "\
---@class RecA
---@field next RecB
---@class RecB
---@field next RecA
",
                luabox_syntax::lua::Dialect::Lua54,
            );
            let items = luabox_syntax::luacats::harvest(&parsed);
            let inner = TypeEnv::build_from_items(&parsed, &items, None);
            crate::env::FileTypes::collect(
                &items,
                &inner,
                &std::collections::HashMap::new(),
                "rec.lua",
            )
        });
        assert!(assignable(
            &env,
            Exactness::Strict,
            &Ty::Named("RecA".into()),
            &Ty::Named("RecB".into())
        ));
    }

    #[test]
    fn an_undeclared_named_value_is_unknown_shaped() {
        // Strict treats an unresolvable name like `unknown` (an error);
        // loose accepts it. An undeclared *target* always accepts.
        let undeclared = Ty::Named("NeverDeclared".into());
        assert!(!assignable(
            &env(),
            Exactness::Strict,
            &undeclared,
            &Ty::Number
        ));
        assert!(assignable(
            &env(),
            Exactness::Loose,
            &undeclared,
            &Ty::Number
        ));
        assert!(assignable(
            &env(),
            Exactness::Strict,
            &Ty::Number,
            &undeclared
        ));
    }

    // --- mismatch explanations -------------------------------------------

    #[test]
    fn explain_names_both_missing_and_mismatched_members() {
        let value = table(&[("a", Ty::String, false)]);
        let target = table(&[("a", Ty::Number, false), ("b", Ty::Number, false)]);
        let detail = explain_mismatch(&env(), Exactness::Strict, &value, &target)
            .expect("a table-level story");
        assert_eq!(
            detail,
            "missing `b`; mismatched `a` (expected `number`, found `string`)"
        );
    }

    #[test]
    fn explain_is_silent_when_the_shapes_agree() {
        let value = table(&[("a", Ty::Number, false)]);
        let target = table(&[("a", Ty::Number, false)]);
        assert_eq!(
            explain_mismatch(&env(), Exactness::Strict, &value, &target),
            None
        );
    }

    #[test]
    fn explain_needs_two_table_shapes() {
        // Non-table operands have no member-level story to tell.
        assert_eq!(
            explain_mismatch(&env(), Exactness::Strict, &Ty::Number, &Ty::String),
            None
        );
        // A union of two real members does not unwrap to a single shape.
        let multi = Ty::union(vec![
            table(&[("a", Ty::Number, false)]),
            table(&[("b", Ty::Number, false)]),
        ]);
        assert_eq!(
            explain_mismatch(
                &env(),
                Exactness::Strict,
                &table(&[("a", Ty::String, false)]),
                &multi
            ),
            None
        );
        // A `T?` optional *does* unwrap to its single non-nil member.
        let optional = Ty::union(vec![table(&[("a", Ty::Number, false)]), Ty::Nil]);
        assert!(
            explain_mismatch(
                &env(),
                Exactness::Strict,
                &table(&[("a", Ty::String, false)]),
                &optional
            )
            .is_some()
        );
    }
}
