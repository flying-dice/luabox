//! Monomorphisation engine for LuaCATS generics ([#84]).
//!
//! Two operations, shared by every generic front-end (generic `---@class<T>`
//! references and `---@generic` functions):
//!
//! - [`subst_ty`] substitutes `Ty::Named(param)` placeholders through tables,
//!   functions, and unions — the machinery a generic *reference* uses to
//!   monomorphise a template body with concrete type arguments.
//! - [`infer_call`] performs luals-style call-site inference: it walks a
//!   generic function's declared parameter types structurally against the
//!   actual argument types and binds each type variable to the first argument
//!   that fixes it (luals is first-match, not Hindley–Milner). Unbound
//!   variables fall back to `unknown` at substitution time.
//!
//! A backtick type parameter (`` ---@param cls `T` ``) captures the type
//! *named by* its argument: a string-literal argument `"Circle"` binds `T` to
//! the class `Circle`. The placeholder is lowered as `Ty::Named("`T`")` (a
//! spelling no real type name can collide with) so the two capture modes stay
//! distinguishable here.

use std::collections::{BTreeMap, HashSet};

use crate::ty::{FunctionTy, TableTy, Ty};

/// Substitute `Ty::Named(param)` placeholders throughout a type, per `map`.
/// A name not in `map` is left as-is (an unbound type variable stays
/// `Ty::Named`, which resolves leniently — like `unknown` — downstream).
pub(crate) fn subst_ty(ty: &Ty, map: &BTreeMap<String, Ty>) -> Ty {
    match ty {
        Ty::Named(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Union(members) => Ty::union(members.iter().map(|m| subst_ty(m, map)).collect()),
        Ty::Table(table) => Ty::Table(Box::new(subst_table(table, map))),
        Ty::Function(func) => Ty::Function(Box::new(subst_function(func, map))),
        _ => ty.clone(),
    }
}

fn subst_table(table: &TableTy, map: &BTreeMap<String, Ty>) -> TableTy {
    let mut out = table.clone();
    for field in out.fields.values_mut() {
        field.ty = subst_ty(&field.ty, map);
    }
    out.indexers = out
        .indexers
        .iter()
        .map(|(k, v)| (subst_ty(k, map), subst_ty(v, map)))
        .collect();
    out.array = out.array.as_ref().map(|a| subst_ty(a, map));
    out
}

/// Substitute placeholders through a whole function signature — the operation
/// that monomorphises a generic function at a call site once its type
/// variables are bound. The `generics` list itself is dropped from the result
/// (the returned signature is concrete).
pub(crate) fn subst_function(func: &FunctionTy, map: &BTreeMap<String, Ty>) -> FunctionTy {
    let mut out = func.clone();
    for param in &mut out.params {
        param.ty = subst_ty(&param.ty, map);
    }
    out.varargs = out.varargs.as_ref().map(|v| subst_ty(v, map));
    out.returns = out.returns.iter().map(|r| subst_ty(r, map)).collect();
    out.generics = Vec::new();
    out
}

/// Call-site type inference for a generic function (luals semantics): bind
/// each `---@generic` variable from the actual argument types, first match
/// wins. Returns the substitution map; callers apply it with [`subst_ty`] /
/// [`subst_function`] and check any constraints against the bindings.
///
/// Concrete (non-`Ty::Unknown`) arguments are widened before binding, so
/// `id(5)` fixes `T = integer` rather than the singleton literal type `5` —
/// matching what luals surfaces.
pub(crate) fn infer_call(sig: &FunctionTy, args: &[Ty]) -> BTreeMap<String, Ty> {
    let vars: HashSet<&str> = sig.generics.iter().map(|g| g.name.as_str()).collect();
    let mut map = BTreeMap::new();
    for (i, param) in sig.params.iter().enumerate() {
        if let Some(arg) = args.get(i) {
            unify(&param.ty, arg, &vars, &mut map);
        }
    }
    if let Some(velem) = &sig.varargs {
        for arg in args.iter().skip(sig.params.len()) {
            unify(velem, arg, &vars, &mut map);
        }
    }
    map
}

/// Structurally match a declared parameter type against an actual argument
/// type, binding any type variable it reaches (first binding wins).
fn unify(param: &Ty, arg: &Ty, vars: &HashSet<&str>, map: &mut BTreeMap<String, Ty>) {
    if let Ty::Named(name) = param {
        // A backtick capture (`` `T` ``) binds the type *named by* the
        // argument: a string literal `"Circle"` fixes `T = Circle`; any other
        // value fixes `T` to the value's own type.
        if let Some(var) = name.strip_prefix('`').and_then(|n| n.strip_suffix('`'))
            && vars.contains(var)
        {
            let captured = match arg {
                Ty::StringLit(class) => Ty::Named(class.clone()),
                other => other.clone(),
            };
            map.entry(var.to_string()).or_insert(captured);
            return;
        }
        if vars.contains(name.as_str()) {
            if !matches!(arg, Ty::Unknown) {
                map.entry(name.clone()).or_insert_with(|| arg.widened());
            }
            return;
        }
    }
    match (param, arg) {
        (Ty::Table(pt), Ty::Table(at)) => {
            if let (Some(pe), Some(ae)) = (&pt.array, &at.array) {
                unify(pe, ae, vars, map);
            }
            for (name, pf) in &pt.fields {
                if let Some(af) = at.fields.get(name) {
                    unify(&pf.ty, &af.ty, vars, map);
                }
            }
            for (pk, pv) in &pt.indexers {
                for (ak, av) in &at.indexers {
                    unify(pk, ak, vars, map);
                    unify(pv, av, vars, map);
                }
            }
        }
        (Ty::Function(pf), Ty::Function(af)) => {
            for (pp, ap) in pf.params.iter().zip(&af.params) {
                unify(&pp.ty, &ap.ty, vars, map);
            }
            for (pr, ar) in pf.returns.iter().zip(&af.returns) {
                unify(pr, ar, vars, map);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::{FieldTy, ParamTy, TableTy, TypeParam};

    fn generic_fn(generics: &[&str], params: &[(&str, Ty)], returns: &[Ty]) -> FunctionTy {
        FunctionTy {
            generics: generics
                .iter()
                .map(|n| TypeParam {
                    name: (*n).to_string(),
                    constraint: None,
                })
                .collect(),
            params: params
                .iter()
                .map(|(name, ty)| ParamTy {
                    name: (*name).to_string(),
                    ty: ty.clone(),
                    optional: false,
                })
                .collect(),
            returns: returns.to_vec(),
            has_return_annotation: true,
            ..FunctionTy::default()
        }
    }

    fn array(elem: Ty) -> Ty {
        Ty::Table(Box::new(TableTy {
            array: Some(elem),
            ..TableTy::default()
        }))
    }

    #[test]
    fn subst_walks_tables_and_functions() {
        let map = BTreeMap::from([("T".to_string(), Ty::Number)]);
        assert_eq!(subst_ty(&Ty::Named("T".into()), &map), Ty::Number);
        assert_eq!(
            subst_ty(&array(Ty::Named("T".into())), &map),
            array(Ty::Number)
        );
        let unbound = subst_ty(&Ty::Named("U".into()), &map);
        assert_eq!(unbound, Ty::Named("U".into()));
    }

    #[test]
    fn infers_from_scalar_argument_widened() {
        let sig = generic_fn(
            &["T"],
            &[("x", Ty::Named("T".into()))],
            &[Ty::Named("T".into())],
        );
        let map = infer_call(&sig, &[Ty::NumberLit("5".into())]);
        assert_eq!(map.get("T"), Some(&Ty::Integer));
    }

    #[test]
    fn first_binding_wins() {
        let sig = generic_fn(
            &["T"],
            &[("a", Ty::Named("T".into())), ("b", Ty::Named("T".into()))],
            &[Ty::Named("T".into())],
        );
        let map = infer_call(
            &sig,
            &[Ty::NumberLit("5".into()), Ty::StringLit("x".into())],
        );
        assert_eq!(map.get("T"), Some(&Ty::Integer));
    }

    #[test]
    fn infers_through_array_element() {
        let sig = generic_fn(
            &["T"],
            &[("xs", array(Ty::Named("T".into())))],
            &[Ty::Named("T".into())],
        );
        let map = infer_call(&sig, &[array(Ty::String)]);
        assert_eq!(map.get("T"), Some(&Ty::String));
    }

    #[test]
    fn backtick_captures_string_literal_as_class() {
        let sig = FunctionTy {
            generics: vec![TypeParam {
                name: "T".into(),
                constraint: None,
            }],
            params: vec![ParamTy {
                name: "name".into(),
                ty: Ty::Named("`T`".into()),
                optional: false,
            }],
            returns: vec![Ty::Named("T".into())],
            has_return_annotation: true,
            ..FunctionTy::default()
        };
        let map = infer_call(&sig, &[Ty::StringLit("Circle".into())]);
        assert_eq!(map.get("T"), Some(&Ty::Named("Circle".into())));
    }

    #[test]
    fn unbound_variable_substitutes_to_itself_then_leniently() {
        // No argument fixes `T`: the map is empty and the placeholder stays.
        let sig = generic_fn(&["T"], &[], &[Ty::Named("T".into())]);
        let map = infer_call(&sig, &[]);
        assert!(map.is_empty());
        let out = subst_function(&sig, &map);
        assert_eq!(out.returns, vec![Ty::Named("T".into())]);
    }

    #[test]
    fn field_shaped_unification() {
        let param = Ty::Table(Box::new(TableTy {
            fields: BTreeMap::from([(
                "value".to_string(),
                FieldTy {
                    ty: Ty::Named("T".into()),
                    optional: false,
                },
            )]),
            ..TableTy::default()
        }));
        let arg = Ty::Table(Box::new(TableTy {
            fields: BTreeMap::from([(
                "value".to_string(),
                FieldTy {
                    ty: Ty::Boolean,
                    optional: false,
                },
            )]),
            ..TableTy::default()
        }));
        let sig = generic_fn(&["T"], &[("p", param)], &[Ty::Named("T".into())]);
        let map = infer_call(&sig, &[arg]);
        assert_eq!(map.get("T"), Some(&Ty::Boolean));
    }
}
