//! Lowering `.lb` declarations into the unified type IR (SHAPES.md §2, §5).
//!
//! A [`ShapeScope`] is the merged view of every shape module a file can see
//! (its `---@use` roots plus their transitive `use`s): structs lowered to
//! *sealed* structural [`TableTy`]s, traits to method-set interfaces,
//! aliases expanded to [`Ty`].
//!
//! Generics are **monomorphised per use site**: a generic struct's body is
//! lowered once with `Ty::Named(param)` placeholders, and each use site
//! substitutes real arguments into a clone (bound violations → `LB2007`,
//! reported at the use site, rustc-style).
//!
//! `Result<T, E>` follows the accepted P1 convention (SHAPES.md §12.1): in
//! return position it lowers to the multi-return pair `(T?, E?)`; anywhere
//! else it degrades to the union `T | E | nil`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;

use luabox_diag::{Code, Diagnostic, Label, Span};

use super::raw::{RawGeneric, RawModule, RawTy};
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

const BOUND_UNSATISFIED: u16 = 2007;

/// One generic parameter of a struct/alias template: name plus trait bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParamDef {
    /// The parameter name (`T`).
    pub name: String,
    /// Trait bounds from `T: Shape + Drawable`.
    pub bounds: Vec<String>,
}

/// A lowered `struct` declaration. When `params` is non-empty this is a
/// *template*: `table` contains `Ty::Named(param)` placeholders and must be
/// instantiated via [`ShapeScope::struct_table`].
#[derive(Debug, Clone)]
pub struct StructShape {
    /// The struct name.
    pub name: String,
    /// Generic parameters (empty for a concrete struct).
    pub params: Vec<GenericParamDef>,
    /// The structural shape; `sealed` unless the struct carries `..`.
    pub table: TableTy,
    /// The `.lb` file declaring the struct (diagnostic name).
    pub file: String,
    /// The declaration's byte range within that file.
    pub range: Range<usize>,
}

/// One trait function signature (the `self` receiver is tracked separately
/// from the positional parameters).
#[derive(Debug, Clone)]
pub struct TraitFnSig {
    /// The function name.
    pub name: String,
    /// Whether the first parameter is the `self` receiver — a conforming
    /// implementation must use `:` method syntax (or an explicit leading
    /// `self` parameter).
    pub has_self: bool,
    /// The non-`self` parameters, in order.
    pub params: Vec<ParamTy>,
    /// Declared returns (`Result<T, E>` already expanded to `(T?, E?)`).
    pub returns: Vec<Ty>,
    /// The `.lb` file declaring the trait (for LB2004's secondary label).
    pub file: String,
    /// The signature's byte range within that file.
    pub range: Range<usize>,
}

/// A lowered `trait` declaration: an interface as a method set.
#[derive(Debug, Clone)]
pub struct TraitShape {
    /// The trait name.
    pub name: String,
    /// Direct supertraits (`trait Drawable: Shape` → `["Shape"]`).
    pub supertraits: Vec<String>,
    /// The required functions, by name.
    pub fns: BTreeMap<String, TraitFnSig>,
    /// The `.lb` file declaring the trait.
    pub file: String,
    /// The declaration's byte range within that file.
    pub range: Range<usize>,
}

/// A lowered `type` alias (templates keep placeholder `params`).
#[derive(Debug, Clone)]
pub struct AliasShape {
    /// Generic parameters (empty for a concrete alias).
    pub params: Vec<GenericParamDef>,
    /// The aliased type (placeholders for template params).
    pub ty: Ty,
}

/// The merged, lowered view of a set of shape modules.
#[derive(Debug, Default)]
pub struct ShapeScope {
    /// Structs by name (concrete and templates).
    pub structs: BTreeMap<String, StructShape>,
    /// Traits by name.
    pub traits: BTreeMap<String, TraitShape>,
    /// Aliases by name.
    pub aliases: BTreeMap<String, AliasShape>,
    /// `(trait, struct)` conformance assertions from `impl ...;` items.
    pub impls: BTreeSet<(String, String)>,
    /// Diagnostics raised while lowering the `.lb` declarations themselves
    /// (e.g. `LB2007` at a use site inside a shape file). Reported when the
    /// declaring `.lb` file is checked — *not* per importing `.lua` file.
    pub diags: Vec<Diagnostic>,
}

