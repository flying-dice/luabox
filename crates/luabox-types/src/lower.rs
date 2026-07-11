//! Lowering LuaCATS type expressions ([`TypeExpr`]) into the type IR
//! ([`Ty`]).
//!
//! Aliases are expanded here (with a cycle guard: a self-referential alias
//! collapses to `unknown`); classes and enums stay as [`Ty::Named`]
//! references resolved through the environment. Generic type variables in
//! scope (`---@generic T` and a generic class's own `<T>` params) lower to
//! `Ty::Named(T)` placeholders that the monomorphisation engine
//! ([`crate::generics`]) substitutes; a generic `---@class Name<T>` reference
//! (`Name<number>`) lowers directly to the substituted table (#84).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use luabox_syntax::luacats::{
    AliasTag, FunParam, FunReturn, Span, TableField, TypeExpr, TypeExprKind,
};

use crate::generics::subst_ty;
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// The type names a file declares, collected before lowering so forward
/// references resolve.
#[derive(Debug, Default)]
pub(crate) struct Declared {
    pub classes: BTreeSet<String>,
    pub enums: BTreeSet<String>,
    pub aliases: BTreeMap<String, AliasTag>,
}

/// A generic `---@class Name<T>`'s template: its parameter names and the
/// lowered shape of its own `---@field`s, with each `T` kept as a
/// `Ty::Named(T)` placeholder. A reference `Name<number>` instantiates this
/// by substituting the type arguments (`#84`, design (a): the reference
/// lowers directly to the substituted `Ty::Table`, mirroring `.luab`
/// templates — `Ty::Named` stays a bare string).
#[derive(Debug, Clone, Default)]
pub(crate) struct GenericClass {
    pub params: Vec<String>,
    pub template: TableTy,
}

/// Lowers [`TypeExpr`]s against a set of declared names, recording every
/// reference to an undeclared type (surfaced as LB0305).
///
/// `.luab` types resolve through the ambient package scope (SHAPES-V2.md):
/// any standard annotation position may name any fully-qualified shape type
/// — there are no imports and no shape-specific tags.
pub(crate) struct Lowerer<'a> {
    decl: &'a Declared,
    /// The ambient `.luab` package scope, when the project has one.
    pub shape_scope: Option<&'a crate::shape::ShapeScope>,
    /// Generic parameter names currently in scope (`---@generic T` and a
    /// generic class's own `<T>` params). These lower to `Ty::Named(name)`
    /// placeholders that substitution later replaces — never LB0305.
    pub generics: HashSet<String>,
    /// Generic `---@class Name<T>` templates, by name — built before the main
    /// lowering pass so references resolve regardless of declaration order.
    pub generic_classes: BTreeMap<String, GenericClass>,
    /// Alias-expansion stack (cycle guard).
    stack: Vec<String>,
    /// `(name, span)` of every reference to an undeclared type name.
    pub unknown_names: Vec<(String, Span)>,
    /// `(message, span)` of every bad `.luab` generic instantiation reached
    /// from a LuaCATS annotation site (wrong arity, or type arguments given
    /// to a non-generic shape type) — surfaced as `LB2007`.
    pub shape_ref_errors: Vec<(String, Span)>,
}

impl<'a> Lowerer<'a> {
    pub fn new(decl: &'a Declared) -> Self {
        Lowerer {
            decl,
            shape_scope: None,
            generics: HashSet::new(),
            generic_classes: BTreeMap::new(),
            stack: Vec::new(),
            unknown_names: Vec::new(),
            shape_ref_errors: Vec::new(),
        }
    }

