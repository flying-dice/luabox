//! Lowering LuaCATS type expressions ([`TypeExpr`]) into the type IR
//! ([`Ty`]).
//!
//! Aliases are expanded here (with a cycle guard: a self-referential alias
//! collapses to `unknown`); classes and enums stay as [`Ty::Named`]
//! references resolved through the environment. Generic parameters in scope
//! (`---@generic T`) lower to `unknown` — TODO(P1): real type variables
//! with constraint solving.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use luabox_syntax::luacats::{
    AliasTag, FunParam, FunReturn, Span, TableField, TypeExpr, TypeExprKind,
};

use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// The type names a file declares, collected before lowering so forward
/// references resolve. Shape names imported via `---@use` participate so
/// LuaCATS annotations can reference `.luab` structs/traits (interop,
/// SHAPES.md §3) without tripping LB0305.
#[derive(Debug, Default)]
pub(crate) struct Declared {
    pub classes: BTreeSet<String>,
    pub enums: BTreeSet<String>,
    pub aliases: BTreeMap<String, AliasTag>,
    /// `.luab` struct/trait names in scope — lower to [`Ty::Named`].
    pub shape_names: BTreeSet<String>,
    /// Concrete `.luab` aliases in scope — pre-lowered, substituted inline.
    pub shape_aliases: BTreeMap<String, Ty>,
}

/// Lowers [`TypeExpr`]s against a set of declared names, recording every
/// reference to an undeclared type (surfaced as LB0305).
pub(crate) struct Lowerer<'a> {
    decl: &'a Declared,
    /// Generic parameter names currently in scope (lower to `unknown`).
    pub generics: HashSet<String>,
    /// Alias-expansion stack (cycle guard).
    stack: Vec<String>,
    /// `(name, span)` of every reference to an undeclared type name.
    pub unknown_names: Vec<(String, Span)>,
}

impl<'a> Lowerer<'a> {
    pub fn new(decl: &'a Declared) -> Self {
        Lowerer {
            decl,
            generics: HashSet::new(),
            stack: Vec::new(),
            unknown_names: Vec::new(),
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
            // TODO(P1): real tuple types; for now a tuple is an array of the
            // member union.
            TypeExprKind::Tuple(members) => Ty::Table(Box::new(TableTy {
                array: Some(Ty::union(members.iter().map(|m| self.lower(m)).collect())),
                ..TableTy::default()
            })),
            TypeExprKind::Table(fields) => self.lower_table(fields),
            TypeExprKind::Fun { params, returns } => self.lower_fun(params, returns),
            TypeExprKind::StringLit(raw) => Ty::StringLit(unquote(raw)),
            TypeExprKind::NumberLit(text) => Ty::NumberLit(text.clone()),
            TypeExprKind::BoolLit(value) => Ty::BoolLit(*value),
            TypeExprKind::Paren(inner) => self.lower(inner),
            // A backtick capture is a generic marker — TODO(P1); a
            // malformed type already carries a LuaCATS parse error.
            TypeExprKind::Backtick(_) | TypeExprKind::Error => Ty::Unknown,
        }
    }

    fn lower_named(&mut self, name: &str, args: &[TypeExpr], span: Span) -> Ty {
        if self.generics.contains(name) {
            return Ty::Unknown; // TODO(P1): generic type variables
        }
        if let Some(ty) = self.lower_builtin(name, args) {
            return ty;
        }
        if self.decl.aliases.contains_key(name) {
            return self.expand_alias(name);
        }
        if self.decl.classes.contains(name) || self.decl.enums.contains(name) {
            // TODO(P1): generic classes — `args` are ignored for now.
            return Ty::Named(name.to_string());
        }
        if let Some(ty) = self.decl.shape_aliases.get(name) {
            return ty.clone();
        }
        if self.decl.shape_names.contains(name) {
            return Ty::Named(name.to_string());
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
    fn generic_param_lowers_to_unknown_without_lb0305() {
        let decl = Declared::default();
        let mut lowerer = Lowerer::new(&decl);
        lowerer.generics.insert("T".to_string());
        let mut parser = TypeParser::new("T", 0);
        let expr = parser.parse_type();
        assert_eq!(lowerer.lower(&expr), Ty::Unknown);
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