impl ShapeScope {
    /// Whether any module in scope declares `name` as a struct.
    #[must_use]
    pub fn has_struct(&self, name: &str) -> bool {
        self.structs.contains_key(name)
    }

    /// The concrete table for a struct use site. `args` are the lowered
    /// generic arguments (empty for concrete structs; a template used
    /// without arguments is instantiated with `unknown`, leniently). Bound
    /// violations are appended to `diags` as `LB2007` at `file`/`range`.
    pub fn struct_table(
        &self,
        name: &str,
        args: &[Ty],
        file: &str,
        range: Range<usize>,
        diags: &mut Vec<Diagnostic>,
    ) -> Option<TableTy> {
        let shape = self.structs.get(name)?;
        if shape.params.is_empty() {
            return Some(shape.table.clone());
        }
        if !args.is_empty() && args.len() != shape.params.len() {
            diags.push(
                Diagnostic::error(
                    Code::new(BOUND_UNSATISFIED),
                    format!(
                        "wrong number of type arguments for `{name}`: expected {}, found {}",
                        shape.params.len(),
                        args.len()
                    ),
                )
                .with_label(Label::primary(
                    Span::new(file, range),
                    format!("`{name}` declares {} parameter(s)", shape.params.len()),
                )),
            );
            return None;
        }
        let mut map = BTreeMap::new();
        for (i, param) in shape.params.iter().enumerate() {
            let arg = args.get(i).cloned().unwrap_or(Ty::Unknown);
            if let Some(ty) = args.get(i) {
                for bound in &param.bounds {
                    if !self.conforms(ty, bound) {
                        diags.push(
                            Diagnostic::error(
                                Code::new(BOUND_UNSATISFIED),
                                format!(
                                    "type argument `{ty}` does not satisfy the bound `{bound}` \
                                     required by parameter `{}` of `{name}`",
                                    param.name
                                ),
                            )
                            .with_label(Label::primary(
                                Span::new(file, range.clone()),
                                format!("`{ty}` is not known to implement `{bound}`"),
                            ))
                            .with_note(format!(
                                "conformance comes from an `impl {bound} for {ty};` assertion in \
                                 a shape module"
                            )),
                        );
                    }
                }
            }
            map.insert(param.name.clone(), arg);
        }
        Some(subst_table(&shape.table, &map))
    }

    /// Whether `ty` is known to conform to `bound` (an `impl bound for ty;`
    /// assertion in scope). `unknown`/`any` conform leniently.
    fn conforms(&self, ty: &Ty, bound: &str) -> bool {
        match ty {
            Ty::Unknown | Ty::Any => true,
            Ty::Named(name) => self.impls.contains(&(bound.to_string(), name.clone())),
            _ => false,
        }
    }

    /// Whether `(trait, struct)` conformance is asserted by an `impl` item
    /// in a `.lb` module in scope.
    #[must_use]
    pub fn lb_impl(&self, trait_name: &str, struct_name: &str) -> bool {
        self.impls
            .contains(&(trait_name.to_string(), struct_name.to_string()))
    }

