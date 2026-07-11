//! The type IR (SPEC.md §3).
//!
//! **Structural tables are load-bearing.** Rich table inference is a hard
//! spec requirement: tables never degrade to an opaque `table` primitive.
//! [`Ty::Table`] therefore carries a field map, indexer list, and array part
//! from day one — even the bare `table` annotation lowers to a structural
//! shape (`{ [any]: any }`) — so P1 inference extends this representation
//! instead of replacing it. A named class ([`Ty::Named`]) is a *reference*
//! resolved to its structural shape through the [`crate::env::TypeEnv`].

use std::collections::BTreeMap;
use std::fmt;

/// A type in the unified IR.
///
/// Optionals have no dedicated variant: `T?` lowers to `T | nil`
/// ([`Ty::optional`]). `unknown` is the type of every unannotated
/// expression (untyped = `unknown`, not `any`, per SPEC.md §3): in warn
/// mode it is assignable both ways, in strict mode `unknown -> T` is an
/// error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// No information — the type of unannotated code.
    Unknown,
    /// Explicitly dynamic; assignable both ways at every strictness.
    Any,
    /// The `nil` type.
    Nil,
    /// The `boolean` primitive.
    Boolean,
    /// The `number` primitive (supertype of `integer`).
    Number,
    /// The `integer` primitive.
    Integer,
    /// The `string` primitive.
    String,
    /// A boolean literal type (`true` / `false`).
    BoolLit(bool),
    /// A number literal type; the source text is kept verbatim.
    NumberLit(String),
    /// A string literal type; the *unquoted* content.
    StringLit(String),
    /// A union `A | B | ...` (flattened, at least two members).
    Union(Vec<Ty>),
    /// A reference to a declared `---@class` or `---@enum`, resolved to its
    /// structural shape via the environment. Aliases are expanded at
    /// lowering time and never appear here.
    Named(String),
    /// A function type.
    Function(Box<FunctionTy>),
    /// A structural table type — never an opaque primitive.
    Table(Box<TableTy>),
}

/// One named field of a [`TableTy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTy {
    /// The field's value type.
    pub ty: Ty,
    /// Whether the field may be absent (`field?` / `---@field name?`).
    pub optional: bool,
}

/// The structural shape of a table: named fields, typed indexers, and an
/// array part. `table<K, V>` becomes one indexer; `T[]` becomes the array
/// part; the bare `table` annotation becomes `{ [any]: any }`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableTy {
    /// Named fields, ordered for deterministic diagnostics.
    pub fields: BTreeMap<String, FieldTy>,
    /// `[KeyTy] -> ValueTy` indexers.
    pub indexers: Vec<(Ty, Ty)>,
    /// The array-part element type (`T[]`).
    pub array: Option<Ty>,
    /// Whether this table is a *sealed* `.luab` object shape (SHAPES-V2.md):
    /// literals checked against it get freshness diagnostics for undeclared
    /// keys (`LB0303`). LuaCATS tables are never sealed.
    pub sealed: bool,
}

/// One `---@generic T[: Constraint]` type parameter of a [`FunctionTy`].
///
/// The name is a placeholder captured as [`Ty::Named`] in the signature's
/// parameter and return types; call-site inference binds it and substitutes
/// (see [`crate::generics`]). `constraint`, when present, is checked against
/// the inferred binding (luals's bounded generics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParam {
    /// The type-variable name (`T`).
    pub name: String,
    /// An optional `: Constraint` bound the inferred binding must satisfy.
    pub constraint: Option<Ty>,
}

/// One parameter of a [`FunctionTy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamTy {
    /// The declared parameter name (diagnostics only).
    pub name: String,
    /// The parameter's type.
    pub ty: Ty,
    /// Whether the argument may be omitted or nil (`---@param name?`).
    pub optional: bool,
}

/// A function signature from `---@param` / `---@return` annotations (or a
/// `fun(...)` type expression).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunctionTy {
    /// Positional parameters, in order.
    pub params: Vec<ParamTy>,
    /// The vararg element type when the function accepts `...`.
    pub varargs: Option<Ty>,
    /// Declared return types, in order.
    pub returns: Vec<Ty>,
    /// Whether the last return is a vararg (`---@return T ...`).
    pub returns_vararg: bool,
    /// Whether any `---@return` was written. Without one, return
    /// statements are not checked and calls evaluate to `unknown`.
    pub has_return_annotation: bool,
    /// Additional `---@overload fun(...)` signatures. A call is accepted
    /// when it matches this primary signature *or* any overload; the
    /// primary governs the inferred result type (TODO(P1): pick the
    /// matching overload's return).
    pub overloads: Vec<FunctionTy>,
    /// `---@generic T[: C]` type parameters. When non-empty the `params` and
    /// `returns` carry `Ty::Named(param)` placeholders that call-site
    /// inference binds and substitutes ([`crate::generics::infer_call`]).
    pub generics: Vec<TypeParam>,
}