    /// Lower one type expression.
    pub fn lower(&mut self, expr: &TypeExpr) -> Ty {
        match &expr.kind {
            TypeExprKind::Named { name, args } => self.lower_named(name, args, expr.span),
            TypeExprKind::Optional(inner) => self.lower(inner).optional(),
            TypeExprKind::Array(elem) => Ty::Table(Box::new(TableTy {
                array: Some(self.lower(elem)),
                ..TableTy::default()
            })),
            TypeExprKind::Union(members) => {
                Ty::union(members.iter().map(|m| self.lower(m)).collect())
            }
            // A tuple `[T1, T2, ...]` is a fixed-position table: member `i`
            // lives at integer key `i` (1-based), modeled as an integer-literal
            // indexer so `t[1]` reads back `T1` and a literal in tuple-typed
            // position is checked per position (#86). Reading past the end is
            // lenient (luals) — no `array` part, so a missing position is
            // `unknown`, never a hard error.
            TypeExprKind::Tuple(members) => {
                let indexers = members
                    .iter()
                    .enumerate()
                    .map(|(i, m)| (Ty::NumberLit((i + 1).to_string()), self.lower(m)))
                    .collect();
                Ty::Table(Box::new(TableTy {
                    indexers,
                    ..TableTy::default()
                }))
            }
            TypeExprKind::Table(fields) => self.lower_table(fields),
            TypeExprKind::Fun { params, returns } => self.lower_fun(params, returns),
            TypeExprKind::StringLit(raw) => Ty::StringLit(unquote(raw)),
            TypeExprKind::NumberLit(text) => Ty::NumberLit(text.clone()),
            TypeExprKind::BoolLit(value) => Ty::BoolLit(*value),
            TypeExprKind::Paren(inner) => self.lower(inner),
            // A backtick capture (`` `T` ``) in a generic function's `---@param`
            // captures the type *named by* the argument. In scope it lowers to
            // a distinguished `Ty::Named("`T`")` placeholder that call-site
            // inference recognises (#84); out of scope it is inert `unknown`.
            TypeExprKind::Backtick(name) => {
                if self.generics.contains(name) {
                    Ty::Named(format!("`{name}`"))
                } else {
                    Ty::Unknown
                }
            }
            // A malformed type already carries a LuaCATS parse error.
            TypeExprKind::Error => Ty::Unknown,
        }
    }

    fn lower_named(&mut self, name: &str, args: &[TypeExpr], span: Span) -> Ty {
        if self.generics.contains(name) {
            // A type variable in scope: keep it as a placeholder the
            // substitution engine resolves (never LB0305).
            return Ty::Named(name.to_string());
        }
        if let Some(ty) = self.lower_builtin(name, args) {
            return ty;
        }
        if self.decl.aliases.contains_key(name) {
            return self.expand_alias(name);
        }
        if self.decl.classes.contains(name) || self.decl.enums.contains(name) {
            // A generic `---@class Name<T>`: monomorphise the template at the
            // reference site (#84). A bare reference (no args) is lenient —
            // missing arguments become `unknown`, matching luals.
            if let Some(gc) = self.generic_classes.get(name) {
                let gc = gc.clone();
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.lower(a)).collect();
                let mut map = BTreeMap::new();
                for (i, param) in gc.params.iter().enumerate() {
                    map.insert(
                        param.clone(),
                        arg_tys.get(i).cloned().unwrap_or(Ty::Unknown),
                    );
                }
                return subst_ty(&Ty::Table(Box::new(gc.template.clone())), &map);
            }
            return Ty::Named(name.to_string());
        }
        if let Some(scope) = self.shape_scope
            && let Some(shape) = scope.get(name)
        {
            if shape.params.is_empty() {
                if !args.is_empty() {
                    self.shape_ref_errors.push((
                        format!("`{name}` is not generic but was given type arguments"),
                        span,
                    ));
                }
                return match &shape.ty {
                    // Concrete object types stay nominal (resolved
                    // structurally via the environment); alias-like RHS
                    // expands inline.
                    Ty::Table(_) => Ty::Named(name.to_string()),
                    other => other.clone(),
                };
            }
            // Monomorphise a template use site. `instantiate` itself reports
            // a wrong non-zero arity via the `diags` sink — recovered here as
            // a `(message, span)` pair anchored to this annotation site
            // rather than the throwaway file/range `instantiate` was given.
            let args: Vec<Ty> = args.iter().map(|a| self.lower(a)).collect();
            let mut diags = Vec::new();
            let result = scope.instantiate(name, &args, "", 0..0, &mut diags);
            if let Some(diag) = diags.into_iter().next() {
                self.shape_ref_errors.push((diag.message, span));
            }
            return result.unwrap_or(Ty::Unknown);
        }
        self.unknown_names.push((name.to_string(), span));
        Ty::Unknown
    }

    /// Built-in type names. `table` is *structural* even when bare
    /// (`{ [any]: any }`) — never an opaque primitive (SPEC.md §3).
    fn lower_builtin(&mut self, name: &str, args: &[TypeExpr]) -> Option<Ty> {
        Some(match name {
            "nil" => Ty::Nil,
            "boolean" | "bool" => Ty::Boolean,
            "number" => Ty::Number,
            "integer" | "int" => Ty::Integer,
            "string" => Ty::String,
            "any" => Ty::Any,
            // `unknown` is explicit; thread/userdata are opaque runtime
            // handles — TODO(P1): dedicated primitives for them.
            "unknown" | "thread" | "userdata" | "lightuserdata" | "self" => Ty::Unknown,
            "table" => {
                if let [key, value] = args {
                    let key = self.lower(key);
                    let value = self.lower(value);
                    Ty::Table(Box::new(TableTy {
                        indexers: vec![(key, value)],
                        ..TableTy::default()
                    }))
                } else {
                    Ty::any_table()
                }
            }
            "function" | "fun" => Ty::Function(Box::new(FunctionTy::opaque())),
            _ => return None,
        })
    }

    /// Expand an alias body, guarding against cycles (`A = B`, `B = A`
    /// collapses to `unknown` rather than recursing forever).
    fn expand_alias(&mut self, name: &str) -> Ty {
        if self.stack.iter().any(|n| n == name) {
            return Ty::Unknown;
        }
        let Some(alias) = self.decl.aliases.get(name) else {
            return Ty::Unknown;
        };
        let alias = alias.clone();
        self.stack.push(name.to_string());
        let mut members: Vec<Ty> = Vec::new();
        if let Some(ty) = &alias.ty {
            members.push(self.lower(ty));
        }
        for member in &alias.members {
            members.push(self.lower(&member.ty));
        }
        self.stack.pop();
        Ty::union(members)
    }

    fn lower_table(&mut self, fields: &[TableField]) -> Ty {
        let mut table = TableTy::default();
        for field in fields {
            match field {
                TableField::Named { name, optional, ty } => {
                    let ty = self.lower(ty);
                    table.fields.insert(
                        name.clone(),
                        FieldTy {
                            ty,
                            optional: *optional,
                        },
                    );
                }
                TableField::Indexer { key, value } => {
                    let key = self.lower(key);
                    let value = self.lower(value);
                    table.indexers.push((key, value));
                }
            }
        }
        Ty::Table(Box::new(table))
    }

    fn lower_fun(&mut self, params: &[FunParam], returns: &[FunReturn]) -> Ty {
        let mut func = FunctionTy {
            has_return_annotation: true,
            ..FunctionTy::default()
        };
        for param in params {
            let ty = param.ty.as_ref().map_or(Ty::Unknown, |t| self.lower(t));
            if param.vararg {
                func.varargs = Some(ty);
            } else {
                func.params.push(ParamTy {
                    name: param.name.clone(),
                    ty,
                    optional: param.optional,
                });
            }
        }
        for ret in returns {
            if ret.vararg {
                func.returns_vararg = true;
            }
            func.returns.push(self.lower(&ret.ty));
        }
        Ty::Function(Box::new(func))
    }
}

