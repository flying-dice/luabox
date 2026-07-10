//! Checking `.lua` binding tags against `.luab` shapes (SHAPES.md §4, §5).
//!
//! Emits the `LB2xxx` family. Shape rules are **hard errors at every
//! strictness level** — `---@struct`/`---@impl` are themselves the opt-in,
//! so the strictness ladder's severity mapping is bypassed (only ordinary
//! field-value type mismatches, reported as `LB0300`, keep strictness
//! semantics).
//!
//! What is checked (the P1 statically-visible subset):
//!
//! - `---@struct` naming an undeclared struct → `LB2006`.
//! - A shape-bound *data* table literal: missing non-optional fields →
//!   `LB2001`; unknown keys on a sealed struct → `LB2002`; field values →
//!   `LB0300` at the file's strictness.
//! - `setmetatable(literal, Carrier)` — the literal is checked against the
//!   carrier's struct fields; the result (and `self` inside the carrier's
//!   methods) is typed as an instance of the struct.
//! - Field reads/writes on shape-bound values visible in the same file:
//!   unknown keys on sealed shapes → `LB2002` (carrier method names and
//!   `__`-prefixed metafields are allowed).
//! - `---@impl Trait for Struct` coherence: completeness → `LB2003`
//!   (listing the missing functions); signature compatibility (params
//!   contravariant, returns covariant, `:` vs `.` receiver matching
//!   `self`) → `LB2004` with both spans; supertrait conformance on the
//!   same carrier → `LB2008`. Extra inherent methods are fine. A
//!   `---@class` table satisfies a trait the same way (interop): its
//!   methods come from `function Class:m()` declarations and from
//!   function-typed `---@field`s.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use luabox_diag::{Code, Diagnostic, Label, Severity, Span};
use luabox_syntax::lua::ast::{AstNode, Expr, Stmt, TableExpr};
use luabox_syntax::lua::{self, SyntaxNode};
use luabox_syntax::luacats::{self, AnnotatedItem, Tag};

use super::raw;
use super::scope::{ShapeScope, subst_ty};
use crate::env::{self, TypeEnv};
use crate::ty::{FunctionTy, TableTy, Ty};
use crate::{Strictness, assignable};

const TYPE_MISMATCH: u16 = 300;
const MISSING_FIELD: u16 = 2001;
const UNKNOWN_KEY: u16 = 2002;
const INCOMPLETE_IMPL: u16 = 2003;
const SIGNATURE_MISMATCH: u16 = 2004;
const UNDECLARED_NAME: u16 = 2006;

/// A `---@struct` binding resolved to its struct.
struct StructBinding {
    struct_name: String,
    table: TableTy,
    /// The local (or carrier) name the tag binds, when determinable.
    local: Option<String>,
    /// Whether the bound table is a class carrier (instances have the
    /// shape) rather than a plain data table (the table itself must match).
    carrier: bool,
}

/// One method found on a carrier in this file.
#[derive(Clone)]
struct MethodInfo {
    /// Declared with `:` (implicit `self`).
    colon: bool,
    /// Declared with `.` but takes an explicit leading `self` parameter.
    explicit_self: bool,
    /// Non-`self` parameter types (annotated, or `unknown`).
    params: Vec<Ty>,
    /// Whether the parameter list ends with `...`.
    has_vararg: bool,
    /// Declared returns, when a `---@return` annotation exists.
    returns: Option<Vec<Ty>>,
    /// Byte range of the declaration in the `.lua` file (`None` for
    /// methods synthesized from `---@field` function types).
    range: Option<Range<usize>>,
}

pub(crate) fn run(
    parse: &lua::Parse,
    items: &[AnnotatedItem],
    scope: &ShapeScope,
    typeenv: &TypeEnv,
    file: &str,
    strictness: Strictness,
) -> Vec<Diagnostic> {
    let root = parse.syntax();
    let mut checker = ShapeChecker {
        scope,
        env: typeenv,
        file,
        strictness,
        diags: Vec::new(),
        methods: HashMap::new(),
        carrier_hints: HashSet::new(),
        carriers: HashMap::new(),
        values: HashMap::new(),
        lua_impls: HashSet::new(),
    };

    checker.collect_facts(&root);
    checker.bind_tags(items, &root);
    checker.check_instantiations(&root);
    checker.check_field_access(&root);
    checker.check_impls(items, &root);
    checker.diags
}