impl FunctionTy {
    /// The signature of a function nothing is known about (the bare
    /// `function` annotation): accepts anything, returns nothing checked.
    #[must_use]
    pub fn opaque() -> Self {
        FunctionTy {
            varargs: Some(Ty::Any),
            ..FunctionTy::default()
        }
    }

    /// How many arguments a call must supply: the non-optional parameters.
    #[must_use]
    pub fn required_params(&self) -> usize {
        self.params.iter().filter(|p| !p.optional).count()
    }
}

impl Ty {
    /// `T?` — a union with `nil` (flattened; `nil?` stays `nil`).
    #[must_use]
    pub fn optional(self) -> Ty {
        Ty::union(vec![self, Ty::Nil])
    }

    /// Build a union, flattening nested unions and dropping duplicates.
    /// A single surviving member collapses to itself.
    #[must_use]
    pub fn union(members: Vec<Ty>) -> Ty {
        let mut flat: Vec<Ty> = Vec::new();
        let push = |ty: Ty, flat: &mut Vec<Ty>| {
            if !flat.contains(&ty) {
                flat.push(ty);
            }
        };
        for member in members {
            match member {
                Ty::Union(inner) => {
                    for ty in inner {
                        push(ty, &mut flat);
                    }
                }
                other => push(other, &mut flat),
            }
        }
        match (flat.len(), flat.pop()) {
            (_, None) => Ty::Unknown,
            (1, Some(only)) => only,
            (_, Some(last)) => {
                flat.push(last);
                Ty::Union(flat)
            }
        }
    }

    /// A structural shape accepting any table: `{ [any]: any }`. This is
    /// what the bare `table` annotation lowers to — never a primitive.
    #[must_use]
    pub fn any_table() -> Ty {
        Ty::Table(Box::new(TableTy {
            indexers: vec![(Ty::Any, Ty::Any)],
            ..TableTy::default()
        }))
    }

    /// Whether a `nil` value satisfies this type without consulting the
    /// environment (named types are resolved by the assignability check).
    #[must_use]
    pub fn admits_nil(&self) -> bool {
        match self {
            Ty::Nil | Ty::Any | Ty::Unknown => true,
            Ty::Union(members) => members.iter().any(Ty::admits_nil),
            _ => false,
        }
    }

    /// Widen literal types to their primitives (`42` → `integer`, `1.5` →
    /// `number`, `"hi"` → `string`, `true` → `boolean`), recursively through
    /// unions, tables, and function signatures. Display-oriented: a binding
    /// initialised from a literal reads as its general type (`1|2` collapses
    /// to `integer`); checking always uses the precise types.
    #[must_use]
    pub fn widened(&self) -> Ty {
        match self {
            Ty::BoolLit(_) => Ty::Boolean,
            Ty::NumberLit(text) => {
                if crate::assign::is_integral_literal(text) {
                    Ty::Integer
                } else {
                    Ty::Number
                }
            }
            Ty::StringLit(_) => Ty::String,
            Ty::Union(members) => Ty::union(members.iter().map(Ty::widened).collect()),
            Ty::Table(table) => Ty::Table(Box::new(TableTy {
                fields: table
                    .fields
                    .iter()
                    .map(|(name, field)| {
                        (
                            name.clone(),
                            FieldTy {
                                ty: field.ty.widened(),
                                optional: field.optional,
                            },
                        )
                    })
                    .collect(),
                indexers: table
                    .indexers
                    .iter()
                    .map(|(k, v)| (k.widened(), v.widened()))
                    .collect(),
                array: table.array.as_ref().map(Ty::widened),
                sealed: table.sealed,
            })),
            Ty::Function(func) => Ty::Function(Box::new(FunctionTy {
                params: func
                    .params
                    .iter()
                    .map(|p| ParamTy {
                        name: p.name.clone(),
                        ty: p.ty.widened(),
                        optional: p.optional,
                    })
                    .collect(),
                varargs: func.varargs.as_ref().map(Ty::widened),
                returns: func.returns.iter().map(Ty::widened).collect(),
                returns_vararg: func.returns_vararg,
                has_return_annotation: func.has_return_annotation,
                overloads: func.overloads.clone(),
                generics: func.generics.clone(),
            })),
            other => other.clone(),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unknown => f.write_str("unknown"),
            Ty::Any => f.write_str("any"),
            Ty::Nil => f.write_str("nil"),
            Ty::Boolean => f.write_str("boolean"),
            Ty::Number => f.write_str("number"),
            Ty::Integer => f.write_str("integer"),
            Ty::String => f.write_str("string"),
            Ty::BoolLit(b) => write!(f, "{b}"),
            Ty::NumberLit(n) => f.write_str(n),
            Ty::StringLit(s) => write!(f, "\"{s}\""),
            Ty::Union(members) => {
                for (i, member) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_str("|")?;
                    }
                    write!(f, "{member}")?;
                }
                Ok(())
            }
            Ty::Named(name) => f.write_str(name),
            Ty::Function(func) => write_function(f, func),
            Ty::Table(table) => write_table(f, table),
        }
    }
}