/// Strip the quotes from a string-literal type's raw text. Escape sequences
/// are kept verbatim (MVP; literal-type comparison is textual).
fn unquote(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use luabox_syntax::luacats::TypeParser;

    use super::*;

    fn lower_str(text: &str) -> (Ty, Vec<(String, Span)>) {
        let decl = Declared::default();
        let mut lowerer = Lowerer::new(&decl);
        let mut parser = TypeParser::new(text, 0);
        let expr = parser.parse_type();
        let ty = lowerer.lower(&expr);
        (ty, lowerer.unknown_names)
    }

    fn lower_ok(text: &str) -> Ty {
        let (ty, unknown) = lower_str(text);
        assert!(unknown.is_empty(), "unexpected unknown names: {unknown:?}");
        ty
    }

    #[test]
    fn primitives_lower() {
        assert_eq!(lower_ok("nil"), Ty::Nil);
        assert_eq!(lower_ok("boolean"), Ty::Boolean);
        assert_eq!(lower_ok("number"), Ty::Number);
        assert_eq!(lower_ok("integer"), Ty::Integer);
        assert_eq!(lower_ok("string"), Ty::String);
        assert_eq!(lower_ok("any"), Ty::Any);
        assert_eq!(lower_ok("unknown"), Ty::Unknown);
    }

    #[test]
    fn bare_table_is_structural_not_opaque() {
        // The hard requirement: `table` never lowers to an opaque primitive.
        let Ty::Table(table) = lower_ok("table") else {
            panic!("expected a structural table");
        };
        assert_eq!(table.indexers, vec![(Ty::Any, Ty::Any)]);
        assert!(table.fields.is_empty());
    }

    #[test]
    fn generic_table_becomes_indexer() {
        let Ty::Table(table) = lower_ok("table<string, number>") else {
            panic!("expected a structural table");
        };
        assert_eq!(table.indexers, vec![(Ty::String, Ty::Number)]);
    }

    #[test]
    fn array_and_optional_and_union() {
        let Ty::Table(table) = lower_ok("integer[]") else {
            panic!("expected array table");
        };
        assert_eq!(table.array, Some(Ty::Integer));
        assert_eq!(lower_ok("number?"), Ty::Union(vec![Ty::Number, Ty::Nil]));
        assert_eq!(
            lower_ok("string|number"),
            Ty::Union(vec![Ty::String, Ty::Number])
        );
    }

    #[test]
    fn table_literal_type_lowers_fields_and_indexers() {
        let Ty::Table(table) = lower_ok("{ x: number, y?: string, [string]: boolean }") else {
            panic!("expected table");
        };
        assert_eq!(
            table.fields["x"],
            FieldTy {
                ty: Ty::Number,
                optional: false
            }
        );
        assert_eq!(
            table.fields["y"],
            FieldTy {
                ty: Ty::String,
                optional: true
            }
        );
        assert_eq!(table.indexers, vec![(Ty::String, Ty::Boolean)]);
    }

    #[test]
    fn literal_types_lower() {
        assert_eq!(lower_ok("\"on\""), Ty::StringLit("on".into()));
        assert_eq!(lower_ok("42"), Ty::NumberLit("42".into()));
        assert_eq!(lower_ok("true"), Ty::BoolLit(true));
    }

    #[test]
    fn fun_type_lowers() {
        let Ty::Function(func) = lower_ok("fun(a: number, b?: string, ...: any): boolean") else {
            panic!("expected function");
        };
        assert_eq!(func.params.len(), 2);
        assert_eq!(func.params[0].ty, Ty::Number);
        assert!(func.params[1].optional);
        assert_eq!(func.varargs, Some(Ty::Any));
        assert_eq!(func.returns, vec![Ty::Boolean]);
    }

    #[test]
    fn unknown_name_is_recorded() {
        let (ty, unknown) = lower_str("Wibble");
        assert_eq!(ty, Ty::Unknown);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].0, "Wibble");
    }

    #[test]
    fn generic_param_lowers_to_placeholder_without_lb0305() {
        let decl = Declared::default();
        let mut lowerer = Lowerer::new(&decl);
        lowerer.generics.insert("T".to_string());
        let mut parser = TypeParser::new("T", 0);
        let expr = parser.parse_type();
        // A type variable in scope stays a `Ty::Named` placeholder (which the
        // substitution engine resolves) — and never trips LB0305.
        assert_eq!(lowerer.lower(&expr), Ty::Named("T".to_string()));
        assert!(lowerer.unknown_names.is_empty());
    }

    #[test]
    fn alias_expands_with_cycle_guard() {
        let mut decl = Declared::default();
        let mut parser = TypeParser::new("\"on\"|\"off\"", 0);
        let body = parser.parse_type();
        decl.aliases.insert(
            "Switch".to_string(),
            AliasTag {
                name: "Switch".to_string(),
                ty: Some(body),
                members: Vec::new(),
                span: Span::new(0, 0),
            },
        );
        // Self-referential alias: `Loop = Loop|number` must not recurse.
        let mut parser = TypeParser::new("Loop|number", 0);
        let loop_body = parser.parse_type();
        decl.aliases.insert(
            "Loop".to_string(),
            AliasTag {
                name: "Loop".to_string(),
                ty: Some(loop_body),
                members: Vec::new(),
                span: Span::new(0, 0),
            },
        );

        let mut lowerer = Lowerer::new(&decl);
        let mut parser = TypeParser::new("Switch", 0);
        let expr = parser.parse_type();
        assert_eq!(
            lowerer.lower(&expr),
            Ty::Union(vec![
                Ty::StringLit("on".into()),
                Ty::StringLit("off".into())
            ])
        );

        let mut parser = TypeParser::new("Loop", 0);
        let expr = parser.parse_type();
        assert_eq!(
            lowerer.lower(&expr),
            Ty::Union(vec![Ty::Unknown, Ty::Number])
        );
        assert!(lowerer.unknown_names.is_empty());
    }
}
