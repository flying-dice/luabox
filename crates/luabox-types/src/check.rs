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
//!    return; a call in last position expands to all returns).
//! b. **Assignments** to `---@type` locals.
//! c. **Returns** inside functions carrying `---@return`: count + types.
//!
//! Everything unannotated is `unknown` — permissive in warn mode, an error
//! source in strict mode (SPEC.md §3). No inference beyond literals:
//! TODO(P1) bidirectional inference, narrowing, metatables, method calls.

use std::collections::HashMap;
use std::ops::Range;

use luabox_diag::{Code, Diagnostic, Label, Severity, Span};
use luabox_syntax::lua::ast::{
    AstNode, Block, CallExpr, Expr, ParamList, ReturnStmt, SourceFile, Stmt, TableExpr,
};
use luabox_syntax::lua::{self, SyntaxNode};

use crate::assign::{assignable, is_integral_literal};
use crate::env::{self, TypeEnv};
use crate::ty::{FieldTy, FunctionTy, TableTy, Ty};

/// Diagnostic codes emitted here (block `LB03xx` — Semantics).
const TYPE_MISMATCH: u16 = 300;
const WRONG_ARG_COUNT: u16 = 301;
const MISSING_FIELD: u16 = 302;
const UNKNOWN_FIELD: u16 = 303;
const RETURN_MISMATCH: u16 = 304;
const UNKNOWN_TYPE_NAME: u16 = 305;

/// Run the checker over one parsed file. `inferred` carries the
/// inference engine's expression types keyed by byte range — consulted
/// where annotations are absent (annotations always win).
pub(crate) fn run(
    parse: &lua::Parse,
    typeenv: &TypeEnv,
    file: &str,
    strict: bool,
    inferred: &HashMap<(usize, usize), Ty>,
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
        inferred,
        diags: Vec::new(),
        scopes: vec![HashMap::new()],
        ret_stack: Vec::new(),
    };

    for (name, span) in &typeenv.unknown_names {
        checker.report(
            UNKNOWN_TYPE_NAME,
            span.start..span.end,
            format!("unknown type name `{name}` in annotation"),
            "not a built-in, `---@class`, `---@alias`, or `---@enum` name".to_string(),
        );
    }

    let tree: SourceFile = parse.tree();
    if let Some(block) = tree.block() {
        checker.visit_block(&block);
    }

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

struct Checker<'a> {
    env: &'a TypeEnv,
    file: &'a str,
    strict: bool,
    severity: Severity,
    /// Inference results by byte range (rich table inference, SPEC.md §3):
    /// the fallback for expressions annotations cannot type.
    inferred: &'a HashMap<(usize, usize), Ty>,
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
                    self.visit_expr(target);
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
            Stmt::Call(call) => self.visit_opt_expr(call.expr()),
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
                // TODO(P1): resolve method receivers through class shapes.
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
            Expr::Field(field) => self.visit_opt_expr(field.base()),
            Expr::Name(_) | Expr::Literal(_) | Expr::Vararg(_) => {}
        }
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
                    Some(sig) if sig.has_return_annotation => {
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

    // --- rule a: call sites ---------------------------------------------

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

        // Overloaded stdlib functions (e.g. `tonumber`, `table.insert`): a
        // call is accepted when it matches the primary signature *or* any
        // `---@overload`. Only when none match do we report against the
        // primary (TODO(P1): pick and report the closest overload).
        if !sig.overloads.is_empty()
            && (self.call_accepts(&sig, &slots, open_ended)
                || sig
                    .overloads
                    .iter()
                    .any(|o| self.call_accepts(o, &slots, open_ended)))
        {
            return;
        }

        let supplied = slots.len();
        let required = sig.required_params();
        let max = sig.params.len();
        let call_range = range(call.syntax());

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
            self.report(
                mismatch_code,
                slot_range(slot),
                format!("{noun}: expected `{expected}`, found `{found}`"),
                format!("expected `{expected}`"),
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
        for (name, field) in &shape.fields {
            if !field.optional && !field.ty.admits_nil() && !present.contains_key(name) {
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

// --- helpers -------------------------------------------------------------

fn range(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Collect a call's argument slots: a parenthesised list, a sole table
/// constructor (`f{ ... }`), or a sole string (`f"s"`).
fn call_slots(call: &CallExpr) -> (Vec<Slot>, Option<(Ty, Range<usize>)>) {
    let Some(args) = call.args() else {
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

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn was_were(n: usize) -> &'static str {
    if n == 1 { "was" } else { "were" }
}