fn write_function(f: &mut fmt::Formatter<'_>, func: &FunctionTy) -> fmt::Result {
    f.write_str("fun(")?;
    for (i, param) in func.params.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        if param.name.is_empty() {
            write!(f, "{}", param.ty)?;
        } else {
            let opt = if param.optional { "?" } else { "" };
            write!(f, "{}{opt}: {}", param.name, param.ty)?;
        }
    }
    if let Some(varargs) = &func.varargs {
        if !func.params.is_empty() {
            f.write_str(", ")?;
        }
        write!(f, "...: {varargs}")?;
    }
    f.write_str(")")?;
    if !func.returns.is_empty() {
        f.write_str(": ")?;
        for (i, ret) in func.returns.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{ret}")?;
        }
    }
    Ok(())
}

fn write_table(f: &mut fmt::Formatter<'_>, table: &TableTy) -> fmt::Result {
    // The catch-all shape renders as its annotation spelling.
    if table.fields.is_empty() && table.array.is_none() && table.indexers == [(Ty::Any, Ty::Any)] {
        return f.write_str("table");
    }
    // A pure array renders as `T[]`.
    if table.fields.is_empty()
        && table.indexers.is_empty()
        && let Some(elem) = &table.array
    {
        return write!(f, "{elem}[]");
    }
    f.write_str("{ ")?;
    let mut first = true;
    let sep = |f: &mut fmt::Formatter<'_>, first: &mut bool| -> fmt::Result {
        if !*first {
            f.write_str(", ")?;
        }
        *first = false;
        Ok(())
    };
    for (name, field) in &table.fields {
        sep(f, &mut first)?;
        let opt = if field.optional { "?" } else { "" };
        write!(f, "{name}{opt}: {}", field.ty)?;
    }
    for (key, value) in &table.indexers {
        sep(f, &mut first)?;
        write!(f, "[{key}]: {value}")?;
    }
    if let Some(elem) = &table.array {
        sep(f, &mut first)?;
        write!(f, "[integer]: {elem}")?;
    }
    f.write_str(" }")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_flattens_and_dedupes() {
        let ty = Ty::union(vec![
            Ty::Number,
            Ty::Union(vec![Ty::String, Ty::Number]),
            Ty::Nil,
        ]);
        assert_eq!(ty, Ty::Union(vec![Ty::Number, Ty::String, Ty::Nil]));
    }

    #[test]
    fn union_of_one_collapses() {
        assert_eq!(Ty::union(vec![Ty::Number, Ty::Number]), Ty::Number);
    }

    #[test]
    fn optional_is_union_with_nil() {
        assert_eq!(Ty::Number.optional(), Ty::Union(vec![Ty::Number, Ty::Nil]));
        // Already-nilable types do not double up.
        assert_eq!(Ty::Nil.optional(), Ty::Nil);
    }

    #[test]
    fn admits_nil() {
        assert!(Ty::Nil.admits_nil());
        assert!(Ty::Number.optional().admits_nil());
        assert!(Ty::Any.admits_nil());
        assert!(!Ty::Number.admits_nil());
    }

    #[test]
    fn display_forms() {
        assert_eq!(Ty::Number.optional().to_string(), "number|nil");
        assert_eq!(Ty::StringLit("on".into()).to_string(), "\"on\"");
        assert_eq!(Ty::any_table().to_string(), "table");
        let array = Ty::Table(Box::new(TableTy {
            array: Some(Ty::String),
            ..TableTy::default()
        }));
        assert_eq!(array.to_string(), "string[]");
        let shape = Ty::Table(Box::new(TableTy {
            fields: [
                (
                    "x".to_string(),
                    FieldTy {
                        ty: Ty::Number,
                        optional: false,
                    },
                ),
                (
                    "y".to_string(),
                    FieldTy {
                        ty: Ty::String,
                        optional: true,
                    },
                ),
            ]
            .into(),
            ..TableTy::default()
        }));
        assert_eq!(shape.to_string(), "{ x: number, y?: string }");
    }
}
