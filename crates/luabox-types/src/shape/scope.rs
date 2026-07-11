//! Lowering `.luab` declarations into the unified type IR (SHAPES-V2.md).
//!
//! A [`ShapeScope`] is the **ambient package scope**: every type declared by
//! every module under `[types] shape-paths`, keyed by fully-qualified name
//! (`geometry.Point`, `love.graphics.Canvas`), plus dependencies' exported
//! surfaces keyed under the dependency's package name. There are no imports;
//! any standard annotation position may name any type in scope.
//!
//! Lowering rules:
//! - Object types lower to *sealed* structural [`TableTy`]s. A method member
//!   lowers to a function-typed field whose leading parameter is literally
//!   named `self`, typed as the enclosing declaration's FQ name — that single
//!   convention carries both `:`-receiver checking and `self` typing.
//! - References to concrete scope types stay nominal ([`Ty::Named`], FQ) and
//!   resolve structurally via the environment; generic references are
//!   **monomorphised per use site** (cycles collapse to `unknown`).
//! - Intersections merge structurally: every member must resolve to a table;
//!   later members win on field conflicts.
//! - `Result<T, E>` keeps the accepted P1 convention: in return position it
//!   lowers to the multi-return pair `(T?, E?)`; anywhere else it degrades to
//!   the union `T | E | nil`.
//!
//! Duplicate FQ declarations are `LB2005` errors at both declaring sites —
//! never silently merged.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use luabox_diag::{Code, Diagnostic, Label, Span};

use super::raw::{RawModule, RawTy, RawTypeDef};
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

const BAD_INSTANTIATION: u16 = 2007;
const DUPLICATE_DECL: u16 = 2005;

/// A lowered `type` declaration. When `params` is non-empty this is a
/// *template*: `ty` contains `Ty::Named(param)` placeholders and must be
/// instantiated via [`ShapeScope::instantiate`].
#[derive(Debug, Clone)]
pub struct TypeShape {
    /// The fully-qualified name (`geometry.Point`).
    pub name: String,
    /// Whether the declaration carries `export` (published surface).
    pub export: bool,
    /// Generic parameter names (empty for a concrete type).
    pub params: Vec<String>,
    /// The lowered right-hand side (placeholders for template params).
    pub ty: Ty,
    /// The `.luab` file declaring the type (diagnostic name).
    pub file: String,
    /// The declaration's byte range within that file.
    pub range: Range<usize>,
}

/// The ambient package scope: every `.luab` type visible to the package.
#[derive(Debug, Default)]
pub struct ShapeScope {
    /// Declarations by fully-qualified name.
    pub types: BTreeMap<String, TypeShape>,
    /// Diagnostics raised while lowering the `.luab` declarations themselves
    /// (duplicates, bad instantiations). Reported when the declaring `.luab`
    /// file is checked — *not* per `.lua` file.
    pub diags: Vec<Diagnostic>,
}

impl ShapeScope {
    /// Whether the scope declares `name` (fully qualified).
    #[must_use]
    pub fn has_type(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    /// The declaration for `name`, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TypeShape> {
        self.types.get(name)
    }

