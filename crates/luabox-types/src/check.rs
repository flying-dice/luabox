//! The annotation-driven checker (P0 subset of SPEC.md §3).
//!
//! One walk over the typed AST with a lexical scope stack. Checked, per the
//! ticket:
//!
//! a. **Calls** to annotated functions: arity (respecting optional
//!    parameters and varargs) and argument types. Argument types come from
//!    literals, table constructors (checked *field-by-field* against the
//!    parameter's structural shape), `---@type` locals, references to
//!    annotated functions, and calls to annotated functions (their first
//!    return; a call in last position expands to all returns). A `:` method
//!    call whose receiver resolves (through inference) to a declared
//!    `---@class` is checked against the method's signature the same way —
//!    its *explicit* arguments (minus the implicit `self`) against the
//!    method's parameters.
//! b. **Assignments** to `---@type` locals.
//! c. **Returns** inside functions carrying `---@return`: count + types.
//!
//! Everything unannotated is `unknown` — permissive in warn mode, an error
//! source in strict mode (SPEC.md §3). No inference beyond literals:
//! TODO(P1) bidirectional inference, narrowing, metatables.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use luabox_diag::{Code, Diagnostic, Label, Severity, Span};
use luabox_syntax::lua::ast::{
    ArgList, AstNode, Block, CallExpr, Expr, MethodCallExpr, ParamList, ReturnStmt, SourceFile,
    Stmt, TableExpr,
};
use luabox_syntax::lua::{self, SyntaxNode};

use crate::assign::{LiteralConformance, assignable, classify_literal, is_integral_literal};
use crate::env::{self, TypeEnv};
use crate::ty::{FieldTy, FunctionTy, TableTy, Ty};

/// Diagnostic codes emitted here (block `LB03xx` — Semantics).
const TYPE_MISMATCH: u16 = 300;
const WRONG_ARG_COUNT: u16 = 301;
const MISSING_FIELD: u16 = 302;
const UNKNOWN_FIELD: u16 = 303;
const RETURN_MISMATCH: u16 = 304;
const UNKNOWN_TYPE_NAME: u16 = 305;
/// Wrong number of generic type arguments (`Name<A, B>` vs its params, #117).
const GENERIC_ARITY: u16 = 313;
/// A self- or mutually-referential `---@alias` cycle (luals parity, #123).
const CYCLIC_ALIAS: u16 = 314;
/// Use of a `---@deprecated` symbol (luals `deprecated`, #111).
const DEPRECATED: u16 = 308;
/// Discarded return of a `---@nodiscard` call (luals `discard-returns`, #112).
const DISCARD_RETURNS: u16 = 309;

/// Run the checker over one parsed file. `inferred` carries the
/// inference engine's expression types keyed by byte range — consulted
/// where annotations are absent (annotations always win).
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    parse: &lua::Parse,
    typeenv: &TypeEnv,
    file: &str,
    strict: bool,
    is_meta: bool,
    inferred: &HashMap<(usize, usize), Ty>,
    method_sigs: &HashMap<(usize, usize), FunctionTy>,
    carrier_final: &HashMap<(usize, usize), Ty>,
    carrier_class_final: &HashMap<String, Ty>,
) -> Vec<Diagnostic> {
    let severity = if strict {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut checker = Checker {
        env: typeenv,
        file,
        strict,
        severity,
        is_meta,
        inferred,
        method_sigs,
        carrier_final,
        carrier_class_final,
        deferred_carriers: Vec::new(),
        class_obligations: Vec::new(),
        diags: Vec::new(),
        scopes: vec![HashMap::new()],
        ret_stack: Vec::new(),
    };

    for (name, span) in &typeenv.unknown_names {
        checker.report_full(
            UNKNOWN_TYPE_NAME,
            span.start..span.end,
            format!("unknown type name `{name}` in annotation"),
            "not a built-in, `---@class`, `---@alias`, or `---@enum` name".to_string(),
            None,
        );
    }

    for err in &typeenv.arity_errors {
        let (n, params, args) = (&err.name, err.expected, err.got);
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        checker.report_full(
            GENERIC_ARITY,
            err.span.start..err.span.end,
            format!(
                "`{n}` takes {params} type argument{}, but {args} {} supplied",
                plural(params),
                if args == 1 { "was" } else { "were" },
            ),
            format!("expected {params} type argument{}", plural(params)),
            None,
        );
    }

    // A cyclic alias may be hit from many reference sites (each expansion
    // that walks back onto it pushes its own entry, `lower.rs`'s
    // `expand_alias`) — dedup by name so exactly one LB0314 survives per
    // cyclic alias, never one per reference (#123).
    let mut seen_cyclic_aliases: HashSet<&str> = HashSet::new();
    for err in &typeenv.cyclic_aliases {
        if !seen_cyclic_aliases.insert(err.name.as_str()) {
            continue;
        }
        checker.report_full(
            CYCLIC_ALIAS,
            err.span.start..err.span.end,
            format!("`{}` is a cyclic `---@alias`", err.name),
            "refers to itself, directly or through other aliases".to_string(),
            Some(
                "a cyclic alias resolves to `unknown` at the recursive edge; break the cycle so the alias terminates in a concrete type".to_string(),
            ),
        );
    }

    let tree: SourceFile = parse.tree();
    if let Some(block) = tree.block() {
        checker.visit_block(&block);
    }
    checker.check_deferred_carriers();
    checker.check_class_conformance();

    let mut diags = checker.diags;
    diags.sort_by_key(|d| d.primary_label().map_or(0, |l| l.span.range.start));
    diags
}

/// A scope binding. Only `---@type` bindings are *checked* on assignment
/// (rule b); other bindings (annotated functions) exist so references and
/// calls resolve.
#[derive(Clone)]
struct Binding {
    ty: Ty,
    checked: bool,
}

/// One supplied value in a call-argument or return list: a real expression,
/// or a pseudo-slot from expanding a multi-return call in last position.
enum Slot {
    Expr(Expr),
    Ty(Ty, Range<usize>),
}

/// A `---@type T` carrier whose whole-carrier conformance was deferred: its
/// immediate initializer literal was missing members only, so the check runs
/// at end-of-walk against the final accumulated shape.
struct DeferredCarrier {
    /// The declared object/shape type the carrier must satisfy.
    target: Ty,
    /// The `local` statement's byte range — the key into `carrier_final`.
    decl_key: (usize, usize),
    /// The `---@type` annotation span the diagnostic is attributed to.
    span: Range<usize>,
}

/// A `---@class Name : Parent` carrier whose `: Interface` conformance is
/// verified at end-of-walk against its final accumulated shape (#107). The
/// obligation is the merged shape of its parent classes; the diagnostic is
/// attributed to the `---@class` tag.
struct ClassObligation {
    /// The declared class name bound to the carrier statement.
    name: String,
    /// The carrier `local` statement's byte range — the key into
    /// `carrier_final`.
    decl_key: (usize, usize),
    /// The `---@class` tag span the diagnostic is attributed to.
    span: Range<usize>,
}

struct Checker<'a> {
    env: &'a TypeEnv,
    file: &'a str,
    strict: bool,
    severity: Severity,
    /// This file is a `---@meta` definition package: its `---@class`
    /// declarations are contracts, not carriers, so no conformance
    /// obligation runs inside it (#107).
    is_meta: bool,
    /// Inference results by byte range (rich table inference, SPEC.md §3):
    /// the fallback for expressions annotations cannot type.
    inferred: &'a HashMap<(usize, usize), Ty>,
    /// Resolved `:` method-call signatures keyed by the method-call
    /// expression's byte range — inference's method resolution (#118). Present
    /// only when the receiver resolved to a declared `---@class` and the method
    /// is an annotated function; the checker argument-checks the call against
    /// it and reports nothing when it is absent (conservatism).
    method_sigs: &'a HashMap<(usize, usize), FunctionTy>,
    /// Final accumulated shape of each `---@type` carrier local, keyed by the
    /// `local` statement's byte range (whole-carrier deferral).
    /// `---@class` carriers publish their reified shape here too (#107).
    carrier_final: &'a HashMap<(usize, usize), Ty>,
    /// Reified accumulated shape of every `---@class` carrier keyed by class
    /// name — the parent-carrier fallback for `: Interface` conformance (#107).
    carrier_class_final: &'a HashMap<String, Ty>,
    /// Carriers whose conformance is deferred to `check_deferred_carriers`.
    deferred_carriers: Vec<DeferredCarrier>,
    /// `---@class Name : Parent` obligations, verified by
    /// `check_class_conformance` (#107).
    class_obligations: Vec<ClassObligation>,
    diags: Vec<Diagnostic>,
    scopes: Vec<HashMap<String, Binding>>,
    /// Expected returns of the enclosing function(s); `None` = unannotated.
    ret_stack: Vec<Option<FunctionTy>>,
}