    /// Lower a use-site type (a binding-tag generic argument) against the
    /// already-lowered scope. Bound violations are reported at
    /// `file`/`range` (the tag site).
    pub(crate) fn lower_use_site(
        &self,
        raw: &RawTy,
        file: &str,
        range: &Range<usize>,
        diags: &mut Vec<Diagnostic>,
    ) -> Ty {
        match raw {
            RawTy::Error => Ty::Unknown,
            RawTy::Optional(inner) => self.lower_use_site(inner, file, range, diags).optional(),
            RawTy::Union(members) => Ty::union(
                members
                    .iter()
                    .map(|m| self.lower_use_site(m, file, range, diags))
                    .collect(),
            ),
            RawTy::Fn { params, returns } => {
                let params = params
                    .iter()
                    .map(|(name, ty)| ParamTy {
                        name: name.clone(),
                        ty: self.lower_use_site(ty, file, range, diags),
                        optional: false,
                    })
                    .collect();
                let returns = returns
                    .iter()
                    .map(|r| self.lower_use_site(r, file, range, diags))
                    .collect();
                Ty::Function(Box::new(FunctionTy {
                    params,
                    returns,
                    has_return_annotation: true,
                    ..FunctionTy::default()
                }))
            }
            RawTy::Named { name, args, .. } => {
                let lowered_args: Vec<Ty> = args
                    .iter()
                    .map(|a| self.lower_use_site(a, file, range, diags))
                    .collect();
                match name.as_str() {
                    "number" => Ty::Number,
                    "integer" => Ty::Integer,
                    "string" => Ty::String,
                    "boolean" => Ty::Boolean,
                    "unknown" => Ty::Unknown,
                    "any" => Ty::Any,
                    "nil" => Ty::Nil,
                    "Vec" if lowered_args.len() == 1 => Ty::Table(Box::new(TableTy {
                        array: Some(lowered_args.into_iter().next().expect("one arg")),
                        ..TableTy::default()
                    })),
                    "HashMap" if lowered_args.len() == 2 => {
                        let mut it = lowered_args.into_iter();
                        let key = it.next().expect("two args");
                        let value = it.next().expect("two args");
                        Ty::Table(Box::new(TableTy {
                            indexers: vec![(key, value)],
                            ..TableTy::default()
                        }))
                    }
                    "Option" if lowered_args.len() == 1 => {
                        lowered_args.into_iter().next().expect("one arg").optional()
                    }
                    "Result" if lowered_args.len() == 2 => {
                        let mut members = lowered_args;
                        members.push(Ty::Nil);
                        Ty::union(members)
                    }
                    _ => {
                        if let Some(alias) = self.aliases.get(name) {
                            let mut map = BTreeMap::new();
                            for (i, param) in alias.params.iter().enumerate() {
                                map.insert(
                                    param.name.clone(),
                                    lowered_args.get(i).cloned().unwrap_or(Ty::Unknown),
                                );
                            }
                            return subst_ty(&alias.ty, &map);
                        }
                        if let Some(shape) = self.structs.get(name) {
                            if shape.params.is_empty() {
                                return Ty::Named(name.clone());
                            }
                            return self
                                .struct_table(name, &lowered_args, file, range.clone(), diags)
                                .map_or(Ty::Unknown, |t| Ty::Table(Box::new(t)));
                        }
                        if self.traits.contains_key(name) {
                            return Ty::Named(name.clone());
                        }
                        Ty::Unknown
                    }
                }
            }
        }
    }
}

