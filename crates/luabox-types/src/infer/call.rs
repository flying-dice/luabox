//! Calls: evaluating a call expression to its (positional) return types.
//!
//! Covers plain calls, `:` method calls, `---@overload` selection, generic
//! instantiation, the modeled stdlib shapes (`setmetatable`, `require`,
//! `pcall`, ...), `---@operator call` callable classes, contextual
//! (bidirectional) seeding of function-literal arguments (#120), and the
//! call-site parameter/outgoing-call bookkeeping the display surface feeds on.

use luabox_hir::{
    BindingKind, Block, BodyId, Expr, ExprId, HirId, Literal, Resolution, Stmt, TableEntry,
};

use crate::ty::{FunctionTy, TableTy, Ty};

use super::{ITy, Infer, InferredReturn, Lookup, MethodSig, Pred, first_value, ity_union};

impl Infer<'_> {
    /// Evaluate a call, returning its full (positional) return types and
    /// whether the list is open-ended.
    pub(super) fn eval_call(&mut self, body: BodyId, expr: ExprId) -> (Vec<ITy>, bool) {
        let Expr::Call { callee, args } = self.body(body).expr(expr).clone() else {
            return (Vec::new(), true);
        };
        // Modeled builtins (global names only — shadowed names skip this).
        if let Expr::Name(name) = self.body(body).expr(callee)
            && matches!(self.resolution(body, callee), Some(Resolution::Global(_)))
        {
            let name = name.clone();
            match name.as_str() {
                "setmetatable" => return (vec![self.eval_setmetatable(body, &args)], false),
                "require" => {
                    for &arg in &args {
                        self.eval(body, arg);
                    }
                    // Display mode with cross-file inputs: a static
                    // `require("mod")` evaluates to the target module's
                    // inferred export type.
                    if let (Some(externals), Some(key)) =
                        (self.externals, self.expr_range(body, expr))
                        && let Some(edge) = self.hir.requires().iter().find(|edge| {
                            (
                                usize::from(edge.range.start()),
                                usize::from(edge.range.end()),
                            ) == key
                        })
                        && let Some(ty) = externals.requires.get(&edge.module)
                    {
                        return (vec![ITy::Ty(ty.clone())], false);
                    }
                    return (vec![ITy::unknown()], true);
                }
                "pairs" | "ipairs" | "next" | "rawget" | "rawequal" | "rawlen" | "tostring"
                | "tonumber" | "select" | "unpack" => {
                    // Read-only stdlib: arguments do not escape, but the
                    // results are not modeled.
                    for &arg in &args {
                        self.eval(body, arg);
                    }
                    let ret = match name.as_str() {
                        "tostring" => ITy::Ty(Ty::String),
                        "rawlen" => ITy::Ty(Ty::Integer),
                        _ => ITy::unknown(),
                    };
                    let open = ret.is_unknown();
                    return (vec![ret], open);
                }
                "type" => {
                    for &arg in &args {
                        self.eval(body, arg);
                    }
                    return (vec![ITy::Ty(Ty::String)], false);
                }
                "assert" => {
                    let mut vals: Vec<ITy> = Vec::new();
                    for &arg in &args {
                        vals.push(self.eval(body, arg));
                    }
                    let first = vals
                        .first()
                        .map_or_else(ITy::unknown, |v| self.narrow(v, &Pred::Truthy));
                    return (vec![first], false);
                }
                _ => {}
            }
        }

        let mut callee_ity = self.eval(body, callee);
        // A dotted callee whose signature lives only in the ambient/defs
        // registry (`string.rep`, a defs-global `mylib.f`): the base table's
        // shape carries no such field (defs register dotted functions by name,
        // not as class members), so the callee evaluates to `unknown` and the
        // call result never reaches an unannotated binding. Resolve the
        // declared signature by dotted name so its returns propagate (#106).
        if callee_ity.is_unknown()
            && let Some(dotted) = self.dotted_callee(body, callee)
            && let Some(sig) = self.env.function(&dotted)
        {
            callee_ity = ITy::Ty(Ty::Function(Box::new(sig.clone())));
        }
        // Contextual typing (#120): a function-literal argument matched to a
        // `---@param cb fun(...)` takes the expected function type's parameter
        // types for its own parameters, so its body checks against them. Seed
        // BEFORE evaluating the args — the lambda body is walked during that
        // evaluation, so the seeds must already be in place.
        self.seed_call_contextual(body, &callee_ity, &args);
        let mut arg_itys: Vec<ITy> = Vec::with_capacity(args.len());
        for &arg in &args {
            let ity = self.eval(body, arg);
            self.mark_escaped(&ity);
            arg_itys.push(ity);
        }
        match &callee_ity {
            ITy::Func(fn_body) => self.record_arg_seeds(*fn_body, &arg_itys, false),
            // Not a function this file defines: record the args by callee
            // name for dependents-side parameter seeding (`M.f(...)`).
            _ => {
                if let Some(name) = self.callee_name(body, callee) {
                    self.record_outgoing(&name, &arg_itys);
                }
            }
        }
        // A generic function (`---@generic T`): infer the type variables from
        // the argument types and substitute into the returns, so an
        // unannotated binding of the call result gets the flowed type (#84 —
        // `local n = id(5)` types `n` as `integer`).
        if let Some(sig) = self.generic_sig_of(&callee_ity) {
            let reified: Vec<Ty> = arg_itys.iter().map(|a| self.reify(a)).collect();
            let map = crate::generics::infer_call(&sig, &reified);
            let sig = crate::generics::subst_function(&sig, &map);
            return (
                sig.returns.iter().cloned().map(ITy::Ty).collect(),
                sig.returns_vararg,
            );
        }
        // Overload-aware result: if the callee's primary signature does not
        // accept the arguments but an `---@overload` does, the call yields the
        // matching overload's returns (first match wins, luals-style, #86).
        if let Some(returns) = self.overloaded_returns(&callee_ity, &arg_itys) {
            return returns;
        }
        // A value whose type is a declared `---@class` with a `---@operator
        // call` overload is callable — the operator's declared result is the
        // call's result type (LB0122).
        if let Some(returns) = self.class_call_returns(&callee_ity, &arg_itys) {
            return returns;
        }
        self.returns_of(&callee_ity)
    }

    /// The result of calling a value whose type resolves to a declared
    /// `---@class` carrying a `---@operator call` overload (LB0122). When the
    /// class declares several `call` overloads, the one whose declared input
    /// accepts the first argument wins (first-match, mirroring binary-operator
    /// selection in [`Self::operator_result`]); a no-input `call` operator
    /// accepts any arguments. `None` for any other callee, leaving the
    /// ordinary [`Self::returns_of`] path untouched (conservative: no
    /// unknown / `any` / union / plain-table callee manufactures a result).
    fn class_call_returns(&mut self, callee: &ITy, arg_itys: &[ITy]) -> Option<(Vec<ITy>, bool)> {
        let Ty::Named(class) = self.reify(callee) else {
            return None;
        };
        let sigs = self.env.class_operators(&class, "call");
        if sigs.is_empty() {
            return None;
        }
        let first_arg = arg_itys.first().map(|a| self.reify(a));
        let chosen = sigs
            .iter()
            .find(|sig| match (&sig.input, &first_arg) {
                (None, _) => true,
                (Some(input), Some(arg)) => {
                    crate::assign::assignable(self.env, self.exact, arg, input)
                }
                (Some(_), None) => false,
            })
            .unwrap_or(&sigs[0]);
        Some((vec![ITy::Ty(chosen.result.clone())], false))
    }

    /// The returns of the first `---@overload` that accepts the arguments when
    /// the primary signature does not — the value-position complement of the
    /// checker's overload acceptance (#86). `None` when the callee has no
    /// overloads or the primary already accepts (ordinary [`Self::returns_of`]).
    fn overloaded_returns(
        &mut self,
        callee_ity: &ITy,
        arg_itys: &[ITy],
    ) -> Option<(Vec<ITy>, bool)> {
        let sig = self.callee_function_ty(callee_ity)?;
        if sig.overloads.is_empty() {
            return None;
        }
        let reified_args: Vec<Ty> = arg_itys.iter().map(|a| self.reify(a)).collect();
        if self.sig_accepts(&sig, &reified_args) {
            return None;
        }
        let overload = sig
            .overloads
            .iter()
            .find(|o| self.sig_accepts(o, &reified_args))?;
        Some((
            overload.returns.iter().cloned().map(ITy::Ty).collect(),
            overload.returns_vararg,
        ))
    }

    /// The declared signature a callee value carries, if any — an annotated
    /// function type or a file-local function with a `---@param`/`---@return`
    /// signature. Used to consult `---@overload`s at the call site (#86).
    fn callee_function_ty(&self, callee: &ITy) -> Option<FunctionTy> {
        match callee {
            ITy::Ty(Ty::Function(sig)) => Some((**sig).clone()),
            ITy::Func(body) => self.funcs.get(body).and_then(|d| d.sig.clone()),
            _ => None,
        }
    }

    /// Whether `sig` accepts these (reified, positional) argument types —
    /// inference's non-reporting mirror of the checker's `call_accepts`,
    /// governing `---@overload` selection for call results (#86).
    fn sig_accepts(&self, sig: &FunctionTy, args: &[Ty]) -> bool {
        let supplied = args.len();
        if supplied < sig.required_params() {
            return false;
        }
        if supplied > sig.params.len() && sig.varargs.is_none() {
            return false;
        }
        for (i, arg) in args.iter().enumerate() {
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
            if !crate::assign::assignable(self.env, self.exact, arg, &expected) {
                return false;
            }
        }
        true
    }

    /// The annotated signature of a generic callee (`---@generic` with a
    /// `---@return`), for call-site monomorphisation. `None` for non-generic
    /// or unannotated callees (they follow the ordinary [`Self::returns_of`]).
    fn generic_sig_of(&self, callee: &ITy) -> Option<FunctionTy> {
        let sig = match callee {
            ITy::Ty(Ty::Function(sig)) => Some((**sig).clone()),
            ITy::Func(body) => self.funcs.get(body).and_then(|d| d.sig.clone()),
            _ => None,
        }?;
        (!sig.generics.is_empty() && sig.has_return_annotation).then_some(sig)
    }

    /// The fully-dotted name of a callee rooted at a *global* name
    /// (`string.rep` → `"string.rep"`), for looking its declared signature up
    /// in the ambient/defs function registry (#106). `None` when the callee is
    /// computed, indexed by a non-string-literal, or rooted at a local binding
    /// (a local shadows any same-named registry function).
    pub(super) fn dotted_callee(&self, body: BodyId, callee: ExprId) -> Option<String> {
        match self.body(body).expr(callee) {
            Expr::Name(name) => match self.resolution(body, callee) {
                Some(Resolution::Global(_)) | None => Some(name.clone()),
                _ => None,
            },
            Expr::Index { base, index, .. } => {
                let seg = match self.body(body).expr(*index) {
                    Expr::Literal(Literal::String(s)) => s.as_str()?.to_string(),
                    _ => return None,
                };
                let base = self.dotted_callee(body, *base)?;
                Some(format!("{base}.{seg}"))
            }
            _ => None,
        }
    }

    /// The terminal name of a callee expression: `f` for a plain name,
    /// `f` for `M.f` / `M["f"]` (any base). `None` for computed callees.
    fn callee_name(&self, body: BodyId, callee: ExprId) -> Option<String> {
        match self.body(body).expr(callee) {
            Expr::Name(name) => Some(name.clone()),
            Expr::Index { index, .. } => match self.body(body).expr(*index) {
                Expr::Literal(Literal::String(s)) => s.as_str().map(str::to_string),
                _ => None,
            },
            _ => None,
        }
    }

    /// Record one observed call of a function this file does not define:
    /// positional argument types, widened and unioned across call sites.
    /// Second pass only (its types are the refined ones).
    fn record_outgoing(&mut self, name: &str, args: &[ITy]) {
        if self.pass != 1 || args.is_empty() {
            return;
        }
        let tys: Vec<Ty> = args.iter().map(|a| self.reify(a).widened()).collect();
        let entry = self.outgoing.entry(name.to_string()).or_default();
        for (i, ty) in tys.into_iter().enumerate() {
            if matches!(ty, Ty::Unknown) {
                continue;
            }
            while entry.len() <= i {
                entry.push(Ty::Unknown);
            }
            entry[i] = if matches!(entry[i], Ty::Unknown) {
                ty
            } else {
                Ty::union(vec![entry[i].clone(), ty])
            };
        }
    }

    /// Record call-site argument types against a file-local function's
    /// parameter bindings (positional; `skip_self` shifts past the implicit
    /// `self` of a `:` call). Fixed types are widened — a parameter is a
    /// general slot, not the one literal a caller happened to pass.
    pub(super) fn record_arg_seeds(&mut self, fn_body: BodyId, args: &[ITy], skip_self: bool) {
        let params = self.body(fn_body).params.clone();
        let params = if skip_self
            && params
                .first()
                .is_some_and(|&p| self.binding(p).kind == BindingKind::SelfParam)
        {
            &params[1..]
        } else {
            &params[..]
        };
        for (&param, arg) in params.iter().zip(args) {
            if arg.is_unknown() {
                continue;
            }
            let seed = match arg {
                ITy::Ty(ty) => ITy::Ty(ty.widened()),
                other => other.clone(),
            };
            let merged = match self.param_seeds.get(&param) {
                Some(existing) => ity_union(vec![existing.clone(), seed]),
                None => seed,
            };
            self.param_seeds.insert(param, merged);
        }
    }

    /// Contextually type a call's arguments from the callee's declared
    /// parameter types (bidirectional typing, #120 + follow-ups). Each
    /// argument is seeded against its matching parameter through the recursive
    /// [`Self::seed_contextual`], so a function-literal argument takes its
    /// expected `fun(...)` parameter types, and a table-literal argument's
    /// function-valued fields (and nested table fields) take the expected
    /// class's declared field types.
    ///
    /// Conservative by construction:
    ///  - `callee_function_ty` yields `None` for an unannotated / `unknown` /
    ///    `any` / plain-table callee, so no expected type ⇒ no seeding
    ///    (behavior exactly as before);
    ///  - a generic callee (`---@generic`) is skipped — its callback parameter
    ///    types carry unbound placeholders, and generic callback inference is
    ///    a documented follow-up, not part of this core;
    ///  - [`Self::seed_contextual`] only acts where the expected type's own
    ///    structure directs it (a `fun(...)` for a lambda, a `---@class`/table
    ///    for a table literal); anything else seeds nothing.
    fn seed_call_contextual(&mut self, body: BodyId, callee_ity: &ITy, args: &[ExprId]) {
        let Some(sig) = self.callee_function_ty(callee_ity) else {
            return;
        };
        // Generic callbacks are deferred (#120): seeding placeholder types
        // would be meaningless. Leave them entirely to today's behavior.
        if !sig.generics.is_empty() {
            return;
        }
        for (i, &arg) in args.iter().enumerate() {
            let Some(param) = sig.params.get(i) else {
                continue;
            };
            let expected = param.ty.clone();
            self.seed_contextual(body, arg, &expected);
        }
    }

    /// Recursively seed contextual (bidirectional) types from an `expected`
    /// type into an expression, following the expected type's own structure.
    /// This mirrors luals `script/vm/compiler.lua`, which lazily compiles a
    /// node against its expected (`infer`) type and recurses through nested
    /// callbacks and table fields (`compileNode` / `compileByNode`). Two
    /// expression shapes carry context:
    ///
    ///  - a **function literal** against an expected `fun(...)`: its parameters
    ///    take the expected parameter types (so the body checks with no
    ///    per-parameter annotation), and — following the expected *return*
    ///    type — a returned function/table literal is seeded transitively, so
    ///    an `outer(function(a) return function(b) ... end end)` against
    ///    `---@param cb fun(a: A): fun(b: B)` types both `a` and `b` (#120
    ///    nested/transitive follow-up);
    ///  - a **table literal** against an expected `---@class`/table: each field
    ///    the class declares is seeded against that field's declared type, so a
    ///    function-valued field's literal takes the field's `fun` parameter
    ///    types and a nested table-literal field takes the field's class type
    ///    (contextual typing *into* a table literal, #120 follow-up).
    ///
    /// Bounded by the expected type's structure — never guessing. An
    /// `unknown`/`any`/non-matching expected type seeds nothing, exactly as
    /// today.
    pub(super) fn seed_contextual(&mut self, body: BodyId, expr: ExprId, expected: &Ty) {
        match self.body(body).expr(expr).clone() {
            Expr::Function(fn_body) => {
                let Ty::Function(expected_fn) = expected else {
                    return;
                };
                let expected_fn = expected_fn.clone();
                self.seed_lambda_params(body, expr, fn_body, &expected_fn);
                // Follow the expected return type into returned literals —
                // nested/transitive propagation through further callback or
                // table layers.
                self.seed_returns(fn_body, &expected_fn.returns);
            }
            Expr::Table { entries } => {
                let Some(shape) = self.expected_shape(expected) else {
                    return;
                };
                for entry in &entries {
                    let (name, value) = match entry {
                        TableEntry::Named { name, value } => (Some(name.clone()), *value),
                        TableEntry::Keyed { key, value } => {
                            let name = match self.body(body).expr(*key) {
                                Expr::Literal(Literal::String(s)) => s.as_str().map(str::to_string),
                                _ => None,
                            };
                            (name, *value)
                        }
                        TableEntry::Positional(_) => continue,
                    };
                    let Some(name) = name else {
                        continue;
                    };
                    if let Some(field) = shape.fields.get(&name) {
                        let fty = field.ty.clone();
                        self.seed_contextual(body, value, &fty);
                    }
                }
            }
            _ => {}
        }
    }

    /// Record contextual parameter seeds for one function-literal expression
    /// from an expected `fun(...)` type (#120): the literal's `i`-th parameter
    /// takes the expected function type's `i`-th parameter type, so the body
    /// checks against it without a per-parameter annotation. A parameter the
    /// lambda annotates itself (`---@param`) is skipped — annotations are
    /// authoritative (SPEC §3) and are applied through the ordinary `sig`
    /// path. An expected parameter typed `unknown`/`any` seeds nothing.
    fn seed_lambda_params(
        &mut self,
        body: BodyId,
        fn_expr: ExprId,
        fn_body: BodyId,
        expected_fn: &FunctionTy,
    ) {
        // The lambda's own `---@param` signature, when the harvester attached
        // one to this expression — authoritative, so those parameters are left
        // unseeded and the contextual type never overrides them.
        let own_sig = self
            .expr_range(body, fn_expr)
            .and_then(|k| self.env.fn_sig(k))
            .cloned();
        // A function *literal* (`function(...)`) never carries an implicit
        // `self`, so parameters line up positionally with the expected type's.
        let params = self.body(fn_body).params.clone();
        for (i, &param) in params.iter().enumerate() {
            let binding = self.binding(param);
            if binding.kind == BindingKind::SelfParam {
                continue;
            }
            let name = binding.name.clone();
            if own_sig
                .as_ref()
                .is_some_and(|s| s.params.iter().any(|p| p.name == name))
            {
                continue;
            }
            let Some(expected_param) = expected_fn.params.get(i) else {
                continue;
            };
            if matches!(expected_param.ty, Ty::Unknown | Ty::Any) {
                continue;
            }
            let ty = if expected_param.optional {
                expected_param.ty.clone().optional()
            } else {
                expected_param.ty.clone()
            };
            self.ctx_param_seeds.insert(param, ITy::Ty(ty));
        }
    }

    /// Seed the function/table literals a body `return`s from the enclosing
    /// (expected) return types — the transitive step that carries a
    /// `fun(...): fun(...)` expected type into a returned nested lambda, and a
    /// `---@return <Class>` into a returned table literal's fields. Descends
    /// through control-flow blocks but not into nested closures (whose returns
    /// belong to those closures).
    fn seed_returns(&mut self, fn_body: BodyId, expected: &[Ty]) {
        if expected.is_empty() {
            return;
        }
        let block = self.body(fn_body).block.clone();
        let mut rets: Vec<Vec<ExprId>> = Vec::new();
        self.collect_returns(fn_body, &block, &mut rets);
        for ret in rets {
            for (i, &e) in ret.iter().enumerate() {
                if let Some(exp) = expected.get(i) {
                    let exp = exp.clone();
                    self.seed_contextual(fn_body, e, &exp);
                }
            }
        }
    }

    /// Collect the expression lists of every `return` that belongs to `body`,
    /// descending through control-flow blocks but never into nested function
    /// literals.
    fn collect_returns(&self, body: BodyId, block: &Block, out: &mut Vec<Vec<ExprId>>) {
        for &stmt in &block.stmts {
            match self.body(body).stmt(stmt) {
                Stmt::Return(exprs) => out.push(exprs.clone()),
                Stmt::If {
                    branches,
                    else_block,
                } => {
                    for br in branches {
                        self.collect_returns(body, &br.block, out);
                    }
                    if let Some(b) = else_block {
                        self.collect_returns(body, b, out);
                    }
                }
                Stmt::While { body: b, .. }
                | Stmt::Repeat { body: b, .. }
                | Stmt::NumericFor { body: b, .. }
                | Stmt::GenericFor { body: b, .. }
                | Stmt::Do { body: b } => self.collect_returns(body, b, out),
                _ => {}
            }
        }
    }

    /// Resolve an expected type to a single class/table field shape (mirrors
    /// the checker's `table_shape`), unwrapping a `T?`/`T|nil` optional. A
    /// union of two or more real members has no single expected shape, so it
    /// seeds nothing (conservative).
    fn expected_shape(&self, expected: &Ty) -> Option<TableTy> {
        match expected {
            Ty::Named(name) => self.env.class_shape(name),
            Ty::Table(t) => Some((**t).clone()),
            Ty::Union(members) => {
                let non_nil: Vec<&Ty> = members.iter().filter(|m| **m != Ty::Nil).collect();
                match non_nil[..] {
                    [single] => self.expected_shape(single),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The inferred returns of every function *without* a `---@return`,
    /// keyed by the function's source range (the display surface behind
    /// editor return-type hints). Annotated functions are the editor's
    /// job: it renders the annotation text verbatim, which survives type
    /// names the per-file environment cannot resolve (cross-file classes).
    pub(super) fn collect_fn_returns(&mut self) -> Vec<InferredReturn> {
        let mut out = Vec::new();
        for (body_id, body) in self.hir.bodies() {
            for (expr_id, expr) in body.exprs() {
                let Expr::Function(fn_body) = expr else {
                    continue;
                };
                let Some(range) = self.hir.source_map().range(HirId::expr(body_id, expr_id)) else {
                    continue;
                };
                let Some(data) = self.funcs.get(fn_body) else {
                    continue;
                };
                if data.sig.as_ref().is_some_and(|s| s.has_return_annotation)
                    || !data.returns_set
                    || data.returns.is_empty()
                {
                    continue;
                }
                let itys = data.returns.clone();
                let returns: Vec<Ty> = itys.iter().map(|ity| self.reify(ity)).collect();
                if returns.iter().all(|ty| matches!(ty, Ty::Unknown)) {
                    continue;
                }
                out.push(InferredReturn {
                    range: usize::from(range.start())..usize::from(range.end()),
                    returns,
                });
            }
        }
        out.sort_by_key(|r| (r.range.start, r.range.end));
        out
    }

    /// `setmetatable(t, M)`: merge `t`'s shape into the shared instance
    /// shape of `M` and return it — the result's field lookups resolve
    /// through `M.__index`.
    fn eval_setmetatable(&mut self, body: BodyId, args: &[ExprId]) -> ITy {
        let t = args.first().map(|&a| self.eval(body, a));
        let m = args.get(1).map(|&a| self.eval(body, a));
        match (t, m) {
            (Some(ITy::Shape(t)), Some(ITy::Shape(m))) => {
                // Record the metatable on `t` itself — this covers the
                // carrier-inheritance idiom `setmetatable(Child, {
                // __index = Base })` where the result is discarded ...
                self.shapes[t].metatable = Some(m);
                // ... and unify constructor results on the shared
                // instance shape of `m`, so `setmetatable(o, Class)` in
                // `new` and `self` inside `Class:method()` bodies all
                // extend one shape.
                let instance = self.instance_of(m);
                if instance != t {
                    self.merge_shape_into(t, instance);
                }
                ITy::Shape(instance)
            }
            (Some(ITy::Shape(t)), _) => {
                // Untracked metatable: field lookups are no longer provable.
                self.shapes[t].meta_unknown = true;
                ITy::Shape(t)
            }
            (Some(other), _) => other,
            (None, _) => ITy::unknown(),
        }
    }

    fn merge_shape_into(&mut self, from: usize, into: usize) {
        let fields: Vec<(String, ITy)> = self.shapes[from]
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (name, ity) in fields {
            self.extend_field(into, &name, ity);
        }
        let array = self.shapes[from].array.clone();
        for ity in array {
            self.extend_array(into, ity);
        }
        let indexers = self.shapes[from].indexers.clone();
        for (key, ity) in indexers {
            self.extend_indexer(into, key, ity);
        }
        if self.shapes[from].escaped {
            self.mark_shape_escaped(into);
        }
        if self.shapes[from].meta_unknown {
            self.shapes[into].meta_unknown = true;
        }
        if self.shapes[into].declared.is_none() {
            let declared = self.shapes[from].declared.clone();
            self.shapes[into].declared = declared;
        }
    }

    pub(super) fn eval_method_call(&mut self, body: BodyId, expr: ExprId) -> (Vec<ITy>, bool) {
        let Expr::MethodCall {
            receiver,
            method,
            args,
        } = self.body(body).expr(expr).clone()
        else {
            return (Vec::new(), true);
        };
        let recv = self.eval(body, receiver);
        let mut arg_itys: Vec<ITy> = Vec::with_capacity(args.len());
        for &arg in &args {
            let ity = self.eval(body, arg);
            self.mark_escaped(&ity);
            arg_itys.push(ity);
        }
        match self.lookup_field(&recv, &method) {
            Lookup::Found(f) => {
                let recv_class = self.receiver_class(&recv);
                if let Some(class) = &recv_class {
                    self.check_visibility(body, expr, class, &method);
                }
                // Publish the resolved method signature so the annotation
                // checker can flag the callee's use-site tags and argument-check
                // the `:` call (#118, #33). Gated on the resolved member being an
                // *annotated* function value (a `---@field m fun(...)` or a
                // `function C:m` carrying a `---@param`/`---@return`/
                // `---@deprecated` signature). An unannotated method reifies to a
                // `fun` with `unknown` parameters and would manufacture arity
                // errors, so — exactly as an unannotated free function is never
                // arity-checked — it is left unpublished (mandatory
                // conservatism).
                //
                // Argument checking additionally requires the receiver to
                // resolve to a declared `---@class`; the tags do not (see
                // [`MethodSig::args_checkable`]).
                // Only pass 1's resolution is published, matching `expr_types`.
                if self.pass == 1
                    && let ITy::Ty(Ty::Function(sig)) = &f
                    && let Some(key) = self.expr_range(body, expr)
                {
                    self.method_sigs.insert(
                        key,
                        MethodSig {
                            sig: (**sig).clone(),
                            args_checkable: recv_class.is_some(),
                        },
                    );
                }
                match &f {
                    ITy::Func(fn_body) => self.record_arg_seeds(*fn_body, &arg_itys, true),
                    _ => self.record_outgoing(&method, &arg_itys),
                }
                self.returns_of(&f)
            }
            Lookup::Absent { provable, declared } => {
                if provable {
                    self.report_absent(body, expr, &method, declared.as_deref());
                }
                self.record_outgoing(&method, &arg_itys);
                (vec![ITy::unknown()], true)
            }
            Lookup::Opaque => {
                self.record_outgoing(&method, &arg_itys);
                (vec![ITy::unknown()], true)
            }
        }
    }

    /// The return types of calling a function value.
    fn returns_of(&mut self, callee: &ITy) -> (Vec<ITy>, bool) {
        match callee {
            ITy::Ty(Ty::Function(sig)) => {
                if sig.has_return_annotation {
                    (
                        sig.returns.iter().cloned().map(ITy::Ty).collect(),
                        sig.returns_vararg,
                    )
                } else {
                    (vec![ITy::unknown()], true)
                }
            }
            ITy::Func(fn_body) => match self.funcs.get(fn_body) {
                Some(data) if data.sig.as_ref().is_some_and(|s| s.has_return_annotation) => {
                    #[expect(
                        clippy::expect_used,
                        reason = "the arm guard already established `data.sig` is Some"
                    )]
                    let sig = data.sig.as_ref().expect("checked above");
                    (
                        sig.returns.iter().cloned().map(ITy::Ty).collect(),
                        sig.returns_vararg,
                    )
                }
                Some(data) if data.in_progress => (vec![ITy::unknown()], true),
                Some(data) if data.returns_set => (data.returns.clone(), false),
                Some(_) => (Vec::new(), false),
                None => (vec![ITy::unknown()], true),
            },
            _ => (vec![ITy::unknown()], true),
        }
    }

    /// Evaluate a value list, expanding a trailing multi-value producer.
    /// `want = None` collects everything (return statements).
    pub(super) fn eval_values(
        &mut self,
        body: BodyId,
        exprs: &[ExprId],
        want: Option<usize>,
    ) -> Vec<ITy> {
        let mut values: Vec<ITy> = Vec::new();
        let last = exprs.len().checked_sub(1);
        for (i, &expr) in exprs.iter().enumerate() {
            if Some(i) == last {
                match self.body(body).expr(expr) {
                    Expr::Call { .. } => {
                        let (mut rets, mut open) = self.eval_call(body, expr);
                        if let Some(ty) = self.as_override(body, expr) {
                            rets = vec![ITy::Ty(ty)];
                            open = false;
                        }
                        self.publish_call(body, expr, &rets, open);
                        Self::push_expansion(&mut values, rets, open, want);
                    }
                    Expr::MethodCall { .. } => {
                        let (mut rets, mut open) = self.eval_method_call(body, expr);
                        if let Some(ty) = self.as_override(body, expr) {
                            rets = vec![ITy::Ty(ty)];
                            open = false;
                        }
                        self.publish_call(body, expr, &rets, open);
                        Self::push_expansion(&mut values, rets, open, want);
                    }
                    Expr::Vararg => {
                        let want = want.unwrap_or(values.len() + 1);
                        while values.len() < want {
                            values.push(ITy::unknown());
                        }
                    }
                    _ => values.push(self.eval(body, expr)),
                }
            } else {
                values.push(self.eval(body, expr));
            }
        }
        if let Some(want) = want {
            while values.len() < want {
                values.push(ITy::Ty(Ty::Nil));
            }
        }
        values
    }

    fn push_expansion(values: &mut Vec<ITy>, rets: Vec<ITy>, open: bool, want: Option<usize>) {
        let base = values.len();
        values.extend(rets);
        if let Some(want) = want {
            let pad = if open {
                ITy::unknown()
            } else {
                ITy::Ty(Ty::Nil)
            };
            while values.len() < want {
                values.push(pad.clone());
            }
            values.truncate(want.max(base));
        }
    }

    /// Publish the (first-value) type of a call evaluated via the
    /// multi-value path, mirroring what [`Infer::eval`] does.
    fn publish_call(&mut self, body: BodyId, expr: ExprId, rets: &[ITy], open: bool) {
        if self.pass != 1 {
            return;
        }
        let first = first_value(rets, open);
        if first.is_unknown() {
            return;
        }
        if let Some(key) = self.expr_range(body, expr) {
            let ty = self.reify(&first);
            if ty != Ty::Unknown {
                self.expr_types.insert(key, ty);
            }
        }
    }

    /// Types of the loop variables of a generic `for`, recognizing
    /// `pairs(t)`, `ipairs(t)`, and `next, t` iteration.
    pub(super) fn iteration_tys(
        &mut self,
        body: BodyId,
        exprs: &[ExprId],
        nvars: usize,
    ) -> Vec<ITy> {
        let mut out = vec![ITy::unknown(); nvars];
        let Some(&first) = exprs.first() else {
            return out;
        };
        // `for k, v in next, t do`
        if let Expr::Name(name) = self.body(body).expr(first)
            && name == "next"
            && matches!(self.resolution(body, first), Some(Resolution::Global(_)))
            && let Some(&table_expr) = exprs.get(1)
        {
            let t = self.eval(body, table_expr);
            let (k, v) = self.pairs_tys(&t);
            if nvars > 0 {
                out[0] = k;
            }
            if nvars > 1 {
                out[1] = v;
            }
            return out;
        }
        let Expr::Call { callee, args } = self.body(body).expr(first).clone() else {
            return out;
        };
        let Expr::Name(name) = self.body(body).expr(callee) else {
            return out;
        };
        if !matches!(self.resolution(body, callee), Some(Resolution::Global(_))) {
            return out;
        }
        let name = name.clone();
        let Some(&table_expr) = args.first() else {
            return out;
        };
        match name.as_str() {
            "ipairs" => {
                let t = self.eval(body, table_expr);
                if nvars > 0 {
                    out[0] = ITy::Ty(Ty::Integer);
                }
                if nvars > 1 {
                    out[1] = self.elem_ty(&t);
                }
            }
            "pairs" | "next" => {
                let t = self.eval(body, table_expr);
                let (k, v) = self.pairs_tys(&t);
                if nvars > 0 {
                    out[0] = k;
                }
                if nvars > 1 {
                    out[1] = v;
                }
            }
            _ => {}
        }
        out
    }
}