/// What a shape-bound name refers to during field-access checking.
enum BoundValue {
    /// A class carrier — the carrier table itself is not sealed-checked
    /// (it holds methods and metatable machinery).
    Carrier,
    /// A plain data table or a struct instance: sealed rules apply.
    Shaped { struct_name: String, table: TableTy },
}

struct ShapeChecker<'a> {
    scope: &'a ShapeScope,
    env: &'a TypeEnv,
    file: &'a str,
    strictness: Strictness,
    diags: Vec<Diagnostic>,
    /// Methods per carrier/base name: `function X:m()`, `function X.m()`,
    /// `X.m = function`, and function-valued literal fields.
    methods: HashMap<String, BTreeMap<String, MethodInfo>>,
    /// Names that structurally look like class carriers (method decls,
    /// `X.__index` writes, `setmetatable(_, X)` usage).
    carrier_hints: HashSet<String>,
    /// Carrier name → struct it carries (from `---@struct` bindings).
    carriers: HashMap<String, (String, TableTy)>,
    /// Local name → what it holds (data tables and instances).
    values: HashMap<String, BoundValue>,
    /// `(trait, struct)` conformances asserted by `---@impl` in this file.
    lua_impls: HashSet<(String, String)>,
}

impl ShapeChecker<'_> {
    fn error(&mut self, code: u16, range: Range<usize>, message: String, label: String) {
        self.diags.push(
            Diagnostic::error(Code::new(code), message)
                .with_label(Label::primary(Span::new(self.file, range), label)),
        );
    }

    // --- pass 1: structural facts ----------------------------------------

    /// One walk collecting carrier hints and the per-carrier method table.
    fn collect_facts(&mut self, root: &SyntaxNode) {
        for node in root.descendants() {
            self.collect_fact(&node);
        }
    }

    fn collect_fact(&mut self, node: &SyntaxNode) {
        let Some(stmt) = Stmt::cast(node.clone()) else {
            if let Some(Expr::Call(call)) = Expr::cast(node.clone())
                && let Some(name) = setmetatable_carrier(&call)
            {
                self.carrier_hints.insert(name);
            }
            return;
        };
        match stmt {
            Stmt::FunctionDecl(func) => {
                let Some(name) = func.name() else { return };
                let segments: Vec<String> = name.segments().map(|s| s.text().to_string()).collect();
                if segments.len() != 2 {
                    return;
                }
                let base = segments[0].clone();
                let method = segments[1].clone();
                self.carrier_hints.insert(base.clone());
                let info = self.method_info_from_decl(&func, name.is_method(), node);
                self.methods.entry(base).or_default().insert(method, info);
            }
            Stmt::Assign(assign) => {
                let targets: Vec<Expr> = assign
                    .targets()
                    .map(|t| t.exprs().collect())
                    .unwrap_or_default();
                let values: Vec<Expr> = assign
                    .values()
                    .map(|v| v.exprs().collect())
                    .unwrap_or_default();
                for (i, target) in targets.iter().enumerate() {
                    let Expr::Field(field) = target else { continue };
                    let (Some(Expr::Name(base)), Some(member)) = (field.base(), field.field_name())
                    else {
                        continue;
                    };
                    let Some(base) = base.name() else { continue };
                    if member.text() == "__index" {
                        self.carrier_hints.insert(base.text().to_string());
                    }
                    if let Some(Expr::Function(func)) = values.get(i) {
                        let info = method_info_from_params(func.param_list(), false);
                        self.methods
                            .entry(base.text().to_string())
                            .or_default()
                            .insert(member.text().to_string(), info);
                    }
                }
            }
            _ => {}
        }
    }

    /// Method info for a `function X:m()` / `function X.m()` declaration,
    /// merging the `---@param`/`---@return` signature when one exists.
    fn method_info_from_decl(
        &self,
        func: &lua::ast::FunctionDeclStmt,
        colon: bool,
        stmt_node: &SyntaxNode,
    ) -> MethodInfo {
        let mut info = method_info_from_params(func.param_list(), colon);
        info.range = Some(node_range(stmt_node));
        let key = {
            let r = stmt_node.text_range();
            (usize::from(r.start()), usize::from(r.end()))
        };
        if let Some(sig) = self.env.fn_sig(key) {
            // Types by name — the reconciled signature covers the AST list.
            let by_name: HashMap<&str, &Ty> = sig
                .params
                .iter()
                .map(|p| (p.name.as_str(), &p.ty))
                .collect();
            let names = ast_param_names(func.param_list());
            info.params = names
                .iter()
                .filter(|n| *n != "self" && *n != "...")
                .map(|n| {
                    by_name
                        .get(n.as_str())
                        .copied()
                        .cloned()
                        .unwrap_or(Ty::Unknown)
                })
                .collect();
            if sig.has_return_annotation {
                info.returns = Some(sig.returns.clone());
            }
        }
        info
    }

    // --- pass 2: `---@struct` bindings -------------------------------------

    fn bind_tags(&mut self, items: &[AnnotatedItem], root: &SyntaxNode) {
        for item in items {
            for tag in &item.block.tags {
                match tag {
                    Tag::Struct(struct_tag) if !struct_tag.name.is_empty() => {
                        self.bind_struct(struct_tag, item.target, root);
                    }
                    Tag::Impl(impl_tag)
                        if !impl_tag.trait_name.is_empty() && !impl_tag.struct_name.is_empty() =>
                    {
                        self.lua_impls
                            .insert((impl_tag.trait_name.clone(), impl_tag.struct_name.clone()));
                    }
                    _ => {}
                }
            }
        }
    }

    fn bind_struct(
        &mut self,
        tag: &luacats::StructTag,
        target: Option<luacats::Span>,
        root: &SyntaxNode,
    ) {
        let tag_range = tag.span.start..tag.span.end;
        if !self.scope.has_struct(&tag.name) {
            self.error(
                UNDECLARED_NAME,
                tag_range,
                format!("`---@struct {}` names an undeclared struct", tag.name),
                "no shape module in scope declares this struct".to_string(),
            );
            return;
        }
        let args: Vec<Ty> = match &tag.args {
            None => Vec::new(),
            Some(text) => match raw::parse_type_args(text) {
                Some(raws) => raws
                    .iter()
                    .map(|r| {
                        self.scope
                            .lower_use_site(r, self.file, &tag_range, &mut self.diags)
                    })
                    .collect(),
                None => Vec::new(),
            },
        };
        let Some(table) =
            self.scope
                .struct_table(&tag.name, &args, self.file, tag_range, &mut self.diags)
        else {
            return;
        };

        let binding = self.classify_binding(&tag.name, table, target, root);
        if let Some(local) = &binding.local {
            if binding.carrier {
                self.carriers.insert(
                    local.clone(),
                    (binding.struct_name.clone(), binding.table.clone()),
                );
                self.values.insert(local.clone(), BoundValue::Carrier);
            } else {
                self.values.insert(
                    local.clone(),
                    BoundValue::Shaped {
                        struct_name: binding.struct_name,
                        table: binding.table,
                    },
                );
            }
        }
    }

    /// Decide carrier vs data table and run the data-literal check.
    fn classify_binding(
        &mut self,
        struct_name: &str,
        table: TableTy,
        target: Option<luacats::Span>,
        root: &SyntaxNode,
    ) -> StructBinding {
        let stmt = target.and_then(|span| stmt_at(root, (span.start, span.end)));
        let (local, value) = match &stmt {
            Some(Stmt::Local(local)) => (
                local
                    .names()
                    .next()
                    .and_then(|n| n.name())
                    .map(|t| t.text().to_string()),
                local.values().and_then(|v| v.exprs().next()),
            ),
            Some(Stmt::FunctionDecl(func)) => (
                func.name()
                    .and_then(|n| n.segments().next())
                    .map(|t| t.text().to_string()),
                None,
            ),
            _ => (None, None),
        };

        let carrier = match (&local, &value) {
            (Some(name), Some(Expr::Table(lit))) => {
                self.carrier_hints.contains(name) || lit.fields().next().is_none()
            }
            (Some(name), _) => self.carrier_hints.contains(name),
            _ => false,
        };

        if !carrier && let Some(Expr::Table(lit)) = &value {
            let methods = self
                .methods
                .get(local.as_deref().unwrap_or_default())
                .map(|m| m.keys().cloned().collect::<BTreeSet<_>>())
                .unwrap_or_default();
            self.check_literal(lit, struct_name, &table, &methods);
        }

        StructBinding {
            struct_name: struct_name.to_string(),
            table,
            local,
            carrier,
        }
    }

    // --- pass 3: `setmetatable(literal, Carrier)` --------------------------

    fn check_instantiations(&mut self, root: &SyntaxNode) {
        // Instances bound by `local x = setmetatable(...)`.
        let mut instance_locals: Vec<(String, String, TableTy)> = Vec::new();
        for node in root.descendants() {
            if let Some(Expr::Call(call)) = Expr::cast(node.clone())
                && let Some(carrier) = setmetatable_carrier(&call)
                && let Some((struct_name, table)) = self.carriers.get(&carrier).cloned()
            {
                if let Some(Expr::Table(lit)) = first_arg(&call) {
                    let methods = self.method_names(&carrier);
                    self.check_literal(&lit, &struct_name, &table, &methods);
                }
                // `local x = setmetatable(...)` binds an instance.
                if let Some(local) = node
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(lua::ast::LocalStmt::cast)
                    && let Some(name) = local.names().next().and_then(|n| n.name())
                {
                    instance_locals.push((name.text().to_string(), struct_name, table));
                }
            }
        }
        for (name, struct_name, table) in instance_locals {
            self.values
                .entry(name)
                .or_insert(BoundValue::Shaped { struct_name, table });
        }
    }

    /// The method names visible on the carrier for `struct_name`-instances.
    fn method_names(&self, carrier: &str) -> BTreeSet<String> {
        self.methods
            .get(carrier)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    // --- pass 4: sealed field reads/writes ---------------------------------

    fn check_field_access(&mut self, root: &SyntaxNode) {
        if self.values.is_empty() && self.carriers.is_empty() {
            return;
        }
        for node in root.descendants() {
            // Field-write type checks (`x.f = v` against the field type).
            if let Some(Stmt::Assign(assign)) = Stmt::cast(node.clone()) {
                self.check_field_writes(&assign);
                continue;
            }
            let Some(Expr::Field(field)) = Expr::cast(node.clone()) else {
                continue;
            };
            let (Some(base), Some(member)) = (field.base(), field.field_name()) else {
                continue;
            };
            let Expr::Name(base_name) = base else {
                continue;
            };
            let Some(base_token) = base_name.name() else {
                continue;
            };
            let key = member.text().to_string();
            if key.starts_with("__") {
                continue; // metatable machinery is never sealed-checked
            }
            let Some((struct_name, table, carrier_name)) =
                self.shaped_base(base_token.text(), &node)
            else {
                continue;
            };
            if !table.sealed || table.fields.contains_key(&key) {
                continue;
            }
            if self.method_names(&carrier_name).contains(&key) {
                continue; // instance method access via `__index`
            }
            self.error(
                UNKNOWN_KEY,
                node_range(field.syntax()),
                format!("unknown key `{key}` on sealed struct `{struct_name}`"),
                format!("`{struct_name}` declares no field `{key}`"),
            );
        }
    }

    /// Resolve the shaped value a name refers to at this node: a bound
    /// local, or `self` inside a carrier's method. Returns the struct
    /// name, its table, and the carrier whose methods are allowed.
    fn shaped_base(&self, name: &str, node: &SyntaxNode) -> Option<(String, TableTy, String)> {
        if name == "self" {
            let func = node
                .ancestors()
                .find_map(lua::ast::FunctionDeclStmt::cast)?;
            let base = func.name()?.segments().next()?.text().to_string();
            let (struct_name, table) = self.carriers.get(&base)?.clone();
            return Some((struct_name, table, base));
        }
        match self.values.get(name)? {
            BoundValue::Carrier => None, // carriers themselves are open
            BoundValue::Shaped { struct_name, table } => {
                // Allowed methods: the carrier carrying this struct, or the
                // bound name itself for data tables with attached helpers.
                let carrier = self
                    .carriers
                    .iter()
                    .find(|(_, (s, _))| s == struct_name)
                    .map_or_else(|| name.to_string(), |(c, _)| c.clone());
                Some((struct_name.clone(), table.clone(), carrier))
            }
        }
    }

    /// Known-field writes get a value type check at the file's strictness.
    fn check_field_writes(&mut self, assign: &lua::ast::AssignStmt) {
        if self.strictness == Strictness::None {
            return;
        }
        let targets: Vec<Expr> = assign
            .targets()
            .map(|t| t.exprs().collect())
            .unwrap_or_default();
        let values: Vec<Expr> = assign
            .values()
            .map(|v| v.exprs().collect())
            .unwrap_or_default();
        for (i, target) in targets.iter().enumerate() {
            let Expr::Field(field) = target else { continue };
            let (Some(Expr::Name(base)), Some(member)) = (field.base(), field.field_name()) else {
                continue;
            };
            let Some(base) = base.name() else { continue };
            let Some((_, table, _)) = self.shaped_base(base.text(), field.syntax()) else {
                continue;
            };
            let Some(decl) = table.fields.get(member.text()) else {
                continue;
            };
            let expected = if decl.optional {
                decl.ty.clone().optional()
            } else {
                decl.ty.clone()
            };
            if let Some(value) = values.get(i) {
                self.check_value(value, &expected);
            }
        }
    }

    // --- literal checking ---------------------------------------------------

    /// Check a table literal against a struct's fields: `LB2001` for
    /// missing non-optional fields, `LB2002` for unknown keys on a sealed
    /// struct, `LB0300` (strictness-mapped) for field value mismatches.
    fn check_literal(
        &mut self,
        lit: &TableExpr,
        struct_name: &str,
        table: &TableTy,
        allowed_methods: &BTreeSet<String>,
    ) {
        let mut present: BTreeMap<String, (Option<Expr>, Range<usize>)> = BTreeMap::new();
        for field in lit.fields() {
            match &field {
                lua::ast::TableField::Name(f) => {
                    if let Some(name) = f.name() {
                        present.insert(
                            name.text().to_string(),
                            (f.value(), node_range(field.syntax())),
                        );
                    }
                }
                lua::ast::TableField::Key(f) => {
                    if let Some(Ty::StringLit(name)) = f.key().as_ref().and_then(env::literal_ty) {
                        present.insert(name, (f.value(), node_range(field.syntax())));
                    }
                }
                lua::ast::TableField::Item(_) => {}
            }
        }

        for (name, decl) in &table.fields {
            if !decl.optional && !decl.ty.admits_nil() && !present.contains_key(name) {
                self.diags.push(
                    Diagnostic::error(
                        Code::new(MISSING_FIELD),
                        format!(
                            "missing non-optional field `{name}` on a value bound to struct \
                             `{struct_name}`"
                        ),
                    )
                    .with_label(Label::primary(
                        Span::new(self.file, node_range(lit.syntax())),
                        format!("`{struct_name}` requires `{name}: {}`", decl.ty),
                    )),
                );
            }
        }

        for (name, (value, range)) in &present {
            if let Some(decl) = table.fields.get(name) {
                let expected = if decl.optional {
                    decl.ty.clone().optional()
                } else {
                    decl.ty.clone()
                };
                if let Some(value) = value {
                    self.check_field_value(value, &expected);
                }
                continue;
            }
            if !table.sealed || name.starts_with("__") || allowed_methods.contains(name) {
                continue;
            }
            self.error(
                UNKNOWN_KEY,
                range.clone(),
                format!("unknown key `{name}` on sealed struct `{struct_name}`"),
                format!("`{struct_name}` declares no field `{name}`"),
            );
        }
    }

    /// A field value: nested literals recurse into sealed checking; other
    /// values get a strictness-mapped assignability check.
    fn check_field_value(&mut self, value: &Expr, expected: &Ty) {
        if let Expr::Table(nested) = value {
            let resolved = match unwrap_optional(expected) {
                Ty::Named(name) => self.env.class_shape(name),
                Ty::Table(table) => Some((**table).clone()),
                _ => None,
            };
            if let Some(table) = resolved {
                let display = match unwrap_optional(expected) {
                    Ty::Named(name) => name.clone(),
                    other => other.to_string(),
                };
                self.check_literal(nested, &display, &table, &BTreeSet::new());
                return;
            }
        }
        self.check_value(value, expected);
    }

    /// Assignability of a simple value expression, reported as `LB0300`
    /// at the file's strictness (suppressed entirely at `none`).
    fn check_value(&mut self, value: &Expr, expected: &Ty) {
        if self.strictness == Strictness::None {
            return;
        }
        let strict = self.strictness == Strictness::Strict;
        let found = simple_expr_ty(value);
        if !assignable(self.env, strict, &found, expected) {
            let severity = if strict {
                Severity::Error
            } else {
                Severity::Warning
            };
            self.diags.push(
                Diagnostic::new(
                    Code::new(TYPE_MISMATCH),
                    severity,
                    format!("type mismatch: expected `{expected}`, found `{found}`"),
                )
                .with_label(Label::primary(
                    Span::new(self.file, node_range(value.syntax())),
                    format!("expected `{expected}`"),
                )),
            );
        }
    }

    // --- pass 5: `---@impl` coherence ---------------------------------------

    fn check_impls(&mut self, items: &[AnnotatedItem], root: &SyntaxNode) {
        for item in items {
            for tag in &item.block.tags {
                let Tag::Impl(impl_tag) = tag else { continue };
                if impl_tag.trait_name.is_empty() || impl_tag.struct_name.is_empty() {
                    continue;
                }
                self.check_impl(impl_tag, item.target, root);
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn check_impl(
        &mut self,
        tag: &luacats::ImplTag,
        target: Option<luacats::Span>,
        root: &SyntaxNode,
    ) {
        let tag_range = tag.span.start..tag.span.end;
        let Some(trait_shape) = self.scope.traits.get(&tag.trait_name).cloned() else {
            self.error(
                UNDECLARED_NAME,
                tag_range,
                format!("`---@impl` names an undeclared trait `{}`", tag.trait_name),
                "no shape module in scope declares this trait".to_string(),
            );
            return;
        };
        // Interop: the implementing type may be a `.luab` struct or a
        // LuaCATS `---@class`.
        if !self.scope.has_struct(&tag.struct_name) && !self.env.has_class(&tag.struct_name) {
            self.error(
                UNDECLARED_NAME,
                tag_range,
                format!(
                    "`---@impl` names an undeclared struct `{}`",
                    tag.struct_name
                ),
                "declare it in a shape module or as a `---@class`".to_string(),
            );
            return;
        }

        // The carrier: the table the tag (or its first method) targets.
        let carrier = target
            .and_then(|span| stmt_at(root, (span.start, span.end)))
            .and_then(|stmt| match stmt {
                Stmt::Local(local) => local
                    .names()
                    .next()
                    .and_then(|n| n.name())
                    .map(|t| t.text().to_string()),
                Stmt::FunctionDecl(func) => func
                    .name()
                    .and_then(|n| n.segments().next())
                    .map(|t| t.text().to_string()),
                _ => None,
            })
            .unwrap_or_else(|| tag.struct_name.clone());

        let methods = self.carrier_methods(&carrier, &tag.struct_name);

        // Completeness (LB2003) — one diagnostic listing every gap.
        let missing: Vec<&str> = trait_shape
            .fns
            .keys()
            .filter(|name| !methods.contains_key(*name))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let listed = missing
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ");
            self.diags.push(
                Diagnostic::error(
                    Code::new(INCOMPLETE_IMPL),
                    format!(
                        "incomplete `---@impl {} for {}`: missing {listed}",
                        tag.trait_name, tag.struct_name
                    ),
                )
                .with_label(Label::primary(
                    Span::new(self.file, tag_range.clone()),
                    format!("the trait requires {listed}"),
                ))
                .with_label(Label::secondary(
                    Span::new(trait_shape.file.clone(), trait_shape.range.clone()),
                    format!("trait `{}` declared here", tag.trait_name),
                )),
            );
        }

        // Signature compatibility (LB2004).
        let self_subst: BTreeMap<String, Ty> =
            [("Self".to_string(), Ty::Named(tag.struct_name.clone()))].into();
        for (name, trait_fn) in &trait_shape.fns {
            let Some(method) = methods.get(name) else {
                continue;
            };
            let impl_range = method.range.clone().unwrap_or_else(|| tag_range.clone());
            let mismatch = |this: &mut Self, detail: String| {
                this.diags.push(
                    Diagnostic::error(
                        Code::new(SIGNATURE_MISMATCH),
                        format!(
                            "`{name}` does not match the signature declared by trait `{}`: \
                             {detail}",
                            tag.trait_name
                        ),
                    )
                    .with_label(Label::primary(
                        Span::new(this.file, impl_range.clone()),
                        "implementation here".to_string(),
                    ))
                    .with_label(Label::secondary(
                        Span::new(trait_fn.file.clone(), trait_fn.range.clone()),
                        format!("trait `{}` declares `{name}` here", tag.trait_name),
                    )),
                );
            };

            // Receiver: `:` vs `.` must match `self`.
            let impl_has_self = method.colon || method.explicit_self;
            if trait_fn.has_self && !impl_has_self {
                mismatch(
                    self,
                    "the trait function takes `self` — declare it with `:` method syntax"
                        .to_string(),
                );
                continue;
            }
            if !trait_fn.has_self && impl_has_self {
                mismatch(
                    self,
                    "the trait function takes no `self` — declare it with `.` syntax".to_string(),
                );
                continue;
            }

            // Arity of the non-`self` parameters (a vararg lifts the cap).
            if method.params.len() != trait_fn.params.len() && !method.has_vararg {
                mismatch(
                    self,
                    format!(
                        "expected {} parameter(s), found {}",
                        trait_fn.params.len(),
                        method.params.len()
                    ),
                );
                continue;
            }

            // Parameters are contravariant: anything the trait promises
            // callers may pass must be accepted by the implementation.
            let mut bad = false;
            for (i, trait_param) in trait_fn.params.iter().enumerate() {
                let Some(impl_ty) = method.params.get(i) else {
                    break;
                };
                let trait_ty = subst_ty(&trait_param.ty, &self_subst);
                if !assignable(self.env, false, &trait_ty, impl_ty) {
                    mismatch(
                        self,
                        format!(
                            "parameter `{}`: the trait passes `{trait_ty}`, the implementation \
                             expects `{impl_ty}`",
                            trait_param.name
                        ),
                    );
                    bad = true;
                    break;
                }
            }
            if bad {
                continue;
            }

            // Returns are covariant, checked only when the implementation
            // declares them (`---@return`).
            if let Some(impl_returns) = &method.returns {
                if impl_returns.len() != trait_fn.returns.len() {
                    mismatch(
                        self,
                        format!(
                            "expected {} return value(s), found {}",
                            trait_fn.returns.len(),
                            impl_returns.len()
                        ),
                    );
                    continue;
                }
                for (impl_ret, trait_ret) in impl_returns.iter().zip(&trait_fn.returns) {
                    let trait_ret = subst_ty(trait_ret, &self_subst);
                    if !assignable(self.env, false, impl_ret, &trait_ret) {
                        mismatch(
                            self,
                            format!("expected return `{trait_ret}`, found `{impl_ret}`"),
                        );
                        break;
                    }
                }
            }
        }

        // Supertraits (LB2008): conformance on the same carrier.
        for supertrait in &trait_shape.supertraits {
            let conformed = self
                .lua_impls
                .contains(&(supertrait.clone(), tag.struct_name.clone()))
                || self.scope.lb_impl(supertrait, &tag.struct_name);
            if !conformed {
                self.diags.push(
                    Diagnostic::error(
                        Code::new(2008),
                        format!(
                            "`---@impl {} for {}` requires supertrait `{supertrait}` conformance \
                             on the same carrier",
                            tag.trait_name, tag.struct_name
                        ),
                    )
                    .with_label(Label::primary(
                        Span::new(self.file, tag_range.clone()),
                        format!("`{}` declares `: {supertrait}`", tag.trait_name),
                    ))
                    .with_label(Label::secondary(
                        Span::new(trait_shape.file.clone(), trait_shape.range.clone()),
                        "supertrait declared here".to_string(),
                    ))
                    .with_note(format!(
                        "add `---@impl {supertrait} for {}` to the same carrier",
                        tag.struct_name
                    )),
                );
            }
        }
    }

    /// The methods available on a carrier: declarations in this file plus,
    /// for LuaCATS interop, function-typed fields of the `---@class`.
    fn carrier_methods(&self, carrier: &str, struct_name: &str) -> BTreeMap<String, MethodInfo> {
        let mut methods = self.methods.get(carrier).cloned().unwrap_or_default();
        if let Some(class) = self.env.class_shape(struct_name) {
            for (name, field) in &class.fields {
                if methods.contains_key(name) {
                    continue;
                }
                if let Ty::Function(func) = &field.ty {
                    methods.insert(name.clone(), method_info_from_fn_ty(func));
                }
            }
        }
        methods
    }
}

/// Method info from a bare parameter list (no annotations).
fn method_info_from_params(list: Option<lua::ast::ParamList>, colon: bool) -> MethodInfo {
    let names = ast_param_names(list);
    let explicit_self = !colon && names.first().is_some_and(|n| n == "self");
    let has_vararg = names.iter().any(|n| n == "...");
    let params = names
        .iter()
        .filter(|n| *n != "..." && !(explicit_self && *n == "self"))
        .map(|_| Ty::Unknown)
        .collect();
    MethodInfo {
        colon,
        explicit_self,
        params,
        has_vararg,
        returns: None,
        range: None,
    }
}

/// Interop: a `---@field m fun(self: X, ...)` class member counts as a
/// method; a leading `self` parameter marks the receiver.
fn method_info_from_fn_ty(func: &FunctionTy) -> MethodInfo {
    let explicit_self = func.params.first().is_some_and(|p| p.name == "self");
    let params = func
        .params
        .iter()
        .skip(usize::from(explicit_self))
        .map(|p| p.ty.clone())
        .collect();
    MethodInfo {
        colon: false,
        explicit_self,
        params,
        has_vararg: func.varargs.is_some(),
        returns: func.has_return_annotation.then(|| func.returns.clone()),
        range: None,
    }
}

// --- helpers ---------------------------------------------------------------

fn node_range(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

fn ast_param_names(list: Option<lua::ast::ParamList>) -> Vec<String> {
    list.map(|l| {
        l.params()
            .map(|p| {
                if p.is_vararg() {
                    "...".to_string()
                } else {
                    p.name().map(|t| t.text().to_string()).unwrap_or_default()
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The innermost statement whose range is exactly `target`.
fn stmt_at(root: &SyntaxNode, target: (usize, usize)) -> Option<Stmt> {
    root.descendants()
        .filter(|node| {
            let range = node.text_range();
            (usize::from(range.start()), usize::from(range.end())) == target
        })
        .find_map(Stmt::cast)
}

/// For a `setmetatable(x, Carrier)` call: the carrier name.
fn setmetatable_carrier(call: &lua::ast::CallExpr) -> Option<String> {
    let Expr::Name(callee) = call.callee()? else {
        return None;
    };
    if callee.name()?.text() != "setmetatable" {
        return None;
    }
    let args = call.args()?.expr_list()?;
    let second = args.exprs().nth(1)?;
    let Expr::Name(name) = second else {
        return None;
    };
    Some(name.name()?.text().to_string())
}

fn first_arg(call: &lua::ast::CallExpr) -> Option<Expr> {
    call.args()?.expr_list()?.exprs().next()
}

/// Unwrap `T?`/`T|nil` down to `T` (single non-nil member).
fn unwrap_optional(ty: &Ty) -> &Ty {
    if let Ty::Union(members) = ty {
        let non_nil: Vec<&Ty> = members.iter().filter(|m| **m != Ty::Nil).collect();
        if let [single] = non_nil[..] {
            return single;
        }
    }
    ty
}

/// The type of a simple value expression: literals, shallow table shapes,
/// function literals. Everything else is `unknown`.
fn simple_expr_ty(expr: &Expr) -> Ty {
    match expr {
        Expr::Literal(_) => env::literal_ty(expr).unwrap_or(Ty::Unknown),
        Expr::Function(_) => Ty::Function(Box::new(FunctionTy::opaque())),
        Expr::Paren(paren) => paren.inner().map_or(Ty::Unknown, |i| simple_expr_ty(&i)),
        Expr::Table(table) => {
            let mut shape = TableTy::default();
            for field in table.fields() {
                if let lua::ast::TableField::Name(f) = field
                    && let Some(name) = f.name()
                {
                    let ty = f.value().map_or(Ty::Unknown, |v| simple_expr_ty(&v));
                    shape.fields.insert(
                        name.text().to_string(),
                        crate::ty::FieldTy {
                            ty,
                            optional: false,
                        },
                    );
                }
            }
            Ty::Table(Box::new(shape))
        }
        _ => Ty::Unknown,
    }
}