    /// Resolve a (possibly generic) reference to a concrete [`Ty`].
    ///
    /// Concrete types resolve nominally (`Ty::Named(fq)`); templates are
    /// monomorphised with `args` (missing arguments become `unknown`,
    /// leniently; a wrong non-zero count is an `LB2007` at `file`/`range`).
    /// Returns `None` when `name` is not in scope.
    pub fn instantiate(
        &self,
        name: &str,
        args: &[Ty],
        file: &str,
        range: Range<usize>,
        diags: &mut Vec<Diagnostic>,
    ) -> Option<Ty> {
        let shape = self.types.get(name)?;
        if shape.params.is_empty() {
            return Some(Ty::Named(name.to_string()));
        }
        if !args.is_empty() && args.len() != shape.params.len() {
            diags.push(
                Diagnostic::error(
                    Code::new(BAD_INSTANTIATION),
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
            map.insert(param.clone(), args.get(i).cloned().unwrap_or(Ty::Unknown));
        }
        Some(crate::generics::subst_ty(&shape.ty, &map))
    }

    /// Deep-expand [`Ty::Named`] scope references into structural types —
    /// used to build self-contained export surfaces. Cycles collapse to
    /// `unknown`.
    #[must_use]
    pub fn structural(&self, ty: &Ty) -> Ty {
        self.structural_inner(ty, &mut Vec::new())
    }

    fn structural_inner(&self, ty: &Ty, stack: &mut Vec<String>) -> Ty {
        match ty {
            Ty::Named(name) => {
                let Some(shape) = self.types.get(name) else {
                    return ty.clone();
                };
                if stack.iter().any(|n| n == name) {
                    return Ty::Unknown;
                }
                stack.push(name.clone());
                let out = self.structural_inner(&shape.ty.clone(), stack);
                stack.pop();
                out
            }
            Ty::Union(members) => Ty::union(
                members
                    .iter()
                    .map(|m| self.structural_inner(m, stack))
                    .collect(),
            ),
            Ty::Table(table) => {
                let mut out = (**table).clone();
                for field in out.fields.values_mut() {
                    field.ty = self.structural_inner(&field.ty.clone(), stack);
                }
                out.indexers = out
                    .indexers
                    .iter()
                    .map(|(k, v)| {
                        (
                            self.structural_inner(k, stack),
                            self.structural_inner(v, stack),
                        )
                    })
                    .collect();
                out.array = out.array.as_ref().map(|a| self.structural_inner(a, stack));
                Ty::Table(Box::new(out))
            }
            Ty::Function(func) => {
                let mut func = (**func).clone();
                for param in &mut func.params {
                    param.ty = self.structural_inner(&param.ty.clone(), stack);
                }
                func.returns = func
                    .returns
                    .iter()
                    .map(|r| self.structural_inner(r, stack))
                    .collect();
                Ty::Function(Box::new(func))
            }
            _ => ty.clone(),
        }
    }
}

/// Build the ambient package scope: `modules` are the package's own shape
/// modules (namespaces already derived from their paths); `dep_types` are
/// dependencies' exported surfaces, already keyed under their package names
/// (see [`super::ShapeStore::export_surface`]).
pub(crate) fn build_scope(
    modules: &[Arc<RawModule>],
    dep_types: BTreeMap<String, TypeShape>,
) -> ShapeScope {
    let mut scope = ShapeScope {
        types: dep_types,
        diags: Vec::new(),
    };

    // Pass 1: register every FQ name so reference classification during
    // lowering is order-independent, and diagnose duplicates at both sites.
    // A local declaration shadowing a dependency export is a warning, not an
    // error — adding a dependency must not break names the package owned.
    let mut index: BTreeMap<String, (&RawTypeDef, &str)> = BTreeMap::new();
    for module in modules {
        for def in &module.types {
            if def.name.is_empty() {
                continue;
            }
            let fq = fq_name(&module.namespace, &def.name);
            if let Some((first, first_file)) = index.get(&fq) {
                scope.diags.push(
                    Diagnostic::error(
                        Code::new(DUPLICATE_DECL),
                        format!("duplicate declaration of `{fq}`"),
                    )
                    .with_label(Label::primary(
                        Span::new(&module.file, def.range.clone()),
                        "declared again here".to_string(),
                    ))
                    .with_label(Label::secondary(
                        Span::new(*first_file, first.range.clone()),
                        "first declared here".to_string(),
                    ))
                    .with_note(
                        "the package type scope is ambient: every fully-qualified name must be \
                         declared exactly once"
                            .to_string(),
                    ),
                );
                continue;
            }
            if scope.types.contains_key(&fq) {
                scope.diags.push(
                    Diagnostic::warning(
                        Code::new(DUPLICATE_DECL),
                        format!("`{fq}` shadows a type exported by a dependency"),
                    )
                    .with_label(Label::primary(
                        Span::new(&module.file, def.range.clone()),
                        "local declaration wins".to_string(),
                    )),
                );
            }
            index.insert(fq, (def, module.file.as_str()));
        }
    }

    // Pass 2: lower every body against the full index.
    let mut ctx = LowerCtx {
        index: &index,
        namespaces: modules
            .iter()
            .map(|m| m.namespace.clone())
            .collect::<Vec<_>>(),
        stack: Vec::new(),
        range_stack: Vec::new(),
        diags: Vec::new(),
    };
    for module in modules {
        for def in &module.types {
            if def.name.is_empty() {
                continue;
            }
            let fq = fq_name(&module.namespace, &def.name);
            // Skip losers of the duplicate check (the index holds the first
            // declaration; later ones were diagnosed in pass 1).
            if index
                .get(&fq)
                .is_none_or(|(winner, _)| !std::ptr::eq(*winner, def))
            {
                continue;
            }
            ctx.range_stack.push(def.range.clone());
            let ty = def.ty.as_ref().map_or(Ty::Unknown, |t| {
                ctx.lower_rhs(t, &module.namespace, &def.generics, &fq, &module.file)
            });
            ctx.range_stack.pop();
            scope.types.insert(
                fq.clone(),
                TypeShape {
                    name: fq,
                    export: def.export,
                    params: def.generics.clone(),
                    ty,
                    file: module.file.clone(),
                    range: def.range.clone(),
                },
            );
        }
    }

    scope.diags.append(&mut ctx.diags);
    scope
}

/// `ns` + `.` + `name` (or bare `name` for an empty namespace).
pub(crate) fn fq_name(ns: &str, name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}.{name}")
    }
}