impl Checker<'_> {
    // --- plumbing ------------------------------------------------------

    fn report(&mut self, code: u16, range: Range<usize>, message: String, label: String) {
        self.report_full(code, range, message, label, None);
    }

    fn report_full(
        &mut self,
        code: u16,
        range: Range<usize>,
        message: String,
        label: String,
        note: Option<String>,
    ) {
        let mut diag = Diagnostic::new(Code::new(code), self.severity, message)
            .with_label(Label::primary(Span::new(self.file, range), label));
        if let Some(note) = note {
            diag = diag.with_note(note);
        }
        self.diags.push(diag);
    }

    /// Report at a fixed `Warning` severity, regardless of the strictness
    /// ladder. `---@deprecated` and `---@nodiscard` findings are advisory:
    /// luals keeps them `Warning` in every mode (they never escalate to an
    /// error the way a real type mismatch does), so luabox mirrors that.
    fn report_warning(&mut self, code: u16, range: Range<usize>, message: String, label: String) {
        self.diags.push(
            Diagnostic::new(Code::new(code), Severity::Warning, message)
                .with_label(Label::primary(Span::new(self.file, range), label)),
        );
    }

    fn assignable(&self, value: &Ty, target: &Ty) -> bool {
        assignable(self.env, self.strict, value, target)
    }

    fn bind(&mut self, name: &str, ty: Ty, checked: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), Binding { ty, checked });
        }
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    // --- traversal -----------------------------------------------------

    fn visit_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        for stmt in block.stmts() {
            self.visit_stmt(&stmt);
        }
        self.scopes.pop();
    }

    fn visit_opt_block(&mut self, block: Option<Block>) {
        if let Some(block) = block {
            self.visit_block(&block);
        }
    }

    fn visit_opt_expr(&mut self, expr: Option<Expr>) {
        if let Some(expr) = expr {
            self.visit_expr(&expr);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Local(local) => self.visit_local(local),
            Stmt::LocalFunction(func) => {
                let sig = self.env.fn_sig(range_key(func.syntax())).cloned();
                if let Some(name) = func.name() {
                    let bound = sig.clone().unwrap_or_else(FunctionTy::opaque);
                    // Bound before the body so recursive calls resolve.
                    self.bind(name.text(), Ty::Function(Box::new(bound)), false);
                }
                self.visit_function_body(func.param_list(), func.body(), sig);
            }
            Stmt::FunctionDecl(func) => {
                let sig = self.env.fn_sig(range_key(func.syntax())).cloned();
                self.visit_function_body(func.param_list(), func.body(), sig);
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
                    if let Expr::Name(name) = target
                        && let Some(name) = name.name()
                        && let Some(binding) = self.lookup(name.text()).cloned()
                        && binding.checked
                        && let Some(value) = values.get(i)
                    {
                        self.check_slot(&Slot::Expr(value.clone()), &binding.ty, TYPE_MISMATCH);
                    }
                    self.visit_target(target);
                }
                for value in &values {
                    self.visit_expr(value);
                }
            }
            Stmt::Return(ret) => {
                if let Some(list) = ret.exprs() {
                    for expr in list.exprs() {
                        self.visit_expr(&expr);
                    }
                }
                self.check_return(ret);
            }
            Stmt::Call(call) => {
                if let Some(Expr::Call(inner)) = call.expr() {
                    self.check_discard(&inner);
                }
                self.visit_opt_expr(call.expr());
            }
            Stmt::Do(stmt) => self.visit_opt_block(stmt.body()),
            Stmt::While(stmt) => {
                self.visit_opt_expr(stmt.condition());
                self.visit_opt_block(stmt.body());
            }
            Stmt::Repeat(stmt) => {
                self.visit_opt_block(stmt.body());
                self.visit_opt_expr(stmt.condition());
            }
            Stmt::If(stmt) => {
                self.visit_opt_expr(stmt.condition());
                self.visit_opt_block(stmt.then_block());
                for clause in stmt.elseif_clauses() {
                    self.visit_opt_expr(clause.condition());
                    self.visit_opt_block(clause.block());
                }
                if let Some(clause) = stmt.else_clause() {
                    self.visit_opt_block(clause.block());
                }
            }
            Stmt::NumericFor(stmt) => {
                self.visit_opt_expr(stmt.start());
                self.visit_opt_expr(stmt.end());
                self.visit_opt_expr(stmt.step());
                self.visit_opt_block(stmt.body());
            }
            Stmt::GenericFor(stmt) => {
                if let Some(list) = stmt.exprs() {
                    for expr in list.exprs() {
                        self.visit_expr(&expr);
                    }
                }
                self.visit_opt_block(stmt.body());
            }
            Stmt::Break(_) | Stmt::Goto(_) | Stmt::Label(_) => {}
        }
    }

    /// `local a, b = x, y` — rule b's declaration half: values checked
    /// against `---@type`, names bound afterwards (initializers cannot see
    /// the names they define).
    fn visit_local(&mut self, local: &lua::ast::LocalStmt) {
        let key = range_key(local.syntax());
        // A `---@class Name : Parent` bound to this carrier `local` incurs a
        // conformance obligation (#107): defer it to the final accumulated
        // shape. Skipped inside `---@meta` defs (declarations, not carriers)
        // and for parentless classes (nothing to conform to).
        if !self.is_meta
            && let Some(name) = self.env.declared_target(key)
            && self.env.class_parents(name).is_some_and(|p| !p.is_empty())
        {
            let name = name.to_string();
            let span = self
                .env
                .class_tag_span(key)
                .unwrap_or_else(|| range(local.syntax()));
            self.class_obligations.push(ClassObligation {
                name,
                decl_key: key,
                span,
            });
        }
        let types: Option<Vec<Ty>> = self.env.typed_local(key).map(<[Ty]>::to_vec);
        let sig = self.env.fn_sig(key).cloned();
        let names: Vec<String> = local
            .names()
            .filter_map(|n| n.name())
            .map(|t| t.text().to_string())
            .collect();
        let values: Vec<Expr> = local
            .values()
            .map(|v| v.exprs().collect())
            .unwrap_or_default();

        // `---@param`/`---@return`-annotated `local f = function(...)`.
        if let Some(sig) = sig
            && let Some(Expr::Function(func)) = values.first()
        {
            if let Some(name) = names.first() {
                self.bind(name, Ty::Function(Box::new(sig.clone())), false);
            }
            self.visit_function_body(func.param_list(), func.body(), Some(sig));
            return;
        }

        if let Some(types) = &types {
            for (i, expected) in types.iter().enumerate() {
                if let Some(value) = values.get(i) {
                    // A single `---@type T` object annotation on a
                    // table-constructor `local X = {}` whose literal is
                    // missing members *only* is a carrier being built: defer
                    // its whole-carrier conformance to the final accumulated
                    // shape, suppressing the immediate missing
                    // -field error. Mismatched present members and excess keys
                    // (freshness) are *not* deferred — they report now.
                    if i == 0
                        && types.len() == 1
                        && let Expr::Table(table) = value
                        && let Ty::Table(lit) = self.table_literal_ty(table)
                        && classify_literal(self.env, self.strict, &lit, expected)
                            == Some(LiteralConformance::MissingOnly)
                    {
                        let span = self
                            .env
                            .typed_local_span(key)
                            .unwrap_or_else(|| range(local.syntax()));
                        self.deferred_carriers.push(DeferredCarrier {
                            target: expected.clone(),
                            decl_key: key,
                            span,
                        });
                        continue;
                    }
                    self.check_slot(&Slot::Expr(value.clone()), expected, TYPE_MISMATCH);
                }
            }
        }
        for value in &values {
            self.visit_expr(value);
        }
        for (i, name) in names.iter().enumerate() {
            if let Some(ty) = types.as_ref().and_then(|t| t.get(i)).cloned() {
                self.bind(name, ty, true);
            } else {
                // No inference beyond "a function literal is a function".
                let ty = match values.get(i) {
                    Some(Expr::Function(_)) => Ty::Function(Box::new(FunctionTy::opaque())),
                    _ => Ty::Unknown,
                };
                self.bind(name, ty, false);
            }
        }
    }

    /// Run every deferred `---@type` carrier conformance obligation against
    /// the binding's final accumulated shape. The diagnostic is attributed to
    /// the `---@type` annotation and carries the same member-naming detail as
    /// an immediate mismatch. A carrier whose accumulated shape satisfies the
    /// type produces nothing.
    fn check_deferred_carriers(&mut self) {
        for carrier in std::mem::take(&mut self.deferred_carriers) {
            let Some(found) = self.carrier_final.get(&carrier.decl_key).cloned() else {
                continue; // inference published no final shape (e.g. off)
            };
            if self.assignable(&found, &carrier.target) {
                continue;
            }
            let detail =
                crate::assign::explain_mismatch(self.env, self.strict, &found, &carrier.target)
                    .map_or(String::new(), |d| format!(": {d}"));
            self.report_full(
                TYPE_MISMATCH,
                carrier.span.clone(),
                format!(
                    "type mismatch: expected `{}`, found `{found}`{detail}",
                    carrier.target
                ),
                format!("expected `{}`", carrier.target),
                None,
            );
        }
    }

    /// Verify each `---@class Name : Parent` carrier against the interface(s)
    /// it declares it extends (#107) — the strictness luals declares but does
    /// not check.
    ///
    /// The obligation is every member the parent chain declares (the merged
    /// [`TypeEnv::class_shape`] of each parent), *excluding* members `Name`
    /// re-declares as its own `---@field` (those are governed by `Name`'s own
    /// declaration). Each obliged member must be satisfied by the carrier's
    /// FINAL accumulated shape. The rule, precisely — a member is satisfied
    /// when it is:
    ///
    /// - **(a) provided by the carrier** — `function X:m()` / `X.f = ...`,
    ///   *plus* anything inherited through a `setmetatable(X, { __index =
    ///   Base })` chain, which [`infer::reify_shape`] already folds into the
    ///   reified `carrier_final` shape. A provided member's type is checked
    ///   against the parent's declaration via [`Checker::assignable`]
    ///   (function subtyping absorbs `self`/receiver looseness); a mismatch is
    ///   reported.
    /// - **(b) inherited from a parent carrier** defined in this file and
    ///   reachable through the class's parent chain — the fallback that
    ///   covers the `X.__index = Base` idiom, whose delegation the carrier's
    ///   own reified shape does not fold in. The inherited implementation is
    ///   the base's concern, so its signature is not re-checked here.
    /// - **(c) optional / nil-admitting in the parent** — no obligation.
    ///
    /// Only a member satisfied by none of these is reported missing. This is
    /// what keeps classic inheritance from being wrongly flagged: a subclass
    /// that inherits a concrete base method (idiom (a) or (b)) is silent.
    fn check_class_conformance(&mut self) {
        for ob in std::mem::take(&mut self.class_obligations) {
            let Some(Ty::Table(provided)) = self.carrier_final.get(&ob.decl_key).cloned() else {
                continue; // inference published no final shape (e.g. off)
            };
            let Some(parents) = self.env.class_parents(&ob.name) else {
                continue;
            };
            let parents: Vec<String> = parents.to_vec();
            // Members already handled — dedup across a diamond of parents.
            let mut seen: HashSet<String> = HashSet::new();
            for parent in &parents {
                let Some(pshape) = self.env.class_shape(parent) else {
                    continue;
                };
                for (member, field) in &pshape.fields {
                    if !seen.insert(member.clone()) {
                        continue;
                    }
                    // A member `Name` re-declares is its own declaration's
                    // responsibility, not an inherited obligation.
                    if self.env.class_declares_own(&ob.name, member) {
                        continue;
                    }
                    // Optional / nil-admitting members impose no obligation.
                    if field.optional || field.ty.admits_nil() {
                        continue;
                    }
                    self.check_class_member(&ob, parent, member, field, &provided);
                }
            }
        }
    }

    /// Check one obliged member against the carrier's final shape. See
    /// [`Checker::check_class_conformance`] for the rule.
    fn check_class_member(
        &mut self,
        ob: &ClassObligation,
        parent: &str,
        member: &str,
        field: &FieldTy,
        provided: &TableTy,
    ) {
        // (a) provided on the carrier (own members + `setmetatable` chain).
        if let Some(actual) = provided.fields.get(member) {
            let expected = if field.optional {
                field.ty.clone().optional()
            } else {
                field.ty.clone()
            };
            if !self.assignable(&actual.ty, &expected) {
                let detail =
                    crate::assign::explain_mismatch(self.env, self.strict, &actual.ty, &expected)
                        .map_or(String::new(), |d| format!(": {d}"));
                self.report_class_conformance(
                    ob.span.clone(),
                    format!(
                        "`{}` does not satisfy `{parent}`: member `{member}` has the wrong type",
                        ob.name
                    ),
                    format!("expected `{expected}`, found `{}`{detail}", actual.ty),
                    parent,
                );
            }
            return;
        }
        // (b) inherited from a parent carrier in this file (the `X.__index =
        //     Base` chain the reified carrier shape does not fold in).
        if self.member_on_parent_carrier(&ob.name, member) {
            return;
        }
        // (c) missing entirely.
        self.report_class_conformance(
            ob.span.clone(),
            format!(
                "`{}` does not satisfy `{parent}`: missing member `{member}`",
                ob.name
            ),
            format!("expected member `{member}` of type `{}`", field.ty),
            parent,
        );
    }

    /// Whether `member` is defined on any parent carrier of `class` in this
    /// file, walking the parent chain transitively. The parent-carrier
    /// fallback of [`Checker::check_class_conformance`].
    fn member_on_parent_carrier(&self, class: &str, member: &str) -> bool {
        let mut stack: Vec<String> = self
            .env
            .class_parents(class)
            .map(<[String]>::to_vec)
            .unwrap_or_default();
        let mut seen: HashSet<String> = HashSet::new();
        while let Some(parent) = stack.pop() {
            if !seen.insert(parent.clone()) {
                continue;
            }
            if let Some(Ty::Table(shape)) = self.carrier_class_final.get(&parent)
                && shape.fields.contains_key(member)
            {
                return true;
            }
            if let Some(grandparents) = self.env.class_parents(&parent) {
                stack.extend(grandparents.iter().cloned());
            }
        }
        false
    }

    /// Report an LB0300 `: Interface` conformance failure at the `---@class`
    /// tag, with a "declared here" secondary label at the parent's in-file
    /// declaration when it has one (ambient/defs parents have none) (#107).
    fn report_class_conformance(
        &mut self,
        span: Range<usize>,
        message: String,
        label: String,
        parent: &str,
    ) {
        let mut diag = Diagnostic::new(Code::new(TYPE_MISMATCH), self.severity, message)
            .with_label(Label::primary(Span::new(self.file, span), label));
        if let Some(range) = self.env.class_decl_span(parent) {
            diag = diag.with_label(Label::secondary(
                Span::new(self.file.to_string(), range),
                format!("`{parent}` declared here"),
            ));
        }
        self.diags.push(diag);
    }

    /// Enter a function body: parameters bound to their annotated types,
    /// expected returns pushed for `return` checking.
    fn visit_function_body(
        &mut self,
        params: Option<ParamList>,
        body: Option<Block>,
        sig: Option<FunctionTy>,
    ) {
        self.scopes.push(HashMap::new());
        if let Some(list) = params {
            for param in list.params() {
                if let Some(name) = param.name() {
                    let ty = sig
                        .as_ref()
                        .and_then(|s| s.params.iter().find(|p| p.name == name.text()))
                        .map_or(Ty::Unknown, |p| {
                            if p.optional {
                                p.ty.clone().optional()
                            } else {
                                p.ty.clone()
                            }
                        });
                    self.bind(name.text(), ty, false);
                }
            }
        }
        self.ret_stack.push(sig);
        self.visit_opt_block(body);
        self.ret_stack.pop();
        self.scopes.pop();
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(call) => {
                self.check_call(call);
                self.visit_opt_expr(call.callee());
                self.visit_arg_exprs(call.args());
            }
            Expr::MethodCall(call) => {
                self.check_method_call(call);
                self.visit_opt_expr(call.receiver());
                self.visit_arg_exprs(call.args());
            }
            Expr::Paren(paren) => self.visit_opt_expr(paren.inner()),
            Expr::Prefix(prefix) => self.visit_opt_expr(prefix.operand()),
            Expr::Bin(bin) => {
                self.visit_opt_expr(bin.lhs());
                self.visit_opt_expr(bin.rhs());
            }
            Expr::Function(func) => {
                self.visit_function_body(func.param_list(), func.body(), None);
            }
            Expr::Table(table) => {
                for field in table.fields() {
                    match field {
                        lua::ast::TableField::Key(f) => {
                            self.visit_opt_expr(f.key());
                            self.visit_opt_expr(f.value());
                        }
                        lua::ast::TableField::Name(f) => self.visit_opt_expr(f.value()),
                        lua::ast::TableField::Item(f) => self.visit_opt_expr(f.value()),
                    }
                }
            }
            Expr::Index(index) => {
                self.visit_opt_expr(index.base());
                self.visit_opt_expr(index.index());
            }
            Expr::Field(field) => {
                self.note_deprecated_read(expr);
                self.visit_opt_expr(field.base());
            }
            Expr::Name(_) => self.note_deprecated_read(expr),
            Expr::Literal(_) | Expr::Vararg(_) => {}
        }
    }

    /// Visit an assignment *target* (LHS): its base/index sub-expressions are
    /// reads, but the target name/field itself is a write and never triggers a
    /// `---@deprecated` use finding. Mirrors the read traversal of
    /// [`Checker::visit_expr`] minus the outer deprecation check.
    fn visit_target(&mut self, target: &Expr) {
        match target {
            Expr::Name(_) => {}
            Expr::Field(field) => self.visit_opt_expr(field.base()),
            Expr::Index(index) => {
                self.visit_opt_expr(index.base());
                self.visit_opt_expr(index.index());
            }
            other => self.visit_expr(other),
        }
    }

    /// Report `LB0308` when `expr` reads a `---@deprecated` function — a bare
    /// name bound to (or globally naming) a deprecated function, or a dotted
    /// `M.f` naming one (luals `deprecated`, #111). Call sites are covered
    /// automatically: a call visits its callee through this path, so `f()`,
    /// `M.f()`, and a plain value reference `local g = f` all report exactly
    /// once. Write targets go through [`Checker::visit_target`] instead and are
    /// never flagged (the declaration site is not a use).
    fn note_deprecated_read(&mut self, expr: &Expr) {
        let (deprecated, name) = match expr {
            Expr::Name(n) => {
                let Some(ident) = n.name() else { return };
                let text = ident.text();
                let dep = match self.lookup(text) {
                    Some(Binding {
                        ty: Ty::Function(f),
                        ..
                    }) => f.deprecated,
                    // A shadowing non-function local means the name no longer
                    // refers to the deprecated function.
                    Some(_) => false,
                    None => self.env.function(text).is_some_and(|f| f.deprecated),
                };
                (dep, text.to_string())
            }
            Expr::Field(_) => {
                let Some(dotted) = dotted_name(expr) else {
                    return;
                };
                // A dotted callable declared in this file's env (`function M.f`,
                // an ambient defs function) is the direct hit; otherwise fall
                // back to the *resolved field type* so a deprecated function
                // reached as a field of a typed receiver — e.g. a `require`d
                // module's member, or a class field typed `fun(...)` — is still
                // flagged. The field path only ever reports when the resolved
                // type is a deprecated function, so it never false-positives.
                let dep = self.env.function(&dotted).is_some_and(|f| f.deprecated)
                    || matches!(self.expr_ty(expr), Ty::Function(f) if f.deprecated);
                (dep, dotted)
            }
            _ => return,
        };
        if deprecated {
            self.report_warning(
                DEPRECATED,
                range(expr.syntax()),
                format!("use of deprecated `{name}`"),
                format!("`{name}` is marked `---@deprecated`"),
            );
        }
    }

    /// Report `LB0309` when a bare call statement discards the return of a
    /// `---@nodiscard` function (luals `discard-returns`, #112). Only a
    /// statement-position call is a discard — a call bound to a local, used in
    /// a larger expression, or passed as an argument keeps its value and is
    /// accepted, matching luals.
    fn check_discard(&mut self, call: &CallExpr) {
        let Some(callee) = call.callee() else {
            return;
        };
        // Env-registered callables (`function M.f` in this file, ambient defs
        // functions) resolve through `callee_sig`; otherwise fall back to the
        // *resolved type* of the callee expression, so a nodiscard function
        // reached as a field of a typed receiver — e.g. a `require`d module's
        // member — is still flagged. Same two-step lookup as the `deprecated`
        // read check ([`Checker::note_deprecated_read`]): the sibling flag
        // rides the identical `FunctionTy`, so both must consult both paths.
        let nodiscard = self.callee_sig(&callee).is_some_and(|sig| sig.nodiscard)
            || matches!(self.expr_ty(&callee), Ty::Function(f) if f.nodiscard);
        if !nodiscard {
            return;
        }
        let name = call
            .callee()
            .and_then(|c| dotted_name(&c))
            .map_or_else(|| "function".to_string(), |n| format!("`{n}`"));
        self.report_warning(
            DISCARD_RETURNS,
            range(call.syntax()),
            format!("return value of {name} is discarded"),
            "this call is annotated `---@nodiscard`; its result must be used".to_string(),
        );
    }

    fn visit_arg_exprs(&mut self, args: Option<lua::ast::ArgList>) {
        let Some(args) = args else { return };
        if let Some(list) = args.expr_list() {
            for expr in list.exprs() {
                self.visit_expr(&expr);
            }
        } else if let Some(table) = args.table_arg() {
            self.visit_expr(&Expr::Table(table));
        }
    }

    // --- expression types ---------------------------------------------

    /// The (first-value) type of an expression: annotation-derived first,
    /// falling back to the inference engine's published type (rich table
    /// inference, SPEC.md §3) when annotations say nothing.
    fn expr_ty(&self, expr: &Expr) -> Ty {
        // An inline `--[[@as T]]` cast is authoritative for the expression
        // it directly follows.
        if let Some(ty) = self.env.as_cast_at(range(expr.syntax()).end) {
            return ty.clone();
        }
        // Name references prefer the flow-sensitive inferred type: for
        // annotated bindings it *starts* from the annotation
        // (authoritative) and only ever refines it (narrowing), and for
        // unannotated bindings it is the only source of information.
        if matches!(expr, Expr::Name(_))
            && let Some(ty) = self.inferred.get(&range_key(expr.syntax()))
        {
            return ty.clone();
        }
        let ty = self.annotated_expr_ty(expr);
        if matches!(ty, Ty::Unknown)
            && let Some(inferred) = self.inferred.get(&range_key(expr.syntax()))
        {
            return inferred.clone();
        }
        ty
    }

    /// The annotation/literal-derived type of an expression (the P0
    /// subset). Unknowable stays `unknown`.
    fn annotated_expr_ty(&self, expr: &Expr) -> Ty {
        match expr {
            Expr::Literal(_) => env::literal_ty(expr).unwrap_or(Ty::Unknown),
            Expr::Name(name) => name.name().map_or(Ty::Unknown, |t| self.name_ty(t.text())),
            Expr::Paren(paren) => paren
                .inner()
                .map_or(Ty::Unknown, |inner| self.expr_ty(&inner)),
            Expr::Table(table) => self.table_literal_ty(table),
            Expr::Function(_) => Ty::Function(Box::new(FunctionTy::opaque())),
            Expr::Call(call) => {
                // `setmetatable(t, Carrier)` is modeled by inference — the
                // result is the carrier's *instance* — so the ambient
                // `---@return table` signature must not mask it: a
                // constructor returning `setmetatable(...)` then satisfies
                // `---@return <Class>` (#73).
                if self.is_global_setmetatable(call) {
                    return Ty::Unknown;
                }
                let sig = call.callee().and_then(|c| self.callee_sig(&c));
                match sig {
                    Some(sig) if sig.has_return_annotation || !sig.overloads.is_empty() => {
                        // When the primary does not accept the arguments but an
                        // `---@overload` does, the call's result type is that
                        // overload's return (first match wins, luals-style, #86).
                        let mut sig = self.resolve_call_overload(sig, call);
                        if !sig.has_return_annotation {
                            return Ty::Unknown;
                        }
                        // Monomorphise a generic call so its result reflects
                        // the inferred type arguments (#84).
                        if !sig.generics.is_empty() {
                            let arg_tys = self.call_arg_tys(call);
                            let map = crate::generics::infer_call(&sig, &arg_tys);
                            sig = crate::generics::subst_function(&sig, &map);
                        }
                        sig.returns.first().cloned().unwrap_or(Ty::Nil)
                    }
                    _ => Ty::Unknown,
                }
            }
            Expr::Field(field) => self.field_ty(field),
            _ => Ty::Unknown,
        }
    }

    fn name_ty(&self, name: &str) -> Ty {
        if let Some(binding) = self.lookup(name) {
            return binding.ty.clone();
        }
        if let Some(func) = self.env.function(name) {
            return Ty::Function(Box::new(func.clone()));
        }
        // Ambient module tables / scalar globals (`math`, `_VERSION`, ...).
        if let Some(ty) = self.env.global_type(name) {
            return ty.clone();
        }
        Ty::Unknown
    }

    /// `Base.field`: an enum member's literal type, an annotated function
    /// reached by its dotted name, or a field of a value whose type
    /// resolves to a table shape.
    fn field_ty(&self, field: &lua::ast::FieldExpr) -> Ty {
        if let (Some(Expr::Name(base)), Some(member)) = (field.base(), field.field_name())
            && let Some(base) = base.name()
            && let Some(ty) = self.env.enum_member(base.text(), member.text())
        {
            return ty.clone();
        }
        if let Some(dotted) = dotted_name(&Expr::Field(field.clone()))
            && let Some(func) = self.env.function(&dotted)
        {
            return Ty::Function(Box::new(func.clone()));
        }
        // A field of a typed value: look it up in the structural shape.
        if let (Some(base), Some(member)) = (field.base(), field.field_name()) {
            let base_ty = self.expr_ty(&base);
            if let Some((_, shape)) = self.table_shape(&base_ty)
                && let Some(fld) = shape.fields.get(member.text())
            {
                return if fld.optional {
                    fld.ty.clone().optional()
                } else {
                    fld.ty.clone()
                };
            }
        }
        Ty::Unknown
    }

    /// The structural shape of a table constructor, from literals only.
    fn table_literal_ty(&self, table: &TableExpr) -> Ty {
        let mut shape = TableTy::default();
        let mut items: Vec<Ty> = Vec::new();
        for field in table.fields() {
            match field {
                lua::ast::TableField::Name(f) => {
                    if let Some(name) = f.name() {
                        let ty = f.value().map_or(Ty::Unknown, |v| self.expr_ty(&v));
                        shape.fields.insert(
                            name.text().to_string(),
                            FieldTy {
                                ty,
                                optional: false,
                            },
                        );
                    }
                }
                lua::ast::TableField::Key(f) => {
                    let key = f.key().map(|k| self.expr_ty(&k));
                    let value = f.value().map_or(Ty::Unknown, |v| self.expr_ty(&v));
                    match key {
                        Some(Ty::StringLit(name)) => {
                            shape.fields.insert(
                                name,
                                FieldTy {
                                    ty: value,
                                    optional: false,
                                },
                            );
                        }
                        Some(Ty::NumberLit(_)) => items.push(value),
                        _ => {} // dynamic key: contributes nothing checkable
                    }
                }
                lua::ast::TableField::Item(f) => {
                    items.push(f.value().map_or(Ty::Unknown, |v| self.expr_ty(&v)));
                }
            }
        }
        if !items.is_empty() {
            shape.array = Some(Ty::union(items));
        }
        Ty::Table(Box::new(shape))
    }

    /// Whether a call invokes the global `setmetatable` builtin (a local
    /// of the same name shadows it).
    fn is_global_setmetatable(&self, call: &CallExpr) -> bool {
        matches!(
            call.callee(),
            Some(Expr::Name(name))
                if name.name().is_some_and(|t| t.text() == "setmetatable")
        ) && self.lookup("setmetatable").is_none()
    }

    /// The function signature an expression evaluates to, when known.
    fn callee_sig(&self, callee: &Expr) -> Option<FunctionTy> {
        match callee {
            Expr::Name(name) => {
                let name = name.name()?;
                if let Some(Binding {
                    ty: Ty::Function(f),
                    ..
                }) = self.lookup(name.text())
                {
                    return Some((**f).clone());
                }
                self.env.function(name.text()).cloned()
            }
            Expr::Field(_) => self.env.function(&dotted_name(callee)?).cloned(),
            Expr::Paren(paren) => self.callee_sig(&paren.inner()?),
            _ => None,
        }
    }

    /// The type of one supplied call slot (annotation/inference derived).
    fn slot_ty(&self, slot: &Slot) -> Ty {
        match slot {
            Slot::Expr(expr) => self.expr_ty(expr),
            Slot::Ty(ty, _) => ty.clone(),
        }
    }

    /// The positional argument types of a call — for generic inference on a
    /// call used as a value (nested/argument position).
    fn call_arg_tys(&self, call: &CallExpr) -> Vec<Ty> {
        let (slots, string_arg) = call_slots(call);
        let mut tys: Vec<Ty> = slots.iter().map(|s| self.slot_ty(s)).collect();
        if let Some((ty, _)) = string_arg {
            tys.push(ty);
        }
        tys
    }

    /// Verify each `---@generic T : Constraint` bound against T's inferred
    /// binding (luals bounded generics). A violation reports the standard
    /// argument type-mismatch (LB0300) at the first argument that fixes T.
    fn check_generic_constraints(
        &mut self,
        sig: &FunctionTy,
        map: &std::collections::BTreeMap<String, Ty>,
        slots: &[Slot],
    ) {
        for g in &sig.generics {
            let (Some(constraint), Some(bound)) = (&g.constraint, map.get(&g.name)) else {
                continue;
            };
            if self.assignable(bound, constraint) {
                continue;
            }
            let Some(i) = sig.params.iter().position(|p| ty_mentions(&p.ty, &g.name)) else {
                continue;
            };
            let Some(slot) = slots.get(i) else { continue };
            self.report(
                TYPE_MISMATCH,
                slot_range(slot),
                format!(
                    "type mismatch: type argument `{}` = `{bound}` does not satisfy constraint `{constraint}`",
                    g.name
                ),
                format!("expected a value assignable to `{constraint}`"),
            );
        }
    }

    // --- rule a: call sites ---------------------------------------------

    /// Pick the signature whose *returns* govern a call's result type: the
    /// primary when it accepts the supplied arguments, otherwise the first
    /// `---@overload` that does, otherwise the primary unchanged (its
    /// diagnostics fire elsewhere). This is the value-position complement of
    /// [`Checker::check_call`]'s overload acceptance (#86).
    fn resolve_call_overload(&self, sig: FunctionTy, call: &CallExpr) -> FunctionTy {
        if sig.overloads.is_empty() {
            return sig;
        }
        let (mut slots, string_arg) = call_slots(call);
        if let Some((ty, range)) = string_arg {
            slots.push(Slot::Ty(ty, range));
        }
        let open_ended = self.expand_last(&mut slots);
        if self.call_accepts(&sig, &slots, open_ended) {
            return sig;
        }
        for overload in &sig.overloads {
            if self.call_accepts(overload, &slots, open_ended) {
                return overload.clone();
            }
        }
        sig
    }

    /// Whether `sig` accepts this argument list — a non-reporting predicate
    /// used to resolve `---@overload` candidates. Mirrors the arity and
    /// per-slot assignability rules of [`Checker::check_call`] without
    /// emitting diagnostics.
    fn call_accepts(&self, sig: &FunctionTy, slots: &[Slot], open_ended: bool) -> bool {
        let supplied = slots.len();
        if supplied < sig.required_params() && !open_ended {
            return false;
        }
        if supplied > sig.params.len() && sig.varargs.is_none() {
            return false;
        }
        for (i, slot) in slots.iter().enumerate() {
            let expected = if let Some(param) = sig.params.get(i) {
                if param.optional {
                    param.ty.clone().optional()
                } else {
                    param.ty.clone()
                }
            } else if let Some(varargs) = &sig.varargs {
                varargs.clone()
            } else {
                continue;
            };
            let found = match slot {
                Slot::Expr(expr) => self.expr_ty(expr),
                Slot::Ty(ty, _) => ty.clone(),
            };
            if !self.assignable(&found, &expected) {
                return false;
            }
        }
        true
    }

    fn check_call(&mut self, call: &CallExpr) {
        let Some(sig) = call.callee().and_then(|c| self.callee_sig(&c)) else {
            return;
        };
        let (mut slots, string_arg) = call_slots(call);
        if let Some((ty, range)) = string_arg {
            slots.push(Slot::Ty(ty, range));
        }
        let open_ended = self.expand_last(&mut slots);
        self.check_arg_slots(sig, &slots, open_ended, range(call.syntax()));
    }

    /// Check a `:` method call against its resolved method signature. The
    /// inference engine resolves the receiver through class shapes /
    /// `__index` / `self`, and publishes the method's signature keyed by the
    /// call's byte range *only* when the receiver is a declared `---@class`
    /// and the member is an annotated function ([`Checker::method_sigs`]) —
    /// so this is a strict no-op for an unknown/`any`/union receiver, a plain
    /// inferred table with no declared class, an unannotated method, or an
    /// unresolved metatable (SPEC §19 conservatism; no false positives).
    ///
    /// When a signature is present, the call's *explicit* arguments (the
    /// implicit `self` stripped from the signature) are checked exactly like a
    /// dotted/free call through the same [`Checker::check_arg_slots`] path, and
    /// a `---@deprecated` method is flagged at its name span (luals
    /// `deprecated`, LB0308).
    fn check_method_call(&mut self, call: &MethodCallExpr) {
        let Some(sig) = self.method_sigs.get(&range_key(call.syntax())) else {
            return;
        };
        let mut sig = sig.clone();
        // The implicit `self` receiver is never an explicit argument. It only
        // appears in the signature when declared with an explicit
        // `---@param self T` or reified from an unannotated body; drop it so the
        // parameters line up with the supplied arguments.
        if sig.params.first().is_some_and(|p| p.name == "self") {
            sig.params.remove(0);
        }
        if sig.deprecated
            && let Some(method) = call.method_name()
        {
            let r = method.text_range();
            self.report_warning(
                DEPRECATED,
                usize::from(r.start())..usize::from(r.end()),
                format!("use of deprecated `{}`", method.text()),
                format!("`{}` is marked `---@deprecated`", method.text()),
            );
        }
        let (mut slots, string_arg) = arg_slots(call.args());
        if let Some((ty, range)) = string_arg {
            slots.push(Slot::Ty(ty, range));
        }
        let open_ended = self.expand_last(&mut slots);
        self.check_arg_slots(sig, &slots, open_ended, range(call.syntax()));
    }

    /// Arity- and type-check a resolved call: shared by dotted/free calls
    /// ([`Checker::check_call`]) and `:` method calls
    /// ([`Checker::check_method_call`]). `slots` are the already-collected
    /// argument slots (with any trailing multi-value expansion applied);
    /// `call_range` anchors the too-few-arguments diagnostic.
    fn check_arg_slots(
        &mut self,
        mut sig: FunctionTy,
        slots: &[Slot],
        open_ended: bool,
        call_range: Range<usize>,
    ) {
        // A generic function (`---@generic T`): infer the type variables from
        // the argument types, verify any constraints, then monomorphise the
        // signature so arg/return checking runs against the bound types (#84).
        if !sig.generics.is_empty() {
            let arg_tys: Vec<Ty> = slots.iter().map(|s| self.slot_ty(s)).collect();
            let map = crate::generics::infer_call(&sig, &arg_tys);
            self.check_generic_constraints(&sig, &map, slots);
            sig = crate::generics::subst_function(&sig, &map);
        }

        // Overloaded stdlib functions (e.g. `tonumber`, `table.insert`): a
        // call is accepted when it matches the primary signature *or* any
        // `---@overload`. Only when none match do we report against the
        // primary (TODO(P1): pick and report the closest overload).
        if !sig.overloads.is_empty()
            && (self.call_accepts(&sig, slots, open_ended)
                || sig
                    .overloads
                    .iter()
                    .any(|o| self.call_accepts(o, slots, open_ended)))
        {
            return;
        }

        let supplied = slots.len();
        let required = sig.required_params();
        let max = sig.params.len();

        if supplied < required && !open_ended {
            let least = if max > required || sig.varargs.is_some() {
                "at least "
            } else {
                ""
            };
            self.report(
                WRONG_ARG_COUNT,
                call_range.clone(),
                format!(
                    "this function takes {least}{required} argument{} but {supplied} {} supplied",
                    plural(required),
                    was_were(supplied),
                ),
                format!("expected {least}{required} argument{}", plural(required)),
            );
        } else if supplied > max && sig.varargs.is_none() {
            let extra_range = slots.get(max).map_or(call_range, slot_range);
            let most = if required < max { "at most " } else { "" };
            self.report(
                WRONG_ARG_COUNT,
                extra_range,
                format!(
                    "this function takes {most}{max} argument{} but {supplied} {} supplied",
                    plural(max),
                    was_were(supplied),
                ),
                "unexpected extra argument".to_string(),
            );
        }

        for (i, slot) in slots.iter().enumerate() {
            if let Some(param) = sig.params.get(i) {
                let expected = if param.optional {
                    param.ty.clone().optional()
                } else {
                    param.ty.clone()
                };
                self.check_slot(slot, &expected, TYPE_MISMATCH);
            } else if let Some(varargs) = &sig.varargs {
                self.check_slot(slot, &varargs.clone(), TYPE_MISMATCH);
            }
        }
    }

    /// Expand a multi-return call in last position into pseudo-slots.
    /// Returns whether the list is open-ended (unknowable length): a
    /// trailing `...`, method call, or call with unknown returns.
    fn expand_last(&self, slots: &mut Vec<Slot>) -> bool {
        let last = match slots.last() {
            Some(Slot::Expr(expr)) => expr.clone(),
            _ => return false,
        };
        match last {
            Expr::Call(call) => {
                let sig = call.callee().and_then(|c| self.callee_sig(&c));
                match sig {
                    Some(sig) if sig.has_return_annotation => {
                        let r = range(call.syntax());
                        for ret in sig.returns.iter().skip(1) {
                            slots.push(Slot::Ty(ret.clone(), r.clone()));
                        }
                        sig.returns_vararg
                    }
                    _ => true,
                }
            }
            Expr::Vararg(_) | Expr::MethodCall(_) => true,
            _ => false,
        }
    }

    /// Check one supplied value against an expected type. Table literals
    /// checked against a table/class shape get field-level diagnostics;
    /// everything else is a single assignability check reported as
    /// `mismatch_code`.
    fn check_slot(&mut self, slot: &Slot, expected: &Ty, mismatch_code: u16) {
        if let Slot::Expr(Expr::Table(table)) = slot
            && let Some((class, shape)) = self.table_shape(expected)
        {
            self.check_table_literal(table, class.as_deref(), &shape);
            return;
        }
        let found = match slot {
            Slot::Expr(expr) => self.expr_ty(expr),
            Slot::Ty(ty, _) => ty.clone(),
        };
        if !self.assignable(&found, expected) {
            let noun = if mismatch_code == RETURN_MISMATCH {
                "return type mismatch"
            } else {
                "type mismatch"
            };
            // Name the offending members when both sides are table-shaped, so
            // the message carries which member is at fault.
            let detail = crate::assign::explain_mismatch(self.env, self.strict, &found, expected)
                .map_or(String::new(), |d| format!(": {d}"));
            self.report_full(
                mismatch_code,
                slot_range(slot),
                format!("{noun}: expected `{expected}`, found `{found}`{detail}"),
                format!("expected `{expected}`"),
                None,
            );
        }
    }

    /// Resolve an expected type to a checkable table shape (unwrapping a
    /// `T?`/`T|nil` optional around it).
    fn table_shape(&self, expected: &Ty) -> Option<(Option<String>, TableTy)> {
        match expected {
            Ty::Named(name) => self
                .env
                .class_shape(name)
                .map(|shape| (Some(name.clone()), shape)),
            Ty::Table(table) => Some((None, (**table).clone())),
            Ty::Union(members) => {
                let non_nil: Vec<&Ty> = members.iter().filter(|m| **m != Ty::Nil).collect();
                match non_nil[..] {
                    [single] => self.table_shape(single),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Field-level check of a table constructor against a structural shape
    /// (LuaLS behaviour: on a *literal*, missing required fields and fields
    /// the class does not declare are each diagnosed, per field).
    #[allow(clippy::too_many_lines)]
    fn check_table_literal(&mut self, table: &TableExpr, class: Option<&str>, shape: &TableTy) {
        let mut present: HashMap<String, (Option<Expr>, Range<usize>)> = HashMap::new();
        let mut items: Vec<Expr> = Vec::new();
        let mut order: Vec<String> = Vec::new();

        for field in table.fields() {
            match &field {
                lua::ast::TableField::Name(f) => {
                    if let Some(name) = f.name() {
                        order.push(name.text().to_string());
                        present.insert(name.text().to_string(), (f.value(), range(field.syntax())));
                    }
                }
                lua::ast::TableField::Key(f) => {
                    let key = f.key().and_then(|k| env::literal_ty(&k));
                    match key {
                        Some(Ty::StringLit(name)) => {
                            order.push(name.clone());
                            present.insert(name, (f.value(), range(field.syntax())));
                        }
                        Some(Ty::NumberLit(n)) if is_integral_literal(&n) => {
                            if let Some(value) = f.value() {
                                items.push(value);
                            }
                        }
                        _ => {} // dynamic key: not checkable here
                    }
                }
                lua::ast::TableField::Item(f) => {
                    if let Some(value) = f.value() {
                        items.push(value);
                    }
                }
            }
        }

        let declared_by = class.map(|c| format!("declared by `---@class {c}`"));

        // Missing required fields — one diagnostic each, naming the field.
        // Carrier member attachments (`function Class:method` collected from
        // the declaring file) resolve on reads but carry no literal
        // obligation (luals `missing-fields` parity — only `---@field`
        // declarations are required).
        let attached = class
            .map(|c| self.env.class_method_names(c))
            .unwrap_or_default();
        for (name, field) in &shape.fields {
            if !field.optional
                && !field.ty.admits_nil()
                && !present.contains_key(name)
                && !attached.contains(name)
            {
                self.report_full(
                    MISSING_FIELD,
                    range(table.syntax()),
                    format!("missing required field `{name}` in table literal"),
                    format!("expected field `{name}` of type `{}`", field.ty),
                    declared_by.clone(),
                );
            }
        }

        // Present fields: known → type check (recursing into nested
        // literals); matched by an indexer → check against it; otherwise
        // unknown (closed-class literals diagnose extras, LuaLS-style).
        for name in order {
            let Some((value, field_range)) = present.get(&name).cloned() else {
                continue;
            };
            if let Some(field) = shape.fields.get(&name) {
                let expected = if field.optional {
                    field.ty.clone().optional()
                } else {
                    field.ty.clone()
                };
                if let Some(value) = value {
                    self.check_slot(&Slot::Expr(value), &expected, TYPE_MISMATCH);
                }
                continue;
            }
            let key_ty = Ty::StringLit(name.clone());
            let indexer = shape
                .indexers
                .iter()
                .find(|(key, _)| self.assignable(&key_ty, key))
                .map(|(_, value)| value.clone());
            if let Some(value_ty) = indexer {
                if let Some(value) = value {
                    self.check_slot(&Slot::Expr(value), &value_ty, TYPE_MISMATCH);
                }
                continue;
            }
            self.report_full(
                UNKNOWN_FIELD,
                field_range,
                format!("unknown field `{name}` in table literal"),
                class.map_or_else(
                    || "the expected table type declares no such field".to_string(),
                    |c| format!("`{c}` declares no field `{name}` and has no indexer"),
                ),
                declared_by.clone(),
            );
        }

        // A fixed-position tuple target (`[string, number]`, modeled as
        // integer-literal indexers): each positional item is checked against
        // its own position; items past the tuple's end are lenient (#86).
        let positional: Vec<(usize, Ty)> = shape
            .indexers
            .iter()
            .filter_map(|(key, value)| match key {
                Ty::NumberLit(n) if is_integral_literal(n) => {
                    n.parse::<usize>().ok().map(|i| (i, value.clone()))
                }
                _ => None,
            })
            .collect();
        if !positional.is_empty() {
            for (i, item) in items.into_iter().enumerate() {
                if let Some((_, ty)) = positional.iter().find(|(pos, _)| *pos == i + 1) {
                    self.check_slot(&Slot::Expr(item), ty, TYPE_MISMATCH);
                }
            }
            return;
        }

        // Array items against the array part / an integer-keyed indexer.
        // (Items against a shape with neither are left alone — P1.)
        let elem = shape.array.clone().or_else(|| {
            shape
                .indexers
                .iter()
                .find(|(key, _)| self.assignable(&Ty::Integer, key))
                .map(|(_, value)| value.clone())
        });
        if let Some(elem) = elem {
            for item in items {
                self.check_slot(&Slot::Expr(item), &elem, TYPE_MISMATCH);
            }
        }
    }

    // --- rule c: returns -------------------------------------------------

    fn check_return(&mut self, ret: &ReturnStmt) {
        let Some(Some(sig)) = self.ret_stack.last().cloned() else {
            return;
        };
        if !sig.has_return_annotation {
            return;
        }
        let mut slots: Vec<Slot> = ret
            .exprs()
            .map(|list| list.exprs().map(Slot::Expr).collect())
            .unwrap_or_default();
        let open_ended = self.expand_last(&mut slots);

        let supplied = slots.len();
        let expected = sig.returns.len();
        if supplied < expected && !open_ended {
            let missing_ok = sig.returns[supplied..].iter().all(Ty::admits_nil);
            if !missing_ok {
                self.report(
                    RETURN_MISMATCH,
                    range(ret.syntax()),
                    format!(
                        "expected {expected} return value{} but {supplied} {} supplied",
                        plural(expected),
                        was_were(supplied),
                    ),
                    format!("declared to return {expected} value{}", plural(expected)),
                );
            }
        } else if supplied > expected && !sig.returns_vararg {
            let extra_range = slots
                .get(expected)
                .map_or_else(|| range(ret.syntax()), slot_range);
            self.report(
                RETURN_MISMATCH,
                extra_range,
                format!(
                    "expected {expected} return value{} but {supplied} {} supplied",
                    plural(expected),
                    was_were(supplied),
                ),
                "unexpected extra return value".to_string(),
            );
        }

        for (i, slot) in slots.iter().enumerate() {
            if let Some(expected_ty) = sig.returns.get(i) {
                self.check_slot(slot, expected_ty, RETURN_MISMATCH);
            }
        }
    }
}

/// Detect a `---@field` name declared more than once for the same
/// `---@class` in this file — luals `duplicate-doc-field` (`LB0311`, #113).
///
/// Scoped to a single file: the same declarations checked standalone or in a
/// project yield the same finding, so the CLI and the LSP agree. The first
/// declaration of a name wins (its type is the one the checker uses); every
/// later `---@field` of that name on the same class is reported at its own
/// span, with a note pointing back. Indexer fields (`---@field [K] V`) are not
/// named and never collide.
pub(crate) fn duplicate_doc_fields(
    items: &[luabox_syntax::luacats::AnnotatedItem],
    file: &str,
) -> Vec<Diagnostic> {
    use luabox_syntax::luacats::{FieldKey, Tag};

    let mut seen: HashMap<String, HashSet<String>> = HashMap::new();
    let mut diags = Vec::new();
    for item in items {
        // Fields belong to the most recent `---@class` in the same block
        // (mirrors `TypeEnv::absorb_block`'s `current_class` tracking); the
        // seen-set is keyed by class *name* so a class split across blocks in
        // one file still collides.
        let mut current: Option<String> = None;
        for tag in &item.block.tags {
            match tag {
                Tag::Class(c) if !c.name.is_empty() => current = Some(c.name.clone()),
                Tag::Field(f) => {
                    let (Some(class), FieldKey::Name(name)) = (current.as_ref(), &f.key) else {
                        continue;
                    };
                    if !seen.entry(class.clone()).or_default().insert(name.clone()) {
                        diags.push(
                            Diagnostic::new(
                                Code::new(311),
                                Severity::Warning,
                                format!("duplicate field `{name}` on class `{class}`"),
                            )
                            .with_label(Label::primary(
                                Span::new(file, f.span.start..f.span.end),
                                format!("`{name}` is already declared on `{class}`"),
                            ))
                            .with_note(
                                "the first declaration wins; remove or rename this one".to_string(),
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
    }
    diags
}

// --- helpers -------------------------------------------------------------

fn range(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Collect a call's argument slots: a parenthesised list, a sole table
/// constructor (`f{ ... }`), or a sole string (`f"s"`).
fn call_slots(call: &CallExpr) -> (Vec<Slot>, Option<(Ty, Range<usize>)>) {
    arg_slots(call.args())
}

/// Collect the argument slots of an [`ArgList`] — the shared core behind
/// [`call_slots`] and the `:` method-call path (both take the same
/// parenthesised-list / sole-table / sole-string argument forms).
fn arg_slots(args: Option<ArgList>) -> (Vec<Slot>, Option<(Ty, Range<usize>)>) {
    let Some(args) = args else {
        return (Vec::new(), None);
    };
    if let Some(list) = args.expr_list() {
        return (list.exprs().map(Slot::Expr).collect(), None);
    }
    if let Some(table) = args.table_arg() {
        return (vec![Slot::Expr(Expr::Table(table))], None);
    }
    if let Some(token) = args.string_arg() {
        let r = token.text_range();
        return (
            Vec::new(),
            Some((
                Ty::StringLit(env::unquote_lua(token.text())),
                usize::from(r.start())..usize::from(r.end()),
            )),
        );
    }
    (Vec::new(), None)
}

fn slot_range(slot: &Slot) -> Range<usize> {
    match slot {
        Slot::Expr(expr) => range(expr.syntax()),
        Slot::Ty(_, r) => r.clone(),
    }
}

fn range_key(node: &SyntaxNode) -> (usize, usize) {
    let r = node.text_range();
    (usize::from(r.start()), usize::from(r.end()))
}

/// `a.b.c` as a dotted string, when the expression is a pure name path.
fn dotted_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.name()?.text().to_string()),
        Expr::Field(field) => {
            let base = dotted_name(&field.base()?)?;
            Some(format!("{base}.{}", field.field_name()?.text()))
        }
        _ => None,
    }
}

/// Whether a (possibly nested) type mentions the generic variable `name` —
/// either as `Ty::Named(name)` or the backtick capture spelling `` `name` ``.
fn ty_mentions(ty: &Ty, name: &str) -> bool {
    match ty {
        Ty::Named(n) => n == name || n.trim_matches('`') == name,
        Ty::Union(members) => members.iter().any(|m| ty_mentions(m, name)),
        Ty::Table(table) => {
            table.fields.values().any(|f| ty_mentions(&f.ty, name))
                || table.array.as_ref().is_some_and(|a| ty_mentions(a, name))
                || table
                    .indexers
                    .iter()
                    .any(|(k, v)| ty_mentions(k, name) || ty_mentions(v, name))
        }
        Ty::Function(func) => {
            func.params.iter().any(|p| ty_mentions(&p.ty, name))
                || func.returns.iter().any(|r| ty_mentions(r, name))
        }
        _ => false,
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn was_were(n: usize) -> &'static str {
    if n == 1 { "was" } else { "were" }
}