/// Substitute `Ty::Named(param)` placeholders throughout a type.
pub(crate) fn subst_ty(ty: &Ty, map: &BTreeMap<String, Ty>) -> Ty {
    match ty {
        Ty::Named(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Union(members) => Ty::union(members.iter().map(|m| subst_ty(m, map)).collect()),
        Ty::Table(table) => Ty::Table(Box::new(subst_table(table, map))),
        Ty::Function(func) => {
            let mut func = (**func).clone();
            for param in &mut func.params {
                param.ty = subst_ty(&param.ty, map);
            }
            func.varargs = func.varargs.as_ref().map(|v| subst_ty(v, map));
            func.returns = func.returns.iter().map(|r| subst_ty(r, map)).collect();
            Ty::Function(Box::new(func))
        }
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

/// Build the merged scope from a set of loaded modules (root-first order).
pub(crate) fn build_scope(modules: &[Arc<RawModule>]) -> ShapeScope {
    let mut scope = ShapeScope::default();
    // First pass: register every impl assertion (bound checks during
    // lowering consult them, order-independently).
    for module in modules {
        for imp in &module.impls {
            scope
                .impls
                .insert((imp.trait_name.clone(), imp.struct_name.clone()));
        }
    }

    // Index the raw declarations for template instantiation during lowering.
    let mut ctx = LowerCtx {
        modules,
        impls: scope.impls.clone(),
        stack: Vec::new(),
        alias_stack: Vec::new(),
        diags: Vec::new(),
    };

    for module in modules {
        for raw in &module.structs {
            if raw.name.is_empty() || scope.structs.contains_key(&raw.name) {
                continue;
            }
            let shape = ctx.lower_struct(raw, &module.file);
            scope.structs.insert(raw.name.clone(), shape);
        }
        for raw in &module.traits {
            if raw.name.is_empty() || scope.traits.contains_key(&raw.name) {
                continue;
            }
            let fns = raw
                .fns
                .iter()
                .map(|f| {
                    let bounds = BTreeMap::new();
                    let params = f
                        .params
                        .iter()
                        .map(|(name, ty)| ParamTy {
                            name: name.clone(),
                            ty: ctx.lower(ty, &bounds, &module.file),
                            optional: false,
                        })
                        .collect();
                    let returns = ctx.lower_returns(&f.returns, &bounds, &module.file);
                    (
                        f.name.clone(),
                        TraitFnSig {
                            name: f.name.clone(),
                            has_self: f.has_self,
                            params,
                            returns,
                            file: module.file.clone(),
                            range: f.range.clone(),
                        },
                    )
                })
                .collect();
            scope.traits.insert(
                raw.name.clone(),
                TraitShape {
                    name: raw.name.clone(),
                    supertraits: raw.supertraits.clone(),
                    fns,
                    file: module.file.clone(),
                    range: raw.range.clone(),
                },
            );
        }
        for raw in &module.aliases {
            if scope.aliases.contains_key(&raw.name) {
                continue;
            }
            let bounds = bounds_of(&raw.generics);
            let ty = raw
                .ty
                .as_ref()
                .map_or(Ty::Unknown, |t| ctx.lower(t, &bounds, &module.file));
            scope.aliases.insert(
                raw.name.clone(),
                AliasShape {
                    params: generic_defs(&raw.generics),
                    ty,
                },
            );
        }
    }

    scope.diags = ctx.diags;
    scope
}

fn generic_defs(generics: &[RawGeneric]) -> Vec<GenericParamDef> {
    generics
        .iter()
        .map(|g| GenericParamDef {
            name: g.name.clone(),
            bounds: g.bounds.clone(),
        })
        .collect()
}

/// The `param -> bounds` map for generic parameters currently in scope
/// (placeholder arguments are checked against these instead of `impl`s).
fn bounds_of(generics: &[RawGeneric]) -> BTreeMap<String, Vec<String>> {
    generics
        .iter()
        .map(|g| (g.name.clone(), g.bounds.clone()))
        .collect()
}

struct LowerCtx<'a> {
    modules: &'a [Arc<RawModule>],
    impls: BTreeSet<(String, String)>,
    /// Struct-template instantiation stack (cycle guard for recursive
    /// generic structs — a cycle collapses to `unknown`).
    stack: Vec<String>,
    /// Alias expansion stack (cycle guard, like LuaCATS aliases).
    alias_stack: Vec<String>,
    diags: Vec<Diagnostic>,
}

impl LowerCtx<'_> {
    fn lower_struct(&mut self, raw: &super::raw::RawStruct, file: &str) -> StructShape {
        let bounds = bounds_of(&raw.generics);
        let mut table = TableTy {
            sealed: !raw.open,
            ..TableTy::default()
        };
        for field in &raw.fields {
            let optional = matches!(field.ty, RawTy::Optional(_));
            let ty = self.lower(&field.ty, &bounds, file);
            table
                .fields
                .insert(field.name.clone(), FieldTy { ty, optional });
        }
        StructShape {
            name: raw.name.clone(),
            params: generic_defs(&raw.generics),
            table,
            file: file.to_string(),
            range: raw.range.clone(),
        }
    }

    fn find_struct(&self, name: &str) -> Option<(&super::raw::RawStruct, &str)> {
        self.modules.iter().find_map(|m| {
            m.structs
                .iter()
                .find(|s| s.name == name)
                .map(|s| (s, m.file.as_str()))
        })
    }

    fn find_alias(&self, name: &str) -> Option<(&super::raw::RawAlias, &str)> {
        self.modules.iter().find_map(|m| {
            m.aliases
                .iter()
                .find(|a| a.name == name)
                .map(|a| (a, m.file.as_str()))
        })
    }

    fn is_trait(&self, name: &str) -> bool {
        self.modules
            .iter()
            .any(|m| m.traits.iter().any(|t| t.name == name))
    }

    /// Lower a return list, expanding `Result<T, E>` into `(T?, E?)`
    /// (SHAPES.md §12.1, the accepted P1 convention).
    fn lower_returns(
        &mut self,
        returns: &[RawTy],
        bounds: &BTreeMap<String, Vec<String>>,
        file: &str,
    ) -> Vec<Ty> {
        let mut out = Vec::new();
        for ret in returns {
            if let RawTy::Named { name, args, .. } = ret
                && name == "Result"
                && args.len() == 2
            {
                out.push(self.lower(&args[0], bounds, file).optional());
                out.push(self.lower(&args[1], bounds, file).optional());
            } else {
                out.push(self.lower(ret, bounds, file));
            }
        }
        out
    }

    #[allow(clippy::too_many_lines)]
    fn lower(&mut self, raw: &RawTy, bounds: &BTreeMap<String, Vec<String>>, file: &str) -> Ty {
        match raw {
            RawTy::Error => Ty::Unknown,
            RawTy::Optional(inner) => self.lower(inner, bounds, file).optional(),
            RawTy::Union(members) => Ty::union(
                members
                    .iter()
                    .map(|m| self.lower(m, bounds, file))
                    .collect(),
            ),
            RawTy::Fn { params, returns } => {
                let params = params
                    .iter()
                    .map(|(name, ty)| ParamTy {
                        name: name.clone(),
                        ty: self.lower(ty, bounds, file),
                        optional: false,
                    })
                    .collect();
                let returns = self.lower_returns(returns, bounds, file);
                Ty::Function(Box::new(FunctionTy {
                    params,
                    returns,
                    has_return_annotation: true,
                    ..FunctionTy::default()
                }))
            }
            RawTy::Named { name, args, range } => {
                self.lower_named(name, args, range.clone(), bounds, file)
            }
        }
    }

    /// The `.lb` type-vocabulary builtins (SHAPES.md §3): primitives plus
    /// the `Vec`/`HashMap`/`Option`/`Result` constructors.
    fn lower_builtin(
        &mut self,
        name: &str,
        args: &[RawTy],
        bounds: &BTreeMap<String, Vec<String>>,
        file: &str,
    ) -> Option<Ty> {
        Some(match name {
            "number" => Ty::Number,
            "integer" => Ty::Integer,
            "string" => Ty::String,
            "boolean" => Ty::Boolean,
            "unknown" => Ty::Unknown,
            "any" => Ty::Any,
            "nil" => Ty::Nil,
            "Self" => Ty::Named("Self".to_string()),
            "Vec" if args.len() == 1 => Ty::Table(Box::new(TableTy {
                array: Some(self.lower(&args[0], bounds, file)),
                ..TableTy::default()
            })),
            "HashMap" if args.len() == 2 => {
                let key = self.lower(&args[0], bounds, file);
                let value = self.lower(&args[1], bounds, file);
                Ty::Table(Box::new(TableTy {
                    indexers: vec![(key, value)],
                    ..TableTy::default()
                }))
            }
            "Option" if args.len() == 1 => self.lower(&args[0], bounds, file).optional(),
            // Outside return position, Result degrades to `T | E | nil`
            // (SHAPES.md §12.1 — the pair convention only exists in
            // multi-return position).
            "Result" if args.len() == 2 => {
                let t = self.lower(&args[0], bounds, file);
                let e = self.lower(&args[1], bounds, file);
                Ty::union(vec![t, e, Ty::Nil])
            }
            _ => return None,
        })
    }

    fn lower_named(
        &mut self,
        name: &str,
        args: &[RawTy],
        range: Range<usize>,
        bounds: &BTreeMap<String, Vec<String>>,
        file: &str,
    ) -> Ty {
        // Generic parameters in scope stay as placeholders.
        if bounds.contains_key(name) {
            return Ty::Named(name.to_string());
        }
        if let Some(ty) = self.lower_builtin(name, args, bounds, file) {
            return ty;
        }

        // Alias expansion (monomorphised for generic aliases; cycles
        // collapse to `unknown` like LuaCATS aliases).
        if let Some((alias, alias_file)) = self.find_alias(name) {
            if self.alias_stack.iter().any(|n| n == name) {
                return Ty::Unknown;
            }
            let alias = alias.clone();
            let alias_file = alias_file.to_string();
            let alias_bounds = bounds_of(&alias.generics);
            let Some(body) = &alias.ty else {
                return Ty::Unknown;
            };
            self.alias_stack.push(name.to_string());
            let lowered = self.lower(body, &alias_bounds, &alias_file);
            self.alias_stack.pop();
            let mut map = BTreeMap::new();
            for (i, generic) in alias.generics.iter().enumerate() {
                let arg = args
                    .get(i)
                    .map_or(Ty::Unknown, |a| self.lower(a, bounds, file));
                self.check_bounds(
                    &arg,
                    &generic.bounds,
                    bounds,
                    name,
                    &generic.name,
                    &range,
                    file,
                );
                map.insert(generic.name.clone(), arg);
            }
            return subst_ty(&lowered, &map);
        }

        // Struct references: concrete stays nominal; generic instantiates.
        if let Some((raw_struct, struct_file)) = self.find_struct(name) {
            if raw_struct.generics.is_empty() {
                return Ty::Named(name.to_string());
            }
            if self.stack.iter().any(|n| n == name) {
                // Recursive generic struct: collapse to `unknown`.
                return Ty::Unknown;
            }
            let raw_struct = raw_struct.clone();
            let struct_file = struct_file.to_string();
            self.stack.push(name.to_string());
            let template = self.lower_struct(&raw_struct, &struct_file);
            self.stack.pop();
            let mut map = BTreeMap::new();
            for (i, generic) in raw_struct.generics.iter().enumerate() {
                let arg = args
                    .get(i)
                    .map_or(Ty::Unknown, |a| self.lower(a, bounds, file));
                self.check_bounds(
                    &arg,
                    &generic.bounds,
                    bounds,
                    name,
                    &generic.name,
                    &range,
                    file,
                );
                map.insert(generic.name.clone(), arg);
            }
            return Ty::Table(Box::new(subst_table(&template.table, &map)));
        }

        if self.is_trait(name) {
            return Ty::Named(name.to_string());
        }

        // Unknown name: lenient `unknown` (P2 will resolve dependency
        // shapes; the declaring file's own diagnostics cover typos).
        Ty::Unknown
    }

    /// Check one instantiation argument against its parameter's bounds
    /// (`LB2007` at the use site on violation). A placeholder argument
    /// conforms when its own declared bounds include the required one.
    #[allow(clippy::too_many_arguments)]
    fn check_bounds(
        &mut self,
        arg: &Ty,
        required: &[String],
        in_scope: &BTreeMap<String, Vec<String>>,
        owner: &str,
        param: &str,
        range: &Range<usize>,
        file: &str,
    ) {
        for bound in required {
            let ok = match arg {
                Ty::Unknown | Ty::Any => true,
                Ty::Named(n) => {
                    in_scope.get(n).is_some_and(|bs| bs.contains(bound))
                        || self.impls.contains(&(bound.clone(), n.clone()))
                }
                _ => false,
            };
            if !ok {
                self.diags.push(
                    Diagnostic::error(
                        Code::new(BOUND_UNSATISFIED),
                        format!(
                            "type argument `{arg}` does not satisfy the bound `{bound}` required \
                             by parameter `{param}` of `{owner}`"
                        ),
                    )
                    .with_label(Label::primary(
                        Span::new(file, range.clone()),
                        format!("`{arg}` is not known to implement `{bound}`"),
                    ))
                    .with_note(format!(
                        "conformance comes from an `impl {bound} for {arg};` assertion in a \
                         shape module"
                    )),
                );
            }
        }
    }
}