struct LowerCtx<'a> {
    /// Every FQ declaration in the package, from pass 1.
    index: &'a BTreeMap<String, (&'a RawTypeDef, &'a str)>,
    /// Every module namespace (collision diagnostics name candidates).
    namespaces: Vec<String>,
    /// Monomorphisation stack (cycle guard for recursive generic types —
    /// a cycle collapses to `unknown`).
    stack: Vec<String>,
    /// Declaration range of the type currently being lowered — the fallback
    /// span for a diagnostic about a member with no range of its own (e.g. an
    /// intersection member that is neither `Named` nor `Object`). Pushed at
    /// every point lowering begins on a (possibly different) declaration's
    /// right-hand side; mirrors the `file` threaded alongside it.
    range_stack: Vec<Range<usize>>,
    diags: Vec<Diagnostic>,
}

impl LowerCtx<'_> {
    /// The declaration range to blame when a member carries none of its own.
    fn current_range(&self) -> Range<usize> {
        self.range_stack.last().cloned().unwrap_or(0..0)
    }
}

impl LowerCtx<'_> {
    /// Lower a declaration's right-hand side. `self_fq` types the `self`
    /// receiver of methods in objects at the top level of the RHS
    /// (including through intersections); nested anonymous objects get
    /// `unknown` receivers.
    fn lower_rhs(
        &mut self,
        raw: &RawTy,
        ns: &str,
        generics: &[String],
        self_fq: &str,
        file: &str,
    ) -> Ty {
        match raw {
            RawTy::Object { .. } => self.lower_object(raw, ns, generics, Some(self_fq), file),
            RawTy::Intersection(members) => {
                self.lower_intersection(members, ns, generics, Some(self_fq), file)
            }
            _ => self.lower(raw, ns, generics, file),
        }
    }

    fn lower(&mut self, raw: &RawTy, ns: &str, generics: &[String], file: &str) -> Ty {
        match raw {
            RawTy::Error => Ty::Unknown,
            RawTy::Optional(inner) => self.lower(inner, ns, generics, file).optional(),
            RawTy::Union(members) => Ty::union(
                members
                    .iter()
                    .map(|m| self.lower(m, ns, generics, file))
                    .collect(),
            ),
            RawTy::Intersection(members) => {
                self.lower_intersection(members, ns, generics, None, file)
            }
            RawTy::Object { .. } => self.lower_object(raw, ns, generics, None, file),
            RawTy::Fn { params, returns } => {
                let params = params
                    .iter()
                    .map(|(name, ty)| ParamTy {
                        name: name.clone(),
                        optional: matches!(ty, RawTy::Optional(_)),
                        ty: self.lower(ty, ns, generics, file),
                    })
                    .collect();
                let returns = self.lower_returns(returns, ns, generics, file);
                Ty::Function(Box::new(FunctionTy {
                    params,
                    returns,
                    has_return_annotation: true,
                    ..FunctionTy::default()
                }))
            }
            RawTy::Named { name, args, range } => {
                self.lower_named(name, args, range.clone(), ns, generics, file)
            }
        }
    }

    /// An object type: a sealed structural table. Methods become
    /// function-typed fields with a leading `self` parameter typed
    /// `self_fq` (or `unknown` in anonymous positions).
    fn lower_object(
        &mut self,
        raw: &RawTy,
        ns: &str,
        generics: &[String],
        self_fq: Option<&str>,
        file: &str,
    ) -> Ty {
        let RawTy::Object {
            fields, methods, ..
        } = raw
        else {
            return Ty::Unknown;
        };
        let mut table = TableTy {
            sealed: true,
            ..TableTy::default()
        };
        for field in fields {
            let optional = matches!(field.ty, RawTy::Optional(_));
            let ty = self.lower(&field.ty, ns, generics, file);
            table
                .fields
                .insert(field.name.clone(), FieldTy { ty, optional });
        }
        for method in methods {
            let mut params: Vec<ParamTy> = Vec::new();
            if method.has_self {
                params.push(ParamTy {
                    name: "self".to_string(),
                    ty: self_fq.map_or(Ty::Unknown, |fq| Ty::Named(fq.to_string())),
                    optional: false,
                });
            }
            params.extend(method.params.iter().map(|(name, ty)| ParamTy {
                name: name.clone(),
                optional: matches!(ty, RawTy::Optional(_)),
                ty: self.lower(ty, ns, generics, file),
            }));
            let returns = self.lower_returns(&method.returns, ns, generics, file);
            table.fields.insert(
                method.name.clone(),
                FieldTy {
                    ty: Ty::Function(Box::new(FunctionTy {
                        params,
                        returns,
                        has_return_annotation: true,
                        ..FunctionTy::default()
                    })),
                    optional: false,
                },
            );
        }
        Ty::Table(Box::new(table))
    }

    /// An intersection merges structurally: every member must resolve to a
    /// table; later members win on field conflicts. A member that does not
    /// resolve to a table degrades the whole intersection to `unknown`
    /// (lenient — the declaring file's own diagnostics cover the mistake).
    fn lower_intersection(
        &mut self,
        members: &[RawTy],
        ns: &str,
        generics: &[String],
        self_fq: Option<&str>,
        file: &str,
    ) -> Ty {
        let mut merged = TableTy {
            sealed: true,
            ..TableTy::default()
        };
        for member in members {
            let lowered = match member {
                RawTy::Object { .. } => self.lower_object(member, ns, generics, self_fq, file),
                _ => self.lower(member, ns, generics, file),
            };
            let resolved = match &lowered {
                Ty::Named(name) => self.resolve_named_to_table(name),
                Ty::Table(t) => Some((**t).clone()),
                _ => None,
            };
            let Some(table) = resolved else {
                // The member itself carries a range when it is `Named`/
                // `Object`; anything else (a primitive, a union, ...) falls
                // back to the enclosing declaration's range.
                let range = member_range(member).unwrap_or_else(|| self.current_range());
                self.diags.push(
                    Diagnostic::error(
                        Code::new(BAD_INSTANTIATION),
                        format!(
                            "intersection member `{}` is not an object type",
                            render_raw_ty(member)
                        ),
                    )
                    .with_label(Label::primary(Span::new(file, range), "not an object type")),
                );
                return Ty::Unknown;
            };
            for (name, field) in table.fields {
                merged.fields.insert(name, field);
            }
            merged.indexers.extend(table.indexers);
            if table.array.is_some() {
                merged.array = table.array;
            }
        }
        Ty::Table(Box::new(merged))
    }

    /// Resolve an FQ scope reference to its (concrete) table body, for
    /// intersection merging. Cycle-guarded; `None` for non-tables.
    fn resolve_named_to_table(&mut self, fq: &str) -> Option<TableTy> {
        let (def, def_file) = self.index.get(fq).copied()?;
        if self.stack.iter().any(|n| n == fq) {
            return None;
        }
        let ns = fq.rsplit_once('.').map_or("", |(ns, _)| ns);
        self.stack.push(fq.to_string());
        self.range_stack.push(def.range.clone());
        let lowered = def.ty.as_ref().map(|t| {
            // Members merged out of a named type keep *their* declaring
            // type as the `self` receiver — `Drawable = Shape & {...}`
            // methods from Shape still take a Shape.
            self.lower_rhs(t, ns, &def.generics, fq, def_file)
        });
        self.range_stack.pop();
        self.stack.pop();
        match lowered {
            Some(Ty::Table(t)) => Some(*t),
            _ => None,
        }
    }

    /// Lower a return list, expanding `Result<T, E>` into `(T?, E?)`
    /// (the accepted P1 convention, unchanged in v2).
    fn lower_returns(
        &mut self,
        returns: &[RawTy],
        ns: &str,
        generics: &[String],
        file: &str,
    ) -> Vec<Ty> {
        let mut out = Vec::new();
        for ret in returns {
            if let RawTy::Named { name, args, .. } = ret
                && name == "Result"
                && args.len() == 2
            {
                out.push(self.lower(&args[0], ns, generics, file).optional());
                out.push(self.lower(&args[1], ns, generics, file).optional());
            } else {
                out.push(self.lower(ret, ns, generics, file));
            }
        }
        out
    }

    /// The `.luab` type-vocabulary builtins: primitives plus the
    /// `Vec`/`HashMap`/`Option`/`Result` constructors (unchanged in v2).
    fn lower_builtin(
        &mut self,
        name: &str,
        args: &[RawTy],
        ns: &str,
        generics: &[String],
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
            "Vec" if args.len() == 1 => Ty::Table(Box::new(TableTy {
                array: Some(self.lower(&args[0], ns, generics, file)),
                ..TableTy::default()
            })),
            "HashMap" if args.len() == 2 => {
                let key = self.lower(&args[0], ns, generics, file);
                let value = self.lower(&args[1], ns, generics, file);
                Ty::Table(Box::new(TableTy {
                    indexers: vec![(key, value)],
                    ..TableTy::default()
                }))
            }
            "Option" if args.len() == 1 => self.lower(&args[0], ns, generics, file).optional(),
            // Outside return position, Result degrades to `T | E | nil`
            // (the pair convention only exists in multi-return position).
            "Result" if args.len() == 2 => {
                let t = self.lower(&args[0], ns, generics, file);
                let e = self.lower(&args[1], ns, generics, file);
                Ty::union(vec![t, e, Ty::Nil])
            }
            _ => return None,
        })
    }

    /// Resolve a written reference: generic parameter → builtin → sibling
    /// (`ns.name`) → as-written FQ name. Unknown names stay lenient
    /// (`unknown`) — the declaring file's diagnostics cover typos.
    fn lower_named(
        &mut self,
        name: &str,
        args: &[RawTy],
        range: Range<usize>,
        ns: &str,
        generics: &[String],
        file: &str,
    ) -> Ty {
        // Generic parameters in scope stay as placeholders.
        if !name.contains('.') && generics.iter().any(|g| g == name) {
            return Ty::Named(name.to_string());
        }
        if let Some(ty) = self.lower_builtin(name, args, ns, generics, file) {
            return ty;
        }

        // Sibling short name, then as-written (already-FQ) name.
        let sibling = fq_name(ns, name);
        let fq = if !name.contains('.') && self.index.contains_key(&sibling) {
            sibling
        } else if self.index.contains_key(name) {
            name.to_string()
        } else {
            // A short name that matches another module's type is a
            // qualification error worth pointing out precisely.
            if !name.contains('.') {
                let candidates: Vec<String> = self
                    .namespaces
                    .iter()
                    .map(|n| fq_name(n, name))
                    .filter(|c| self.index.contains_key(c))
                    .collect();
                if !candidates.is_empty() {
                    self.diags.push(
                        Diagnostic::error(
                            Code::new(BAD_INSTANTIATION),
                            format!("`{name}` is not declared in this module"),
                        )
                        .with_label(Label::primary(
                            Span::new(file, range),
                            format!("did you mean `{}`?", candidates.join("` or `")),
                        ))
                        .with_note(
                            "references outside the declaring module are fully qualified"
                                .to_string(),
                        ),
                    );
                    return Ty::Unknown;
                }
            }
            return Ty::Unknown;
        };

        let (def, _) = self.index.get(&fq).copied().expect("indexed above");
        if def.generics.is_empty() {
            if !args.is_empty() {
                self.diags.push(
                    Diagnostic::error(
                        Code::new(BAD_INSTANTIATION),
                        format!("`{fq}` is not generic but was given type arguments"),
                    )
                    .with_label(Label::primary(
                        Span::new(file, range),
                        "remove the type arguments".to_string(),
                    )),
                );
            }
            return Ty::Named(fq);
        }

        // Monomorphise a template use site (cycles collapse to `unknown`).
        if self.stack.iter().any(|n| n == &fq) {
            return Ty::Unknown;
        }
        if !args.is_empty() && args.len() != def.generics.len() {
            self.diags.push(
                Diagnostic::error(
                    Code::new(BAD_INSTANTIATION),
                    format!(
                        "wrong number of type arguments for `{fq}`: expected {}, found {}",
                        def.generics.len(),
                        args.len()
                    ),
                )
                .with_label(Label::primary(
                    Span::new(file, range),
                    format!("`{fq}` declares {} parameter(s)", def.generics.len()),
                )),
            );
            return Ty::Unknown;
        }
        let lowered_args: Vec<Ty> = args
            .iter()
            .map(|a| self.lower(a, ns, generics, file))
            .collect();
        let def_ns = fq.rsplit_once('.').map_or("", |(ns, _)| ns);
        let def_generics = def.generics.clone();
        let def_ty = def.ty.clone();
        let def_range = def.range.clone();
        let (_, def_file) = self.index.get(&fq).copied().expect("indexed above");
        self.stack.push(fq.clone());
        self.range_stack.push(def_range);
        let template = def_ty.as_ref().map_or(Ty::Unknown, |t| {
            self.lower_rhs(t, def_ns, &def_generics, &fq, def_file)
        });
        self.range_stack.pop();
        self.stack.pop();
        let mut map = BTreeMap::new();
        for (i, param) in def_generics.iter().enumerate() {
            map.insert(
                param.clone(),
                lowered_args.get(i).cloned().unwrap_or(Ty::Unknown),
            );
        }
        crate::generics::subst_ty(&template, &map)
    }
}

/// The written-source range of a `RawTy`, when it carries one of its own
/// (`Named`/`Object`) — `None` for compositions (`Optional`/`Union`/
/// `Intersection`/`Fn`) and `Error`, which don't.
fn member_range(raw: &RawTy) -> Option<Range<usize>> {
    match raw {
        RawTy::Named { range, .. } | RawTy::Object { range, .. } => Some(range.clone()),
        _ => None,
    }
}

/// A short rendering of a `RawTy` for diagnostics naming an offending
/// intersection member.
fn render_raw_ty(raw: &RawTy) -> String {
    match raw {
        RawTy::Named { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!("{name}<...>")
            }
        }
        RawTy::Object { .. } => "{ ... }".to_string(),
        RawTy::Optional(inner) => format!("{}?", render_raw_ty(inner)),
        RawTy::Union(_) => "a union type".to_string(),
        RawTy::Intersection(_) => "an intersection type".to_string(),
        RawTy::Fn { .. } => "a function type".to_string(),
        RawTy::Error => "<error>".to_string(),
    }
}
