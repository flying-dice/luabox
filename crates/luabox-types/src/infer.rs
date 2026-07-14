//! Rich table inference over the HIR (SPEC.md §3 — hard requirement).
//!
//! Tables never degrade to a bare `table` type: every table constructed in
//! the file gets an identity-tracked *shape* ([`ShapeData`]) that later
//! statements extend — constructor entries, `t.x = v` assignments,
//! `function T.f()` / `function T:m()` declarations, and
//! `setmetatable`/`__index` chains — so idiomatic Lua OOP
//! (`Class.__index = Class`, `:` methods, inheritance by `__index`
//! delegation) types correctly with **zero annotations**.
//!
//! # Architecture
//!
//! Two flow-ordered passes over the [`LoweredFile`] (a bounded two-step
//! fixpoint — no general iteration):
//!
//! 1. **Build**: walk every body in flow order, allocating shapes for table
//!    constructors (keyed by construction site so the second pass reuses
//!    them), extending shapes from assignments, wiring metatables, and
//!    recording inferred function returns.
//! 2. **Emit**: the identical walk, now against *final* shapes — publishes
//!    a `byte-range → type` table the annotation checker consults for
//!    expressions it cannot type itself, and reports reads of provably
//!    absent fields (`LB0306`).
//!
//! The per-binding type state is a flat `BindingId → ITy` map (binding ids
//! are file-global, so upvalue reads in nested closures come for free).
//! `local` assignments *replace* the state in flow order; branch arms are
//! walked on cloned states and merged by union at the join; a branch that
//! ends in `return`/`break` contributes nothing (which is what makes
//! early-return narrowing fall out naturally). Annotated bindings are
//! authoritative: their declared type is the state, and assignments never
//! overwrite it (the annotation checker diagnoses those).
//!
//! Flow-sensitive narrowing covers `if type(x) == "..."`, truthiness
//! (`if x then` strips `nil`/`false`), `x == nil` / `x ~= nil`, literal
//! equality, `not`, and `and`/`or` combinations — all union-based and
//! intraprocedural.
//!
//! # Conservatism
//!
//! `LB0306` (absent-field read) only fires when the receiver's shape is
//! *fully known*: never for shapes that escaped into unanalyzed code
//! (arguments to unmodeled calls), shapes with indexers or dynamic-key
//! writes, shapes whose metatable is unresolved, or carriers bound to a
//! `---@class` (its declaration governs instead). Unknown stays `unknown` —
//! never `any`.

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_diag::{Code, Diagnostic, Label, Severity, Span};
use luabox_hir::{
    BinOp, Binding, BindingId, BindingKind, Block, Body, BodyId, Expr, ExprId, HirId, IfBranch,
    Literal, LoweredFile, Resolution, Stmt, StmtId, TableEntry, UnOp,
};

use luabox_syntax::luacats;

use crate::env::TypeEnv;
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// Diagnostic codes emitted here (block `LB03xx` — Semantics).
const FIELD_NOT_FOUND: u16 = 306;
/// Access of a `---@private`/`---@protected`/`---@package` member from outside
/// its visibility scope (luals `invisible`, #115).
const INVISIBLE: u16 = 312;

/// A byte range key, matching the annotation checker's convention.
type Key = (usize, usize);

/// What inference hands back to the checker.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Reified expression types keyed by byte range. Only expressions the
    /// annotation checker cannot type itself are published; `unknown`
    /// results are omitted.
    pub(crate) expr_types: HashMap<Key, Ty>,
    /// The resolved function signature of a `:` method call, keyed by the
    /// method-call expression's byte range. Published only when the receiver
    /// resolves to a concrete field whose type is a function — the engine's
    /// method resolution the annotation checker consumes to argument-check the
    /// call (#118). The signature is as-declared (including its `self`
    /// parameter, if any, and `---@deprecated`); the checker strips the
    /// implicit `self` before matching explicit arguments.
    pub(crate) method_sigs: HashMap<Key, FunctionTy>,
    /// Inference's own diagnostics (`LB0306`).
    pub(crate) diags: Vec<Diagnostic>,
    /// Final reified type of every binding, in declaration order (the
    /// [`crate::infer_display_types`] surface behind editor inlay hints;
    /// the "bare `table` never appears" acceptance check walks it too).
    pub(crate) binding_types: Vec<InferredBinding>,
    /// Inferred return types of every unannotated function, keyed by the
    /// function's source range (the [`crate::infer_display_types`] surface
    /// behind editor return-type hints).
    pub(crate) fn_returns: Vec<InferredReturn>,
    /// The reified type of the chunk's first `return` value — the module's
    /// export surface, consumed by *other* files' display inference to type
    /// their `require` results.
    pub(crate) module_export: Option<Ty>,
    /// Argument types observed at calls of functions this file does *not*
    /// define, keyed by the callee's terminal name (`M.area(3, 4)` and
    /// `obj:area(3, 4)` both record under `area`). Positional unions,
    /// widened; the cross-file half of call-site parameter seeding.
    pub(crate) outgoing_calls: HashMap<String, Vec<Ty>>,
    /// Final accumulated structural type of each `---@type` carrier local
    /// (`local X = {}` extended by later `X.f = ...` / `function X:m()`),
    /// keyed by the `local` statement's byte range. The whole-carrier shape
    /// the checker's deferred `---@type` conformance check runs against.
    /// `---@class Name : Parent` carriers publish their
    /// reified accumulated shape here too, keyed by the `local` statement, so
    /// the checker can verify `: Interface` conformance (#107).
    pub(crate) carrier_final: HashMap<Key, Ty>,
    /// Reified accumulated shape of every `---@class` carrier, keyed by the
    /// class *name* — the parent-carrier lookup the `: Interface` conformance
    /// check consults so a member inherited from a same-file base carrier via
    /// a `Child.__index = Base` chain (which the carrier's own reified shape
    /// does not fold in) is still counted as provided (#107).
    pub(crate) carrier_class_final: HashMap<String, Ty>,
}

/// Cross-file inputs to display-mode inference, assembled by the analysis
/// layer from the *other* files of the project.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ExternalTypes {
    /// Module string → the target file's inferred export type
    /// ([`Outcome::module_export`] of the resolved file): what
    /// `require("mod")` evaluates to.
    pub requires: HashMap<String, Ty>,
    /// Function name → positional argument types observed at call sites in
    /// dependent files ([`Outcome::outgoing_calls`] of every file that
    /// requires this one): seeds for this file's exported functions.
    pub fn_param_seeds: HashMap<String, Vec<Ty>>,
}

/// One binding's final inferred type: the declaration-site name range plus
/// the reified type. What editors render as an inlay hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredBinding {
    /// The binding's name.
    pub name: String,
    /// What introduced the binding (local / param / for-var ...).
    pub kind: BindingKind,
    /// The byte range of the name token at the declaration site.
    pub range: std::ops::Range<usize>,
    /// The reified inferred type.
    pub ty: Ty,
}

/// The inferred return types of one function without a `---@return`
/// annotation, keyed by the byte range of the whole function
/// (declaration statement or `function` expression).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredReturn {
    /// The byte range of the function in the source.
    pub range: std::ops::Range<usize>,
    /// The reified positional return types (unioned across `return`
    /// statements, padded with `nil`).
    pub returns: Vec<Ty>,
}

/// Run inference over one lowered file.
///
/// `seed_params` turns on call-site parameter inference: an unannotated
/// parameter takes the union of the (widened) argument types observed at
/// the function's call sites during the first pass. `externals` carries the
/// cross-file half (require exports + dependent files' call args). Both are
/// display-only — the checker runs without them, so neither can manufacture
/// diagnostics.
pub(crate) fn run(
    hir: &LoweredFile,
    env: &TypeEnv,
    file: &str,
    strict: bool,
    seed_params: bool,
    externals: Option<&ExternalTypes>,
) -> Outcome {
    let severity = if strict {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut infer = Infer {
        env,
        hir,
        file,
        severity,
        strict,
        pass: 0,
        seed_params,
        externals,
        shapes: Vec::new(),
        shape_of_expr: HashMap::new(),
        instances: HashMap::new(),
        declared_carriers: HashMap::new(),
        carrier_locals: HashMap::new(),
        class_carrier_locals: HashMap::new(),
        carrier_keys: HashSet::new(),
        funcs: HashMap::new(),
        param_seeds: HashMap::new(),
        ctx_param_seeds: HashMap::new(),
        outgoing: HashMap::new(),
        state: HashMap::new(),
        declared: HashSet::new(),
        globals: HashMap::new(),
        expr_types: HashMap::new(),
        method_sigs: HashMap::new(),
        diags: Vec::new(),
        memo: HashMap::new(),
        reify_stack: Vec::new(),
        class_ctx: Vec::new(),
    };
    infer.run_pass();
    infer.pass = 1;
    infer.run_pass();

    let mut binding_types = Vec::new();
    for (id, binding) in hir.bindings() {
        if let Some(ity) = infer.state.get(&id).cloned() {
            let ty = infer.reify(&ity);
            binding_types.push(InferredBinding {
                name: binding.name.clone(),
                kind: binding.kind,
                range: usize::from(binding.range.start())..usize::from(binding.range.end()),
                ty,
            });
        }
    }
    let fn_returns = infer.collect_fn_returns();
    let module_export = infer
        .funcs
        .get(&hir.chunk())
        .filter(|data| data.returns_set)
        .and_then(|data| data.returns.first().cloned())
        .map(|ity| infer.reify(&ity));
    // The final accumulated shape of every carrier local, snapshotted after
    // both passes so later `X.f = ...` / `function X:m()` extensions are all
    // in (whole-carrier conformance).
    let carriers: Vec<(Key, BindingId)> =
        infer.carrier_locals.iter().map(|(k, v)| (*k, *v)).collect();
    let mut carrier_final = HashMap::new();
    for (key, binding) in carriers {
        if let Some(ity) = infer.state.get(&binding).cloned() {
            let ty = infer.reify(&ity);
            carrier_final.insert(key, ty);
        }
    }
    // `---@class` carriers publish their final reified shape twice: keyed by
    // the `local` statement (so the checker can attribute an obligation to the
    // exact carrier) and by class name (the parent-carrier fallback). The
    // reified shape folds in `setmetatable(X, { __index = Base })`-style
    // inheritance via `reify_shape`'s `__index` walk; the name-keyed map
    // covers the `X.__index = Base` chain the carrier's own shape omits (#107).
    let class_carriers: Vec<(Key, BindingId, String)> = infer
        .class_carrier_locals
        .iter()
        .map(|(k, (b, n))| (*k, *b, n.clone()))
        .collect();
    let mut carrier_class_final = HashMap::new();
    for (key, binding, name) in class_carriers {
        if let Some(ity) = infer.state.get(&binding).cloned() {
            let ty = infer.reify(&ity);
            carrier_final.entry(key).or_insert_with(|| ty.clone());
            carrier_class_final.insert(name, ty);
        }
    }
    Outcome {
        expr_types: infer.expr_types,
        method_sigs: infer.method_sigs,
        diags: infer.diags,
        binding_types,
        fn_returns,
        module_export,
        outgoing_calls: infer.outgoing,
        carrier_final,
        carrier_class_final,
    }
}

// === The inferred-type lattice ===

/// An inference-time type: either a fixed structural [`Ty`], a reference to
/// a mutable table shape, an inferred function, or a union of those.
#[derive(Debug, Clone, PartialEq)]
enum ITy {
    /// A fixed type (annotation- or literal-derived). Never contains shape
    /// references.
    Ty(Ty),
    /// A locally-constructed table, identity-tracked so later assignments
    /// extend it everywhere it flows.
    Shape(usize),
    /// An unannotated function literal; its signature lives in [`FuncData`].
    Func(BodyId),
    /// A flattened, deduplicated union (at least two members, no nested
    /// unions).
    Union(Vec<ITy>),
}

impl ITy {
    fn unknown() -> ITy {
        ITy::Ty(Ty::Unknown)
    }

    fn is_unknown(&self) -> bool {
        matches!(self, ITy::Ty(Ty::Unknown))
    }
}

/// Union of inference types: flatten, dedup, and drop `unknown` when any
/// concrete member is present (optimistic — unannotated code must check).
#[expect(
    clippy::expect_used,
    reason = "reached only in the `flat.len() == 1` match arm, so `pop` always yields the single element"
)]
fn ity_union(members: Vec<ITy>) -> ITy {
    let mut flat: Vec<ITy> = Vec::new();
    let push = |ity: ITy, flat: &mut Vec<ITy>| {
        if !ity.is_unknown() && !flat.contains(&ity) {
            flat.push(ity);
        }
    };
    for member in members {
        match member {
            ITy::Union(inner) => {
                for ity in inner {
                    push(ity, &mut flat);
                }
            }
            other => push(other, &mut flat),
        }
    }
    match flat.len() {
        0 => ITy::unknown(),
        1 => flat.pop().expect("non-empty"),
        _ => ITy::Union(flat),
    }
}

fn ity_members(ity: &ITy) -> Vec<ITy> {
    match ity {
        ITy::Union(members) => members.clone(),
        other => vec![other.clone()],
    }
}

// === Shapes & inferred functions ===

/// The mutable inferred shape of one locally-constructed table.
#[derive(Debug, Default)]
struct ShapeData {
    /// Named fields, union-extended by constructors and assignments.
    fields: BTreeMap<String, ITy>,
    /// Array-part element candidates (deduplicated).
    array: Vec<ITy>,
    /// Dynamic-key writes: `[K] = V` pairs with generalized key types.
    indexers: Vec<(Ty, ITy)>,
    /// The metatable installed by `setmetatable(t, M)`, when tracked.
    metatable: Option<usize>,
    /// `setmetatable` was called with an untracked metatable — field
    /// lookups can no longer be proven absent.
    meta_unknown: bool,
    /// The `---@class` name bound to this table's declaration,
    /// when any. Field lookups consult the declaration; `LB0306` defers to
    /// the declaration's own diagnostics.
    declared: Option<String>,
    /// This is the shared *instance* shape of a carrier (the type of
    /// `setmetatable(x, Carrier)` results and of `self` in the carrier's
    /// methods). Declared instances reify as their declared name, so
    /// constructor results unify with `---@return <Class>` (#73).
    is_instance: bool,
    /// The value flowed somewhere analysis cannot see (argument to an
    /// unmodeled call). Suppresses `LB0306`.
    escaped: bool,
}

/// The inferred signature of one function body.
#[derive(Debug, Default)]
struct FuncData {
    /// The annotated signature, when one exists — authoritative.
    sig: Option<FunctionTy>,
    /// Inferred positional returns (union across `return` statements,
    /// padded with `nil`).
    returns: Vec<ITy>,
    /// Whether any `return` statement was seen.
    returns_set: bool,
    /// The body is currently being walked (recursion guard).
    in_progress: bool,
}

/// The outcome of a field lookup on an inference type.
enum Lookup {
    Found(ITy),
    /// The field is absent; `provable` means the whole shape (and its
    /// metatable chain) is fully known, so the absence is a diagnosis.
    /// `declared` names the `---@class` the receiver resolved to when the
    /// absence is on a *declared* shape (`self` in a class method, a
    /// `---@type Class` value) — the luals `undefined-field` case (#90),
    /// distinguished from an inferred table so the message can name the
    /// class and point at its declaration.
    Absent {
        provable: bool,
        declared: Option<String>,
    },
    /// The receiver is not a table we can inspect.
    Opaque,
}

/// A resolved assignment target.
enum Target {
    Binding { id: BindingId, upvalue: bool },
    Global(String),
    Field { shape: Option<usize>, name: String },
    ArrayElem { shape: Option<usize> },
    Indexer { shape: Option<usize>, key: Ty },
    Opaque,
}

/// What a `---@cast` override writes into.
enum CastTarget {
    Binding(BindingId),
    Global(String),
}

/// A narrowing predicate derived from a condition.
#[derive(Debug, Clone)]
enum Pred {
    Truthy,
    Falsy,
    Nil,
    NonNil,
    TypeIs(&'static str),
    NotTypeIs(&'static str),
    Lit(Ty),
    NotLit(Ty),
}

struct Infer<'a> {
    env: &'a TypeEnv,
    hir: &'a LoweredFile,
    file: &'a str,
    severity: Severity,
    /// Strict mode (drives assignability inside carrier classification).
    strict: bool,
    /// 0 = build shapes, 1 = emit diagnostics + publish types.
    pass: u8,
    /// Seed unannotated parameters from call-site argument types
    /// (display-only inference; see [`run`]).
    seed_params: bool,
    /// Cross-file inputs (require exports + dependents' call args), when
    /// the analysis layer supplied them. Display-only.
    externals: Option<&'a ExternalTypes>,
    shapes: Vec<ShapeData>,
    /// Constructor site → shape, so pass 2 reuses pass 1's identities.
    shape_of_expr: HashMap<(BodyId, ExprId), usize>,
    /// Carrier shape → its shared instance shape.
    instances: HashMap<usize, usize>,
    /// Declared name → the carrier shape bound to that declaration, so an
    /// annotated instance value (`---@return Circle`) still resolves
    /// methods and inferred extensions through the carrier (#73).
    declared_carriers: HashMap<String, usize>,
    /// `---@type` carrier locals (`local X = {}` whose object annotation is
    /// satisfied only by later extension), keyed by the `local` statement's
    /// byte range → the carrier binding. Their final shape is published as
    /// [`Outcome::carrier_final`] for the deferred conformance check.
    carrier_locals: HashMap<Key, BindingId>,
    /// `---@class Name : ...` carriers (`local X = {}` tagged with a class
    /// that has parents), keyed by the `local` statement's byte range → the
    /// carrier binding, plus the class name. Their final reified shape feeds
    /// the checker's `: Interface` conformance check (#107).
    class_carrier_locals: HashMap<Key, (BindingId, String)>,
    /// The statement keys classified as carriers in pass 0, reused verbatim
    /// in pass 1 so the keep-the-shape decision (rather than freeze to the
    /// annotated type) is identical across passes.
    carrier_keys: HashSet<Key>,
    funcs: HashMap<BodyId, FuncData>,
    /// Running union of argument types observed at call sites, per
    /// parameter binding. Persists across passes: pass 0 collects, pass 1
    /// walks bodies with the seeds applied (the bounded fixpoint's one
    /// extra step). Only read when [`Self::seed_params`] is on.
    param_seeds: HashMap<BindingId, ITy>,
    /// Contextual (bidirectional) parameter types (#120): the parameter
    /// bindings of a function *literal* written in a position whose EXPECTED
    /// type is a `fun(...)` — a call argument matched to a `---@param cb
    /// fun(...)`, the initializer of a `---@type fun(...)` local, a `---@return
    /// fun(...)` return expression, or a function-valued field of a
    /// contextually-typed table literal — including layers reached
    /// transitively through an expected `fun(a): fun(b)` return type or a
    /// nested table field (all seeded by [`Self::seed_contextual`]). Each
    /// param takes the expected function type's corresponding parameter type
    /// so the lambda body type-checks against it with no per-parameter
    /// annotation. Unlike [`Self::param_seeds`] (observed-call, display-only)
    /// this is **annotation-derived**, so it is consulted on the *check* path
    /// too — the whole point is that the checker sees the lambda params typed.
    /// A parameter the lambda annotates itself (`---@param`) is left out
    /// (annotations are authoritative, SPEC §3). Populated immediately before
    /// the lambda body is walked; keyed by file-global binding id, so the
    /// value is deterministic and identical across both passes.
    ctx_param_seeds: HashMap<BindingId, ITy>,
    /// Flow state: binding → current inferred type (flat across bodies).
    state: HashMap<BindingId, ITy>,
    /// Argument types observed at calls of functions not defined in this
    /// file, keyed by terminal callee name (see [`Outcome::outgoing_calls`]).
    outgoing: HashMap<String, Vec<Ty>>,
    /// Bindings with an authoritative annotated type (never overwritten).
    declared: HashSet<BindingId>,
    globals: HashMap<String, ITy>,
    expr_types: HashMap<Key, Ty>,
    method_sigs: HashMap<Key, FunctionTy>,
    diags: Vec<Diagnostic>,
    memo: HashMap<usize, Ty>,
    reify_stack: Vec<usize>,
    /// The stack of enclosing `---@class` method contexts (#115): the class a
    /// carrier method (`function C:m()` / `function C.m()`) is attached to,
    /// pushed while its body is walked. An access `recv.member` is "inside the
    /// class" — the luals `getEnvClass` determination — when the owner of a
    /// restricted `member` is present in this stack (or, for `protected`, a
    /// superclass of an entry). Kept as a stack, not a single slot, so a nested
    /// closure inside a method still counts as inside the class (a deliberate
    /// widening over luals's nearest-function rule, in the conservative
    /// no-false-positive direction).
    class_ctx: Vec<String>,
}

impl Infer<'_> {
    fn run_pass(&mut self) {
        self.state.clear();
        self.declared.clear();
        self.globals.clear();
        let chunk = self.hir.chunk();
        self.walk_body(chunk, None, None);
    }

    // --- plumbing ------------------------------------------------------

    fn body(&self, body: BodyId) -> &'_ Body {
        self.hir.body(body)
    }

    fn expr_range(&self, body: BodyId, expr: ExprId) -> Option<Key> {
        self.hir
            .source_map()
            .range(HirId::expr(body, expr))
            .map(|r| (usize::from(r.start()), usize::from(r.end())))
    }

    fn stmt_range(&self, body: BodyId, stmt: StmtId) -> Option<Key> {
        self.hir
            .source_map()
            .range(HirId::stmt(body, stmt))
            .map(|r| (usize::from(r.start()), usize::from(r.end())))
    }

    fn resolution(&self, body: BodyId, expr: ExprId) -> Option<&Resolution> {
        self.hir.resolution(HirId::expr(body, expr))
    }

    fn binding(&self, id: BindingId) -> &Binding {
        self.hir.binding(id)
    }

    fn alloc_shape(&mut self, body: BodyId, expr: ExprId) -> usize {
        if let Some(&id) = self.shape_of_expr.get(&(body, expr)) {
            return id;
        }
        let id = self.shapes.len();
        self.shapes.push(ShapeData::default());
        self.shape_of_expr.insert((body, expr), id);
        id
    }

    /// The shared instance shape of a carrier/metatable (created lazily).
    fn instance_of(&mut self, carrier: usize) -> usize {
        if let Some(&id) = self.instances.get(&carrier) {
            return id;
        }
        let id = self.shapes.len();
        self.shapes.push(ShapeData {
            metatable: Some(carrier),
            declared: self.shapes[carrier].declared.clone(),
            is_instance: true,
            ..ShapeData::default()
        });
        self.instances.insert(carrier, id);
        id
    }

    fn extend_field(&mut self, shape: usize, name: &str, ity: ITy) {
        match self.shapes[shape].fields.get(name) {
            Some(existing) => {
                let merged = ity_union(vec![existing.clone(), ity]);
                self.shapes[shape].fields.insert(name.to_string(), merged);
            }
            None => {
                self.shapes[shape].fields.insert(name.to_string(), ity);
            }
        }
    }

    fn extend_array(&mut self, shape: usize, ity: ITy) {
        if !self.shapes[shape].array.contains(&ity) {
            self.shapes[shape].array.push(ity);
        }
    }

    fn extend_indexer(&mut self, shape: usize, key: Ty, ity: ITy) {
        let entry = (key, ity);
        if !self.shapes[shape].indexers.contains(&entry) {
            self.shapes[shape].indexers.push(entry);
        }
    }

    /// Mark a value as escaped into unanalyzed code (transitively through
    /// the fields reachable from it).
    fn mark_escaped(&mut self, ity: &ITy) {
        match ity {
            ITy::Shape(id) => self.mark_shape_escaped(*id),
            ITy::Union(members) => {
                for member in members.clone() {
                    self.mark_escaped(&member);
                }
            }
            _ => {}
        }
    }

    fn mark_shape_escaped(&mut self, id: usize) {
        if self.shapes[id].escaped {
            return;
        }
        self.shapes[id].escaped = true;
        let reachable: Vec<ITy> = self.shapes[id]
            .fields
            .values()
            .chain(self.shapes[id].array.iter())
            .chain(self.shapes[id].indexers.iter().map(|(_, v)| v))
            .cloned()
            .collect();
        for ity in reachable {
            self.mark_escaped(&ity);
        }
    }

    /// Report an absent-field read (`LB0306`). When `declared` names the
    /// `---@class` the receiver resolved to, the message is luals'
    /// `undefined-field` phrasing and — where the class is declared in this
    /// file — carries a "declared here" secondary label (#90). For an
    /// inferred table it keeps the constructor/metatable phrasing.
    fn report_absent(&mut self, body: BodyId, expr: ExprId, name: &str, declared: Option<&str>) {
        if self.pass != 1 {
            return;
        }
        let Some((start, end)) = self.expr_range(body, expr) else {
            return;
        };
        let (message, label) = match declared {
            Some(class) => (
                format!("undefined field `{name}` on `{class}`"),
                format!("`{class}` declares no field `{name}`"),
            ),
            None => (
                format!("cannot find field `{name}` on this table"),
                format!(
                    "`{name}` is not defined by the table's constructor, assignments, or metatable chain"
                ),
            ),
        };
        let mut diag = Diagnostic::new(Code::new(FIELD_NOT_FOUND), self.severity, message)
            .with_label(Label::primary(Span::new(self.file, start..end), label));
        if let Some(class) = declared
            && let Some(range) = self.env.class_decl_span(class)
        {
            diag = diag.with_label(Label::secondary(
                Span::new(self.file.to_string(), range),
                format!("`{class}` declared here"),
            ));
        }
        self.diags.push(diag);
    }

    // --- member visibility (luals `invisible`, #115) ---------------------

    /// The single `---@class` a receiver value resolves to, if any: a
    /// `Ty::Named` class, or an inference shape whose (metatable-chained)
    /// declaration names a class. `None` for anything else (a union, a plain
    /// table, `unknown`) — the conservative direction, so visibility is only
    /// ever judged against an unambiguous class receiver.
    fn receiver_class(&self, recv: &ITy) -> Option<String> {
        match recv {
            ITy::Ty(Ty::Named(class)) if self.env.is_class(class) => Some(class.clone()),
            ITy::Shape(id) => self.shape_declared_class(*id),
            _ => None,
        }
    }

    /// The declared `---@class` name of a shape, following the `__index` chain
    /// (the receiver-side of [`Self::receiver_class`], reused to name the
    /// enclosing class of a carrier method).
    fn shape_declared_class(&self, id: usize) -> Option<String> {
        let mut cur = Some(id);
        let mut seen = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            if let Some(class) = &self.shapes[s].declared
                && self.env.is_class(class)
            {
                return Some(class.clone());
            }
            cur = self.index_delegate(s);
        }
        None
    }

    /// Check an access `recv.member` (or `recv:member()`) against the member's
    /// declared visibility and report `invisible` (`LB0312`) when it is not
    /// reachable from here (#115). `recv_class` is the receiver's resolved
    /// class; a public member (or one on a non-restricting class) is silent.
    fn check_visibility(&mut self, body: BodyId, expr: ExprId, recv_class: &str, member: &str) {
        if self.pass != 1 {
            return;
        }
        let Some((scope, owner)) = self.env.member_visibility(recv_class, member) else {
            return;
        };
        let allowed = match scope {
            luacats::FieldScope::Public => return,
            // Private: only the owning class's own methods.
            luacats::FieldScope::Private => self.class_ctx.iter().any(|c| c == &owner),
            // Protected: the owning class or any subclass method.
            luacats::FieldScope::Protected => self
                .class_ctx
                .iter()
                .any(|c| c == &owner || self.env.is_subclass(c, &owner)),
            // Package: anywhere in the file that declares the owning class.
            luacats::FieldScope::Package => self.env.declares_class_locally(&owner),
        };
        if !allowed {
            self.report_invisible(body, expr, member, &owner, scope);
        }
    }

    /// Report an `invisible` access (`LB0312`). Follows the strictness ladder
    /// like its sibling `undefined-field` (`LB0306`) — a warning in warn mode,
    /// an error in strict — which is stricter than luals (always a warning).
    fn report_invisible(
        &mut self,
        body: BodyId,
        expr: ExprId,
        member: &str,
        owner: &str,
        scope: luacats::FieldScope,
    ) {
        let Some((start, end)) = self.expr_range(body, expr) else {
            return;
        };
        let (kind, reach) = match scope {
            luacats::FieldScope::Private => ("private", "its own class"),
            luacats::FieldScope::Protected => ("protected", "its class and subclasses"),
            luacats::FieldScope::Package => ("package", "the file that declares its class"),
            luacats::FieldScope::Public => return,
        };
        let mut diag = Diagnostic::new(
            Code::new(INVISIBLE),
            self.severity,
            format!("cannot access {kind} member `{member}` of `{owner}` here"),
        )
        .with_label(Label::primary(
            Span::new(self.file, start..end),
            format!("`{member}` is {kind} to `{owner}` — accessible only from {reach}"),
        ));
        if let Some(range) = self.env.class_decl_span(owner) {
            diag = diag.with_label(Label::secondary(
                Span::new(self.file.to_string(), range),
                format!("`{owner}` declared here"),
            ));
        }
        self.diags.push(diag);
    }

    // --- reification -----------------------------------------------------

    /// Snapshot an inference type as a plain structural [`Ty`].
    fn reify(&mut self, ity: &ITy) -> Ty {
        match ity {
            ITy::Ty(ty) => ty.clone(),
            ITy::Shape(id) => self.reify_shape(*id),
            ITy::Func(body) => Ty::Function(Box::new(self.reify_func(*body))),
            ITy::Union(members) => {
                let members = members.clone();
                Ty::union(members.iter().map(|m| self.reify(m)).collect())
            }
        }
    }

    fn reify_func(&mut self, body: BodyId) -> FunctionTy {
        if let Some(sig) = self.funcs.get(&body).and_then(|f| f.sig.clone()) {
            return sig;
        }
        let hir_body = self.body(body);
        let params: Vec<ParamTy> = hir_body
            .params
            .iter()
            .map(|&p| ParamTy {
                name: self.binding(p).name.clone(),
                ty: Ty::Unknown,
                optional: false,
            })
            .collect();
        let varargs = hir_body.is_vararg.then_some(Ty::Unknown);
        let (returns_set, returns) = match self.funcs.get(&body) {
            Some(data) => (data.returns_set, data.returns.clone()),
            None => (false, Vec::new()),
        };
        let returns = if returns_set {
            returns.iter().map(|r| self.reify(r)).collect()
        } else {
            Vec::new()
        };
        FunctionTy {
            params,
            varargs,
            returns,
            returns_vararg: false,
            // In display mode the inferred returns are the signature: a
            // dependent file calling this exported function gets them. The
            // checker (seed_params off) keeps the conservative `false`.
            has_return_annotation: returns_set && self.seed_params,
            overloads: Vec::new(),
            generics: Vec::new(),
            // Inferred (unannotated) functions carry no doc-comment flags.
            deprecated: false,
            nodiscard: false,
            is_async: false,
            version: None,
        }
    }

    /// Reify a shape: own fields plus the flattened `__index` chain
    /// (nearest definition wins), skipping `__`-metafields. Cycles cut off
    /// with the catch-all table shape.
    fn reify_shape(&mut self, id: usize) -> Ty {
        // A declared *instance* shape reifies as its declared name: the
        // result of `setmetatable(x, Carrier)` (and `self` in the carrier's
        // methods) IS the declared class/struct at annotated boundaries, so
        // constructors satisfy `---@return <Class>` (#73).
        if self.shapes[id].is_instance
            && let Some(name) = self.shapes[id].declared.clone()
            && self.env.resolve_named(&name).is_some()
        {
            return Ty::Named(name);
        }
        if let Some(ty) = self.memo.get(&id) {
            return ty.clone();
        }
        if self.reify_stack.contains(&id) {
            return Ty::any_table();
        }
        self.reify_stack.push(id);

        let mut fields: BTreeMap<String, ITy> = BTreeMap::new();
        let mut indexers: Vec<(Ty, ITy)> = Vec::new();
        let mut array: Vec<ITy> = Vec::new();
        let mut cur = Some(id);
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            for (name, ity) in &self.shapes[s].fields {
                if !name.starts_with("__") && !fields.contains_key(name) {
                    fields.insert(name.clone(), ity.clone());
                }
            }
            for entry in &self.shapes[s].indexers {
                if !indexers.contains(entry) {
                    indexers.push(entry.clone());
                }
            }
            for ity in &self.shapes[s].array {
                if !array.contains(ity) {
                    array.push(ity.clone());
                }
            }
            cur = self.index_delegate(s);
        }

        let mut table = TableTy::default();
        for (name, ity) in fields {
            let ty = self.reify(&ity);
            table.fields.insert(
                name,
                FieldTy {
                    ty,
                    optional: false,
                },
            );
        }
        for (key, ity) in indexers {
            let value = self.reify(&ity);
            table.indexers.push((key, value));
        }
        if !array.is_empty() {
            let elems: Vec<Ty> = array.iter().map(|i| self.reify(i)).collect();
            table.array = Some(Ty::union(elems));
        }
        self.reify_stack.pop();
        let ty = Ty::Table(Box::new(table));
        if self.pass == 1 {
            self.memo.insert(id, ty.clone());
        }
        ty
    }

    /// The shape a lookup delegates to via the metatable's `__index`, when
    /// it is a tracked table.
    fn index_delegate(&self, shape: usize) -> Option<usize> {
        let meta = self.shapes[shape].metatable?;
        match self.shapes[meta].fields.get("__index") {
            Some(ITy::Shape(next)) => Some(*next),
            _ => None,
        }
    }

    // --- field lookup ------------------------------------------------------

    /// Look a named field up on a receiver, following the `__index` chain.
    fn lookup_field(&mut self, recv: &ITy, name: &str) -> Lookup {
        match recv {
            ITy::Shape(id) => self.lookup_shape_field(*id, name),
            ITy::Ty(ty) => self.lookup_ty_field(&ty.clone(), name),
            ITy::Union(members) => {
                let members = members.clone();
                let mut found: Vec<ITy> = Vec::new();
                for member in &members {
                    match self.lookup_field(member, name) {
                        Lookup::Found(ity) => found.push(ity),
                        _ => return Lookup::Opaque,
                    }
                }
                Lookup::Found(ity_union(found))
            }
            ITy::Func(_) => Lookup::Opaque,
        }
    }

    fn lookup_shape_field(&mut self, id: usize, name: &str) -> Lookup {
        let mut provable = true;
        // The `---@class` the receiver resolved to (the outermost declared
        // shape in the chain), for the luals `undefined-field` message (#90).
        let mut declared_class: Option<String> = None;
        let mut cur = Some(id);
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            if let Some(class) = self.shapes[s].declared.clone() {
                // Declared carriers are governed by their declaration:
                // declared fields resolve at their DECLARED types (an
                // inferred constructor value never shadows the declaration
                // — `self.side` is `integer` when the struct says so, #73),
                // inferred extensions and carrier methods fill in the rest.
                // A field the declaration, its parent chain, the carrier's
                // methods, and inferred extensions all lack is a genuine
                // undefined-field read (#90) — provable UNLESS the class
                // declares an indexer / array part (dynamic access is
                // declared, so any string key is admissible).
                if let Some(shape) = self.env.class_shape(&class) {
                    if let Some(field) = shape.fields.get(name) {
                        let ty = if field.optional {
                            field.ty.clone().optional()
                        } else {
                            field.ty.clone()
                        };
                        return Lookup::Found(ITy::Ty(ty));
                    }
                    // Absence is diagnosable only for a real LuaCATS `---@class`
                    // with no indexer/array part. A dynamic-access class stays
                    // lenient.
                    if self.env.is_class(&class)
                        && shape.indexers.is_empty()
                        && shape.array.is_none()
                    {
                        declared_class.get_or_insert(class);
                    } else {
                        provable = false;
                    }
                } else {
                    // An alias / non-resolvable declared name: stay lenient.
                    provable = false;
                }
            }
            if let Some(ity) = self.shapes[s].fields.get(name) {
                return Lookup::Found(ity.clone());
            }
            let data = &self.shapes[s];
            if data.escaped || data.meta_unknown || !data.indexers.is_empty() {
                provable = false;
            }
            if let Some(meta) = self.shapes[s].metatable {
                match self.shapes[meta].fields.get("__index") {
                    Some(ITy::Shape(next)) => {
                        cur = Some(*next);
                        continue;
                    }
                    Some(_) => {
                        // `__index` is a function or untracked value.
                        provable = false;
                    }
                    None => {
                        if self.shapes[meta].escaped || self.shapes[meta].meta_unknown {
                            provable = false;
                        }
                    }
                }
            }
            cur = None;
        }
        Lookup::Absent {
            provable,
            declared: declared_class,
        }
    }

    fn lookup_ty_field(&mut self, ty: &Ty, name: &str) -> Lookup {
        match ty {
            Ty::Table(table) => {
                if let Some(field) = table.fields.get(name) {
                    let ty = if field.optional {
                        field.ty.clone().optional()
                    } else {
                        field.ty.clone()
                    };
                    return Lookup::Found(ITy::Ty(ty));
                }
                let key = Ty::StringLit(name.to_string());
                for (k, v) in &table.indexers {
                    if crate::assign::assignable(self.env, false, &key, k) {
                        return Lookup::Found(ITy::Ty(v.clone()));
                    }
                }
                // A plain structural table (inferred, or a `---@type {...}`
                // literal shape) stays lenient: un-annotated code invents no
                // undefined-field obligation (#90). Only a *named* class does.
                Lookup::Absent {
                    provable: false,
                    declared: None,
                }
            }
            Ty::Named(class) => {
                let Some(resolved) = self.env.resolve_named(class) else {
                    return Lookup::Opaque;
                };
                if let Lookup::Found(ity) = self.lookup_ty_field(&resolved, name) {
                    return Lookup::Found(ity);
                }
                // An annotated instance (`---@return Circle`, `---@type
                // Circle`) still resolves methods and inferred extensions
                // through the declared carrier's shared instance shape (#73).
                if let Some(&carrier) = self.declared_carriers.get(class.as_str()) {
                    let instance = self.instance_of(carrier);
                    if let Lookup::Found(ity) = self.lookup_shape_field(instance, name) {
                        return Lookup::Found(ity);
                    }
                }
                // Absent on a declared class → luals `undefined-field` (#90),
                // provable only for a real LuaCATS `---@class` with no
                // indexer/array part (dynamic access) and that resolved to a
                // table (not an enum union).
                let dynamic = match &resolved {
                    Ty::Table(t) => !t.indexers.is_empty() || t.array.is_some(),
                    _ => true,
                };
                let provable = self.env.is_class(class) && !dynamic;
                Lookup::Absent {
                    provable,
                    declared: provable.then(|| class.clone()),
                }
            }
            Ty::Union(members) => {
                let mut found: Vec<ITy> = Vec::new();
                for member in members.clone() {
                    match self.lookup_ty_field(&member, name) {
                        Lookup::Found(ity) => found.push(ity),
                        _ => return Lookup::Opaque,
                    }
                }
                Lookup::Found(ity_union(found))
            }
            _ => Lookup::Opaque,
        }
    }

    /// The element type produced by indexing with an integer (`t[i]`,
    /// `ipairs` values).
    fn elem_ty(&mut self, recv: &ITy) -> ITy {
        match recv {
            ITy::Shape(id) => {
                let mut parts: Vec<ITy> = self.shapes[*id].array.clone();
                for (key, value) in self.shapes[*id].indexers.clone() {
                    if integerish(&key) {
                        parts.push(value);
                    }
                }
                ity_union(parts)
            }
            ITy::Ty(Ty::Table(table)) => {
                let mut parts: Vec<ITy> = Vec::new();
                if let Some(elem) = &table.array {
                    parts.push(ITy::Ty(elem.clone()));
                }
                for (key, value) in &table.indexers {
                    if integerish(key) || matches!(key, Ty::Any) {
                        parts.push(ITy::Ty(value.clone()));
                    }
                }
                ity_union(parts)
            }
            ITy::Ty(Ty::Named(class)) => match self.env.resolve_named(class) {
                Some(resolved) => self.elem_ty(&ITy::Ty(resolved)),
                None => ITy::unknown(),
            },
            ITy::Union(members) => {
                let members = members.clone();
                ity_union(members.iter().map(|m| self.elem_ty(m)).collect())
            }
            _ => ITy::unknown(),
        }
    }

    /// `(key, value)` unions for `pairs`/`next` iteration.
    fn pairs_tys(&mut self, recv: &ITy) -> (ITy, ITy) {
        match recv {
            ITy::Shape(id) => {
                let data = &self.shapes[*id];
                let mut keys: Vec<ITy> = Vec::new();
                let mut values: Vec<ITy> = data.fields.values().cloned().collect();
                if !data.fields.is_empty() {
                    keys.push(ITy::Ty(Ty::String));
                }
                if !data.array.is_empty() {
                    keys.push(ITy::Ty(Ty::Integer));
                    values.extend(data.array.iter().cloned());
                }
                for (key, value) in data.indexers.clone() {
                    keys.push(ITy::Ty(key));
                    values.push(value);
                }
                (ity_union(keys), ity_union(values))
            }
            ITy::Ty(Ty::Table(table)) => {
                let mut keys: Vec<ITy> = Vec::new();
                let mut values: Vec<ITy> = Vec::new();
                if !table.fields.is_empty() {
                    keys.push(ITy::Ty(Ty::String));
                    values.extend(table.fields.values().map(|f| ITy::Ty(f.ty.clone())));
                }
                if let Some(elem) = &table.array {
                    keys.push(ITy::Ty(Ty::Integer));
                    values.push(ITy::Ty(elem.clone()));
                }
                for (key, value) in &table.indexers {
                    keys.push(ITy::Ty(key.clone()));
                    values.push(ITy::Ty(value.clone()));
                }
                (ity_union(keys), ity_union(values))
            }
            ITy::Ty(Ty::Named(class)) => match self.env.resolve_named(class) {
                Some(resolved) => self.pairs_tys(&ITy::Ty(resolved)),
                None => (ITy::unknown(), ITy::unknown()),
            },
            ITy::Union(members) => {
                let members = members.clone();
                let mut keys = Vec::new();
                let mut values = Vec::new();
                for member in &members {
                    let (k, v) = self.pairs_tys(member);
                    keys.push(k);
                    values.push(v);
                }
                (ity_union(keys), ity_union(values))
            }
            _ => (ITy::unknown(), ITy::unknown()),
        }
    }

    // --- bodies & blocks ---------------------------------------------------

    /// Walk one function body (or the chunk): bind parameters, reset the
    /// inferred returns, walk the block.
    fn walk_body(&mut self, body: BodyId, sig: Option<&FunctionTy>, self_ty: Option<&ITy>) {
        {
            let data = self.funcs.entry(body).or_default();
            data.sig = sig.cloned();
            data.returns.clear();
            data.returns_set = false;
            data.in_progress = true;
        }
        let params = self.body(body).params.clone();
        for &param in &params {
            let binding = self.binding(param);
            let ity = if binding.kind == BindingKind::SelfParam {
                // An explicit `---@param self T` is authoritative; otherwise
                // the metatable-inferred instance type (the constructor tie).
                if let Some(p) = sig.and_then(|s| s.params.iter().find(|p| p.name == "self")) {
                    let ty = if p.optional {
                        p.ty.clone().optional()
                    } else {
                        p.ty.clone()
                    };
                    ITy::Ty(ty)
                } else {
                    self_ty.cloned().unwrap_or_else(ITy::unknown)
                }
            } else if let Some(p) = sig.and_then(|s| {
                let name = &self.binding(param).name;
                s.params.iter().find(|p| &p.name == name)
            }) {
                let ty = if p.optional {
                    p.ty.clone().optional()
                } else {
                    p.ty.clone()
                };
                ITy::Ty(ty)
            } else if let Some(seed) = self.ctx_param_seeds.get(&param).cloned() {
                // Contextual (bidirectional) typing (#120): this unannotated
                // parameter's type comes from the expected `fun(...)` at the
                // lambda's position (call argument / `---@type fun` target).
                // Annotation-derived, so it feeds the checker (it is consulted
                // whether or not `seed_params` is on). The lambda's own
                // `---@param` (the `sig` branch above) wins — an annotated
                // parameter is never recorded here.
                seed
            } else if self.seed_params {
                // Call-site inference: the union of argument types the
                // previous pass observed for this parameter.
                self.param_seeds
                    .get(&param)
                    .cloned()
                    .unwrap_or_else(ITy::unknown)
            } else {
                ITy::unknown()
            };
            self.state.insert(param, ity);
        }
        let block = self.body(body).block.clone();
        self.walk_block(body, &block);
        if let Some(data) = self.funcs.get_mut(&body) {
            data.in_progress = false;
        }
    }

    fn walk_block(&mut self, body: BodyId, block: &Block) {
        for &stmt in &block.stmts {
            self.walk_stmt(body, stmt);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn walk_stmt(&mut self, body: BodyId, stmt: StmtId) {
        self.apply_casts(body, stmt);
        match self.body(body).stmt(stmt).clone() {
            Stmt::Local { names, init } => self.walk_local(body, stmt, &names, &init),
            Stmt::LocalFunction { binding, func } => {
                let sig = self
                    .stmt_range(body, stmt)
                    .and_then(|key| self.env.fn_sig(key))
                    .cloned();
                let fn_body = match self.body(body).expr(func) {
                    Expr::Function(b) => Some(*b),
                    _ => None,
                };
                if let Some(fn_body) = fn_body {
                    // Bind before the walk so recursive calls resolve.
                    self.state.insert(binding, ITy::Func(fn_body));
                    if let Some(sig) = &sig {
                        self.state
                            .insert(binding, ITy::Ty(Ty::Function(Box::new(sig.clone()))));
                        self.declared.insert(binding);
                    }
                    // Cross-file seeding by name (covers locals re-exported
                    // via `return { f = f }`).
                    if let Some(externals) = self.externals
                        && let Some(seeds) = externals
                            .fn_param_seeds
                            .get(&self.binding(binding).name)
                            .cloned()
                    {
                        let itys: Vec<ITy> = seeds.into_iter().map(ITy::Ty).collect();
                        self.record_arg_seeds(fn_body, &itys, false);
                    }
                    self.walk_body(fn_body, sig.as_ref(), None);
                    if sig.is_none() {
                        self.state.insert(binding, ITy::Func(fn_body));
                    }
                }
            }
            Stmt::Assign { targets, values } => self.walk_assign(body, stmt, &targets, &values),
            Stmt::ExprStmt(expr) => {
                self.eval(body, expr);
            }
            Stmt::Return(exprs) => {
                // Contextual typing of return expressions (#120 follow-up):
                // the enclosing function's declared `---@return` types seed
                // returned function/table literals before they are walked, the
                // same way `---@type` seeds an initializer — luals types a
                // returned node against the function's return `infer`
                // (`script/vm/compiler.lua`). Fires only for a function with a
                // declared signature; a contextually-typed lambda (no declared
                // sig of its own) has its returns seeded transitively by
                // `seed_returns` at the point its parameters are seeded.
                if let Some(returns) = self
                    .funcs
                    .get(&body)
                    .and_then(|d| d.sig.as_ref().map(|s| s.returns.clone()))
                {
                    for (i, &e) in exprs.iter().enumerate() {
                        if let Some(exp) = returns.get(i) {
                            let exp = exp.clone();
                            self.seed_contextual(body, e, &exp);
                        }
                    }
                }
                let values = self.eval_values(body, &exprs, None);
                let data = self.funcs.entry(body).or_default();
                for (i, value) in values.into_iter().enumerate() {
                    if i < data.returns.len() {
                        let merged = ity_union(vec![data.returns[i].clone(), value]);
                        data.returns[i] = merged;
                    } else if data.returns_set {
                        // Prior returns were shorter: this slot may be nil.
                        data.returns.push(ity_union(vec![value, ITy::Ty(Ty::Nil)]));
                    } else {
                        data.returns.push(value);
                    }
                }
                data.returns_set = true;
            }
            Stmt::If {
                branches,
                else_block,
            } => self.walk_if(body, &branches, else_block.as_ref()),
            Stmt::While { cond, body: block } => {
                self.eval(body, cond);
                let entry = self.state.clone();
                self.apply_narrows(body, cond, true);
                self.walk_block(body, &block);
                let out = std::mem::take(&mut self.state);
                self.merge_states(vec![out, entry]);
            }
            Stmt::Repeat { body: block, cond } => {
                self.walk_block(body, &block);
                self.eval(body, cond);
            }
            Stmt::NumericFor {
                var,
                start,
                end,
                step,
                body: block,
            } => {
                let start_ty = self.eval(body, start);
                self.eval(body, end);
                let step_ty = step.map(|s| self.eval(body, s));
                let var_ty = numeric_for_var(&start_ty, step_ty.as_ref());
                let entry = self.state.clone();
                self.state.insert(var, var_ty);
                self.walk_block(body, &block);
                let out = std::mem::take(&mut self.state);
                self.merge_states(vec![out, entry]);
            }
            Stmt::GenericFor {
                vars,
                exprs,
                body: block,
            } => {
                for &expr in &exprs {
                    self.eval(body, expr);
                }
                let var_tys = self.iteration_tys(body, &exprs, vars.len());
                let entry = self.state.clone();
                for (i, &var) in vars.iter().enumerate() {
                    let ity = var_tys.get(i).cloned().unwrap_or_else(ITy::unknown);
                    self.state.insert(var, ity);
                }
                self.walk_block(body, &block);
                let out = std::mem::take(&mut self.state);
                self.merge_states(vec![out, entry]);
            }
            Stmt::Do { body: block } => self.walk_block(body, &block),
            Stmt::Break | Stmt::Goto { .. } | Stmt::Label { .. } | Stmt::Error => {}
        }
    }

    fn walk_local(
        &mut self,
        body: BodyId,
        stmt: StmtId,
        names: &[luabox_hir::LocalBinding],
        init: &[ExprId],
    ) {
        let key = self.stmt_range(body, stmt);
        let declared_tys: Option<Vec<Ty>> = key
            .and_then(|k| self.env.typed_local(k))
            .map(<[Ty]>::to_vec);
        let sig = key.and_then(|k| self.env.fn_sig(k)).cloned();

        // `---@param`/`---@return`-annotated `local f = function() ... end`.
        let fn_init = init.first().and_then(|&e| match self.body(body).expr(e) {
            Expr::Function(b) => Some((*b, e)),
            _ => None,
        });
        let values: Vec<ITy> = if let (Some(sig), Some((fn_body, _))) = (&sig, fn_init) {
            self.walk_body(fn_body, Some(sig), None);
            vec![ITy::Ty(Ty::Function(Box::new(sig.clone())))]
        } else {
            // Contextual typing (#120 + follow-ups): a `---@type T` on the
            // local seeds its initializer against `T` before the value is
            // walked (the `sig` branch above already covers a
            // `---@param`-annotated initializer, whose annotation wins). A
            // `fun(...)` target types a function-literal initializer's
            // parameters; a `---@class`/table target types a table-literal
            // initializer's function-valued fields and nested table fields. A
            // non-matching `---@type` leaves the initializer untyped.
            if let Some(tys) = declared_tys.as_deref() {
                for (i, ty) in tys.iter().enumerate() {
                    if let Some(&e) = init.get(i) {
                        let ty = ty.clone();
                        self.seed_contextual(body, e, &ty);
                    }
                }
            }
            self.eval_values(body, init, Some(names.len()))
        };

        // A `---@type T` carrier — `local X = {}` whose single object
        // annotation is currently unsatisfied *only* by missing members (a
        // carrier still being built). Keep the inferred shape rather than
        // freezing the binding to `T`, so later `X.f = ...` / `function X:m()`
        // extend it; its final shape is published for the checker's deferred
        // whole-carrier conformance check.
        let is_carrier = key.is_some_and(|k| {
            self.classify_carrier(body, k, names, init, declared_tys.as_deref(), &values)
        });

        for (i, local) in names.iter().enumerate() {
            if is_carrier && i == 0 {
                let ity = values.first().cloned().unwrap_or(ITy::Ty(Ty::Nil));
                self.state.insert(local.binding, ity);
                if let Some(k) = key {
                    self.carrier_locals.insert(k, local.binding);
                }
            } else if let Some(ty) = declared_tys.as_ref().and_then(|t| t.get(i)) {
                self.declared.insert(local.binding);
                self.state.insert(local.binding, ITy::Ty(ty.clone()));
            } else if i == 0 && sig.is_some() {
                self.declared.insert(local.binding);
                #[expect(
                    clippy::expect_used,
                    reason = "this arm is guarded by `sig.is_some()`, so the clone is always Some"
                )]
                self.state.insert(
                    local.binding,
                    ITy::Ty(Ty::Function(Box::new(sig.clone().expect("sig present")))),
                );
            } else {
                let ity = values.get(i).cloned().unwrap_or(ITy::Ty(Ty::Nil));
                self.state.insert(local.binding, ity);
            }
        }

        // `---@class` / `---@struct` bound to this local: associate the
        // carrier shape with its declaration.
        if let (Some(key), Some(local)) = (key, names.first())
            && let Some(name) = self.env.declared_target(key)
        {
            let name = name.to_string();
            if let Some(ITy::Shape(id)) = self.state.get(&local.binding) {
                let id = *id;
                self.shapes[id].declared = Some(name.clone());
                self.declared_carriers.insert(name.clone(), id);
                // A `---@class Name : Parent` carrier: record it so its final
                // reified shape can be checked for `: Interface` conformance
                // (#107). Parentless classes carry no obligation, so the
                // checker skips them; recording them here is harmless.
                self.class_carrier_locals.insert(key, (local.binding, name));
            }
        }
    }

    /// Whether a `local X = {…}` is a `---@type T` carrier (single name,
    /// single object annotation, table-constructor init whose immediate
    /// conformance to `T` fails *only* by missing members). Pass 0 computes
    /// and records the decision in [`Self::carrier_keys`]; pass 1 replays it
    /// verbatim so the keep-the-shape choice is identical across passes and
    /// no partial reification poisons the shape memo.
    fn classify_carrier(
        &mut self,
        body: BodyId,
        key: Key,
        names: &[luabox_hir::LocalBinding],
        init: &[ExprId],
        declared_tys: Option<&[Ty]>,
        values: &[ITy],
    ) -> bool {
        if self.pass == 1 {
            return self.carrier_keys.contains(&key);
        }
        let carrier = names.len() == 1
            && init.len() == 1
            && matches!(self.body(body).expr(init[0]), Expr::Table { .. })
            && match (declared_tys, values.first()) {
                (Some([target]), Some(&ITy::Shape(id))) => {
                    let target = target.clone();
                    match self.reify_shape(id) {
                        Ty::Table(lit) => matches!(
                            crate::assign::classify_literal(self.env, self.strict, &lit, &target),
                            Some(crate::assign::LiteralConformance::MissingOnly)
                        ),
                        _ => false,
                    }
                }
                _ => false,
            };
        if carrier {
            self.carrier_keys.insert(key);
        }
        carrier
    }

    fn walk_assign(&mut self, body: BodyId, stmt: StmtId, targets: &[ExprId], values: &[ExprId]) {
        // Desugared method/function declaration: `function T:m() ... end`
        // becomes `T.m = function(self) ... end`. The carrier must be known
        // *before* the body walks so `self` gets the instance shape.
        if let ([target], [value]) = (targets, values)
            && let Expr::Function(fn_body) = self.body(body).expr(*value)
        {
            let fn_body = *fn_body;
            let sig = self
                .stmt_range(body, stmt)
                .and_then(|key| self.env.fn_sig(key))
                .cloned();
            let resolved = self.resolve_target(body, *target);
            let takes_self = self
                .body(fn_body)
                .params
                .first()
                .is_some_and(|&p| self.binding(p).kind == BindingKind::SelfParam);
            let self_ty = match (&resolved, takes_self) {
                (Target::Field { shape: Some(c), .. }, true) => {
                    let instance = self.instance_of(*c);
                    Some(ITy::Shape(instance))
                }
                _ => None,
            };
            // Cross-file seeding: dependent files' observed call args for
            // this function's name, merged before the body walks.
            if let Some(externals) = self.externals {
                let exported_name = match &resolved {
                    Target::Field { name, .. } | Target::Global(name) => Some(name.as_str()),
                    _ => None,
                };
                if let Some(seeds) = exported_name.and_then(|n| externals.fn_param_seeds.get(n)) {
                    let itys: Vec<ITy> = seeds.iter().cloned().map(ITy::Ty).collect();
                    self.record_arg_seeds(fn_body, &itys, takes_self);
                }
            }
            // Track the enclosing `---@class` while the method body walks, so
            // an access to a restricted member of that class resolves as
            // "inside the class" (luals `getEnvClass`, #115). Covers both `:`
            // methods and `.` functions carried by a declared class.
            let enclosing = match &resolved {
                Target::Field { shape: Some(c), .. } => self.shape_declared_class(*c),
                _ => None,
            };
            if let Some(class) = &enclosing {
                self.class_ctx.push(class.clone());
            }
            self.walk_body(fn_body, sig.as_ref(), self_ty.as_ref());
            if enclosing.is_some() {
                self.class_ctx.pop();
            }
            let ity = match sig {
                Some(sig) => ITy::Ty(Ty::Function(Box::new(sig))),
                None => ITy::Func(fn_body),
            };
            self.assign_into(&resolved, ity);
            return;
        }

        let resolved: Vec<Target> = targets
            .iter()
            .map(|&t| self.resolve_target(body, t))
            .collect();
        let values = self.eval_values(body, values, Some(targets.len()));
        for (target, value) in resolved.into_iter().zip(values) {
            self.assign_into(&target, value);
        }
    }

    /// Resolve an assignment target without treating it as a read (writes
    /// extend shapes; they never diagnose absent fields).
    fn resolve_target(&mut self, body: BodyId, target: ExprId) -> Target {
        match self.body(body).expr(target).clone() {
            Expr::Name(name) => match self.resolution(body, target) {
                Some(Resolution::Local(id)) => Target::Binding {
                    id: *id,
                    upvalue: false,
                },
                Some(Resolution::Upvalue { binding, .. }) => Target::Binding {
                    id: *binding,
                    upvalue: true,
                },
                Some(Resolution::Global(_)) | None => Target::Global(name),
            },
            Expr::Index { base, index, .. } => {
                let base_ity = self.eval(body, base);
                let shape = match base_ity {
                    ITy::Shape(id) => Some(id),
                    _ => None,
                };
                match self.body(body).expr(index) {
                    Expr::Literal(Literal::String(s)) => match s.as_str() {
                        Some(name) => Target::Field {
                            shape,
                            name: name.to_string(),
                        },
                        None => Target::Indexer {
                            shape,
                            key: Ty::String,
                        },
                    },
                    Expr::Literal(Literal::Number(_)) => Target::ArrayElem { shape },
                    _ => {
                        let key = self.eval(body, index);
                        let key = generalize_key(&self.reify(&key));
                        Target::Indexer { shape, key }
                    }
                }
            }
            _ => Target::Opaque,
        }
    }

    fn assign_into(&mut self, target: &Target, value: ITy) {
        match target {
            Target::Binding { id, upvalue } => {
                if self.declared.contains(id) {
                    return; // annotations are authoritative
                }
                if *upvalue {
                    let merged = match self.state.get(id) {
                        Some(existing) => ity_union(vec![existing.clone(), value]),
                        None => value,
                    };
                    self.state.insert(*id, merged);
                } else {
                    self.state.insert(*id, value);
                }
            }
            Target::Global(name) => {
                let merged = match self.globals.get(name) {
                    Some(existing) => ity_union(vec![existing.clone(), value]),
                    None => value,
                };
                self.globals.insert(name.clone(), merged);
            }
            Target::Field { shape, name } => match shape {
                Some(id) => self.extend_field(*id, name, value),
                None => self.mark_escaped(&value),
            },
            Target::ArrayElem { shape } => match shape {
                Some(id) => self.extend_array(*id, value),
                None => self.mark_escaped(&value),
            },
            Target::Indexer { shape, key } => match shape {
                Some(id) => self.extend_indexer(*id, key.clone(), value),
                None => self.mark_escaped(&value),
            },
            Target::Opaque => self.mark_escaped(&value),
        }
    }

    // --- `---@cast` / inline `--[[@as T]]` overrides -------------------------

    /// Apply any `---@cast var T` annotations attached to this statement
    /// before walking it (LuaLS semantics: the override holds from this
    /// point in the flow, even for annotated bindings).
    fn apply_casts(&mut self, body: BodyId, stmt: StmtId) {
        let Some(key) = self.stmt_range(body, stmt) else {
            return;
        };
        let env = self.env;
        let Some(casts) = env.casts_at(key) else {
            return;
        };
        for cast in casts {
            let target = self.cast_target(body, stmt, &cast.var);
            let current = match &target {
                CastTarget::Binding(id) => self.state.get(id).cloned(),
                CastTarget::Global(name) => self.globals.get(name).cloned(),
            };
            let mut ity = current.unwrap_or_else(ITy::unknown);
            for (kind, ty) in &cast.ops {
                ity = match kind {
                    luacats::CastKind::Replace => ITy::Ty(ty.clone()),
                    luacats::CastKind::Add => ity_union(vec![ity, ITy::Ty(ty.clone())]),
                    luacats::CastKind::Remove => remove_cast_member(&ity, ty),
                };
            }
            match target {
                CastTarget::Binding(id) => {
                    self.state.insert(id, ity);
                }
                CastTarget::Global(name) => {
                    self.globals.insert(name, ity);
                }
            }
        }
    }

    /// Resolve the variable a `---@cast` names: a resolved use inside the
    /// annotated statement when one exists (precise), otherwise the most
    /// recently declared binding of that name (approximate), otherwise a
    /// global.
    fn cast_target(&self, body: BodyId, stmt: StmtId, var: &str) -> CastTarget {
        if let Some(id) = self.find_name_in_stmt(body, stmt, var) {
            return CastTarget::Binding(id);
        }
        let best = self
            .state
            .keys()
            .filter(|id| self.binding(**id).name == var)
            .max_by_key(|id| self.binding(**id).range.start());
        match best {
            Some(&id) => CastTarget::Binding(id),
            None => CastTarget::Global(var.to_string()),
        }
    }

    /// The inline `--[[@as T]]` cast anchored to this expression, if any.
    fn as_override(&self, body: BodyId, expr: ExprId) -> Option<Ty> {
        let (_, end) = self.expr_range(body, expr)?;
        self.env.as_cast_at(end).cloned()
    }

    fn find_name_in_stmt(&self, body: BodyId, stmt: StmtId, var: &str) -> Option<BindingId> {
        match self.body(body).stmt(stmt) {
            Stmt::Local { init, .. } => self.find_name_in_exprs(body, init, var),
            Stmt::LocalFunction { func, .. } => self.find_name_in_expr(body, *func, var),
            Stmt::Assign { targets, values } => self
                .find_name_in_exprs(body, targets, var)
                .or_else(|| self.find_name_in_exprs(body, values, var)),
            Stmt::ExprStmt(e) => self.find_name_in_expr(body, *e, var),
            Stmt::Return(exprs) => self.find_name_in_exprs(body, exprs, var),
            Stmt::If {
                branches,
                else_block,
            } => branches
                .iter()
                .find_map(|b| {
                    self.find_name_in_expr(body, b.cond, var)
                        .or_else(|| self.find_name_in_block(body, &b.block, var))
                })
                .or_else(|| {
                    else_block
                        .as_ref()
                        .and_then(|b| self.find_name_in_block(body, b, var))
                }),
            Stmt::While { cond, body: block } => self
                .find_name_in_expr(body, *cond, var)
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::Repeat { body: block, cond } => self
                .find_name_in_block(body, block, var)
                .or_else(|| self.find_name_in_expr(body, *cond, var)),
            Stmt::NumericFor {
                start,
                end,
                step,
                body: block,
                ..
            } => self
                .find_name_in_expr(body, *start, var)
                .or_else(|| self.find_name_in_expr(body, *end, var))
                .or_else(|| step.and_then(|s| self.find_name_in_expr(body, s, var)))
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::GenericFor {
                exprs, body: block, ..
            } => self
                .find_name_in_exprs(body, exprs, var)
                .or_else(|| self.find_name_in_block(body, block, var)),
            Stmt::Do { body: block } => self.find_name_in_block(body, block, var),
            Stmt::Break | Stmt::Goto { .. } | Stmt::Label { .. } | Stmt::Error => None,
        }
    }

    fn find_name_in_block(&self, body: BodyId, block: &Block, var: &str) -> Option<BindingId> {
        block
            .stmts
            .iter()
            .find_map(|&s| self.find_name_in_stmt(body, s, var))
    }

    fn find_name_in_exprs(&self, body: BodyId, exprs: &[ExprId], var: &str) -> Option<BindingId> {
        exprs
            .iter()
            .find_map(|&e| self.find_name_in_expr(body, e, var))
    }

    fn find_name_in_expr(&self, body: BodyId, expr: ExprId, var: &str) -> Option<BindingId> {
        match self.body(body).expr(expr) {
            Expr::Name(name) if name == var => self.name_binding(body, expr),
            Expr::Name(_) | Expr::Literal(_) | Expr::Vararg | Expr::Error => None,
            Expr::Index { base, index, .. } => self
                .find_name_in_expr(body, *base, var)
                .or_else(|| self.find_name_in_expr(body, *index, var)),
            Expr::Call { callee, args } => self
                .find_name_in_expr(body, *callee, var)
                .or_else(|| self.find_name_in_exprs(body, args, var)),
            Expr::MethodCall { receiver, args, .. } => self
                .find_name_in_expr(body, *receiver, var)
                .or_else(|| self.find_name_in_exprs(body, args, var)),
            Expr::Function(fn_body) => {
                // An upvalue use inside a closure resolves to the same
                // binding — still a precise hit.
                let fn_body = *fn_body;
                let block = &self.body(fn_body).block;
                self.find_name_in_block(fn_body, block, var)
            }
            Expr::Table { entries } => entries.iter().find_map(|entry| match entry {
                TableEntry::Positional(v) => self.find_name_in_expr(body, *v, var),
                TableEntry::Named { value, .. } => self.find_name_in_expr(body, *value, var),
                TableEntry::Keyed { key, value } => self
                    .find_name_in_expr(body, *key, var)
                    .or_else(|| self.find_name_in_expr(body, *value, var)),
            }),
            Expr::Binary { lhs, rhs, .. } => self
                .find_name_in_expr(body, *lhs, var)
                .or_else(|| self.find_name_in_expr(body, *rhs, var)),
            Expr::Unary { operand, .. } => self.find_name_in_expr(body, *operand, var),
            Expr::Truncate(inner) => self.find_name_in_expr(body, *inner, var),
        }
    }

    // --- branching & narrowing ---------------------------------------------

    fn walk_if(&mut self, body: BodyId, branches: &[IfBranch], else_block: Option<&Block>) {
        let mut outs: Vec<HashMap<BindingId, ITy>> = Vec::new();
        // The running "no branch so far was taken" state.
        let mut fallthrough = self.state.clone();
        for branch in branches {
            self.state = fallthrough.clone();
            self.eval(body, branch.cond);
            self.apply_narrows(body, branch.cond, true);
            self.walk_block(body, &branch.block);
            if !self.block_terminates(body, &branch.block) {
                outs.push(std::mem::take(&mut self.state));
            }
            // Later arms (and the code after the `if`) know this
            // condition was false.
            self.state = fallthrough;
            self.apply_narrows(body, branch.cond, false);
            fallthrough = std::mem::take(&mut self.state);
        }
        if let Some(block) = else_block {
            self.state = fallthrough;
            self.walk_block(body, block);
            if !self.block_terminates(body, block) {
                outs.push(std::mem::take(&mut self.state));
            }
        } else {
            outs.push(fallthrough);
        }
        self.merge_states(outs);
    }

    /// Union-merge branch-exit states into `self.state`.
    fn merge_states(&mut self, outs: Vec<HashMap<BindingId, ITy>>) {
        let mut iter = outs.into_iter();
        let Some(mut merged) = iter.next() else {
            // Every path terminated: keep the entry state (dead code after).
            return;
        };
        for out in iter {
            for (id, ity) in out {
                match merged.get(&id) {
                    Some(existing) => {
                        let union = ity_union(vec![existing.clone(), ity]);
                        merged.insert(id, union);
                    }
                    None => {
                        merged.insert(id, ity);
                    }
                }
            }
        }
        self.state = merged;
    }

    fn block_terminates(&self, body: BodyId, block: &Block) -> bool {
        block.stmts.last().is_some_and(|&stmt| {
            matches!(
                self.body(body).stmt(stmt),
                Stmt::Return(_) | Stmt::Break | Stmt::Goto { .. }
            )
        })
    }

    fn apply_narrows(&mut self, body: BodyId, cond: ExprId, positive: bool) {
        let mut preds: Vec<(BindingId, Pred)> = Vec::new();
        self.cond_narrows(body, cond, positive, &mut preds);
        for (binding, pred) in preds {
            if let Some(current) = self.state.get(&binding) {
                let narrowed = self.narrow(&current.clone(), &pred);
                self.state.insert(binding, narrowed);
            }
        }
    }

    /// Derive narrowing predicates from a condition (`positive` = the
    /// branch where the condition held).
    fn cond_narrows(
        &self,
        body: BodyId,
        cond: ExprId,
        positive: bool,
        out: &mut Vec<(BindingId, Pred)>,
    ) {
        match self.body(body).expr(cond) {
            Expr::Name(_) => {
                if let Some(binding) = self.name_binding(body, cond) {
                    out.push((binding, if positive { Pred::Truthy } else { Pred::Falsy }));
                }
            }
            Expr::Truncate(inner) => self.cond_narrows(body, *inner, positive, out),
            Expr::Unary {
                op: UnOp::Not,
                operand,
            } => self.cond_narrows(body, *operand, !positive, out),
            Expr::Binary { op, lhs, rhs } => match op {
                BinOp::And if positive => {
                    self.cond_narrows(body, *lhs, true, out);
                    self.cond_narrows(body, *rhs, true, out);
                }
                BinOp::Or if !positive => {
                    self.cond_narrows(body, *lhs, false, out);
                    self.cond_narrows(body, *rhs, false, out);
                }
                BinOp::Eq | BinOp::Ne => {
                    let holds = (*op == BinOp::Eq) == positive;
                    self.eq_narrows(body, *lhs, *rhs, holds, out);
                    self.eq_narrows(body, *rhs, *lhs, holds, out);
                }
                _ => {}
            },
            _ => {}
        }
    }

    /// Narrowing from `subject ==/~= probe` where `probe` is a literal, or
    /// `subject` is a `type(x)` call compared to a type-name string.
    fn eq_narrows(
        &self,
        body: BodyId,
        subject: ExprId,
        probe: ExprId,
        holds: bool,
        out: &mut Vec<(BindingId, Pred)>,
    ) {
        // `type(x) == "string"`.
        if let Expr::Call { callee, args } = self.body(body).expr(subject)
            && matches!(self.body(body).expr(*callee), Expr::Name(n) if n == "type")
            && matches!(self.resolution(body, *callee), Some(Resolution::Global(_)))
            && let [arg] = args[..]
            && let Some(binding) = self.name_binding(body, arg)
            && let Expr::Literal(Literal::String(s)) = self.body(body).expr(probe)
            && let Some(name) = s.as_str().and_then(type_name)
        {
            out.push((
                binding,
                if holds {
                    Pred::TypeIs(name)
                } else {
                    Pred::NotTypeIs(name)
                },
            ));
            return;
        }
        // `x == nil` / `x == <literal>`.
        if let Some(binding) = self.name_binding(body, subject)
            && let Expr::Literal(lit) = self.body(body).expr(probe)
        {
            let pred = match (lit, holds) {
                (Literal::Nil, true) => Pred::Nil,
                (Literal::Nil, false) => Pred::NonNil,
                (other, true) => Pred::Lit(literal_ty(other)),
                (other, false) => Pred::NotLit(literal_ty(other)),
            };
            out.push((binding, pred));
        }
    }

    fn name_binding(&self, body: BodyId, expr: ExprId) -> Option<BindingId> {
        if !matches!(self.body(body).expr(expr), Expr::Name(_)) {
            return None;
        }
        match self.resolution(body, expr) {
            Some(Resolution::Local(id)) => Some(*id),
            Some(Resolution::Upvalue { binding, .. }) => Some(*binding),
            _ => None,
        }
    }

    /// Apply a predicate to an inference type (union-filtering).
    fn narrow(&self, ity: &ITy, pred: &Pred) -> ITy {
        let members = ity_members(ity);
        let mut kept: Vec<ITy> = Vec::new();
        for member in members {
            match pred {
                Pred::Truthy => match &member {
                    ITy::Ty(Ty::Nil | Ty::BoolLit(false)) => {}
                    ITy::Ty(Ty::Boolean) => kept.push(ITy::Ty(Ty::BoolLit(true))),
                    ITy::Ty(Ty::Union(inner)) => {
                        let inner: Vec<Ty> = inner
                            .iter()
                            .filter(|t| !matches!(t, Ty::Nil | Ty::BoolLit(false)))
                            .cloned()
                            .collect();
                        if !inner.is_empty() {
                            kept.push(ITy::Ty(Ty::union(inner)));
                        }
                    }
                    _ => kept.push(member),
                },
                Pred::Falsy => match &member {
                    ITy::Ty(Ty::Nil | Ty::BoolLit(false)) => kept.push(member),
                    ITy::Ty(Ty::Boolean) => kept.push(ITy::Ty(Ty::BoolLit(false))),
                    ITy::Ty(Ty::Unknown | Ty::Any) => {
                        kept.push(ITy::Ty(Ty::union(vec![Ty::Nil, Ty::BoolLit(false)])));
                    }
                    ITy::Ty(Ty::Union(inner)) => {
                        if inner.contains(&Ty::Nil) {
                            kept.push(ITy::Ty(Ty::Nil));
                        }
                        if inner.contains(&Ty::BoolLit(false)) || inner.contains(&Ty::Boolean) {
                            kept.push(ITy::Ty(Ty::BoolLit(false)));
                        }
                    }
                    _ => {}
                },
                Pred::Nil => match &member {
                    ITy::Ty(Ty::Nil | Ty::Unknown | Ty::Any) => kept.push(ITy::Ty(Ty::Nil)),
                    ITy::Ty(Ty::Union(inner)) if inner.contains(&Ty::Nil) => {
                        kept.push(ITy::Ty(Ty::Nil));
                    }
                    _ => {}
                },
                Pred::NonNil => match &member {
                    ITy::Ty(Ty::Nil) => {}
                    ITy::Ty(Ty::Union(inner)) => {
                        let inner: Vec<Ty> = inner
                            .iter()
                            .filter(|t| !matches!(t, Ty::Nil))
                            .cloned()
                            .collect();
                        if !inner.is_empty() {
                            kept.push(ITy::Ty(Ty::union(inner)));
                        }
                    }
                    _ => kept.push(member),
                },
                Pred::TypeIs(name) => {
                    if let Some(narrowed) = narrow_type_is(&member, name) {
                        kept.push(narrowed);
                    }
                }
                Pred::NotTypeIs(name) => {
                    if narrow_type_is(&member, name).is_none() || member.is_unknown() {
                        kept.push(member);
                    }
                }
                Pred::Lit(lit) => match &member {
                    ITy::Ty(Ty::Unknown | Ty::Any) => kept.push(ITy::Ty(lit.clone())),
                    ITy::Ty(ty) => {
                        if crate::assign::assignable(self.env, false, lit, ty) {
                            kept.push(ITy::Ty(lit.clone()));
                        }
                    }
                    _ => {}
                },
                Pred::NotLit(lit) => {
                    if member != ITy::Ty(lit.clone()) {
                        kept.push(member);
                    }
                }
            }
        }
        if kept.is_empty() {
            // The branch is (statically) impossible; degrade gracefully.
            match pred {
                Pred::Nil => ITy::Ty(Ty::Nil),
                Pred::TypeIs(name) => ITy::Ty(type_base(name)),
                _ => ITy::unknown(),
            }
        } else {
            ity_union(kept)
        }
    }

    // --- expressions ---------------------------------------------------------

    /// Evaluate an expression to its (single-value) inference type,
    /// publishing the reified type for the annotation checker.
    fn eval(&mut self, body: BodyId, expr: ExprId) -> ITy {
        let mut ity = self.eval_inner(body, expr);
        if let Some(ty) = self.as_override(body, expr) {
            // Inline `--[[@as T]]`: an authoritative override.
            ity = ITy::Ty(ty);
        }
        if self.pass == 1
            && !ity.is_unknown()
            && !matches!(
                self.body(body).expr(expr),
                Expr::Literal(_) | Expr::Table { .. } | Expr::Function(_) | Expr::Error
            )
            && let Some(key) = self.expr_range(body, expr)
        {
            let ty = self.reify(&ity);
            if ty != Ty::Unknown {
                self.expr_types.insert(key, ty);
            }
        }
        ity
    }

    #[allow(clippy::too_many_lines)]
    fn eval_inner(&mut self, body: BodyId, expr: ExprId) -> ITy {
        match self.body(body).expr(expr).clone() {
            Expr::Literal(lit) => ITy::Ty(literal_ty(&lit)),
            Expr::Name(name) => match self.resolution(body, expr) {
                Some(Resolution::Local(id)) => {
                    self.state.get(id).cloned().unwrap_or_else(ITy::unknown)
                }
                Some(Resolution::Upvalue { binding, .. }) => self
                    .state
                    .get(binding)
                    .cloned()
                    .unwrap_or_else(ITy::unknown),
                Some(Resolution::Global(_)) | None => match self.globals.get(&name) {
                    Some(ity) => ity.clone(),
                    None => match self.env.function(&name) {
                        Some(sig) => ITy::Ty(Ty::Function(Box::new(sig.clone()))),
                        None => match self.env.global_type(&name) {
                            Some(ty) => ITy::Ty(ty.clone()),
                            None => ITy::unknown(),
                        },
                    },
                },
            },
            Expr::Index { base, index, .. } => self.eval_index(body, expr, base, index),
            Expr::Call { .. } => {
                let (returns, open) = self.eval_call(body, expr);
                first_value(&returns, open)
            }
            Expr::MethodCall { .. } => {
                let (returns, open) = self.eval_method_call(body, expr);
                first_value(&returns, open)
            }
            Expr::Function(fn_body) => {
                self.walk_body(fn_body, None, None);
                ITy::Func(fn_body)
            }
            Expr::Table { entries } => self.eval_table(body, expr, &entries),
            Expr::Binary { op, lhs, rhs } => self.eval_binary(body, op, lhs, rhs),
            Expr::Unary { op, operand } => {
                let operand_ty = self.eval(body, operand);
                match op {
                    UnOp::Not => ITy::Ty(Ty::Boolean),
                    UnOp::Len => {
                        // `#x` is `integer` unless the operand's class declares
                        // `---@operator len: T`, in which case it takes `T`
                        // (luals parity, #114).
                        let ty = self.reify(&operand_ty);
                        self.operator_result(&ty, "len", None)
                            .map_or_else(|| ITy::Ty(Ty::Integer), ITy::Ty)
                    }
                    UnOp::Neg => {
                        let ty = self.reify(&operand_ty);
                        if integerish(&ty) {
                            ITy::Ty(Ty::Integer)
                        } else if numberish(&ty) {
                            ITy::Ty(Ty::Number)
                        } else {
                            self.operator_result(&ty, "unm", None)
                                .map_or_else(ITy::unknown, ITy::Ty)
                        }
                    }
                    UnOp::BNot => {
                        let ty = self.reify(&operand_ty);
                        if numberish(&ty) {
                            ITy::Ty(Ty::Integer)
                        } else {
                            self.operator_result(&ty, "bnot", None)
                                .map_or_else(ITy::unknown, ITy::Ty)
                        }
                    }
                }
            }
            Expr::Truncate(inner) => self.eval(body, inner),
            Expr::Vararg | Expr::Error => ITy::unknown(),
        }
    }

    fn eval_index(&mut self, body: BodyId, expr: ExprId, base: ExprId, index: ExprId) -> ITy {
        let recv = self.eval(body, base);
        match self.body(body).expr(index).clone() {
            Expr::Literal(Literal::String(s)) => match s.as_str() {
                Some(name) => {
                    let name = name.to_string();
                    match self.lookup_field(&recv, &name) {
                        Lookup::Found(ity) => {
                            if let Some(class) = self.receiver_class(&recv) {
                                self.check_visibility(body, expr, &class, &name);
                            }
                            ity
                        }
                        Lookup::Absent { provable, declared } => {
                            // A global-rooted dotted read of a *declared module
                            // table* (`string.rep`, `table.insert`, a
                            // version-gated `string.pack`) is a library-member
                            // access, not an undefined field: a module's members
                            // register as dotted registry functions, not
                            // `---@field`s, so they are absent from the module
                            // class's shape whether or not they are declared.
                            // Resolve the function when known, else treat the
                            // member as an unknown module value — never a
                            // diagnostic. The #90 undefined-field rule targets
                            // typed *values* (locals, params, `self`), handled
                            // in the `else` arm.
                            if declared.is_some()
                                && let Some(dotted) = self.dotted_callee(body, expr)
                            {
                                match self.env.function(&dotted) {
                                    Some(sig) => ITy::Ty(Ty::Function(Box::new(sig.clone()))),
                                    None => ITy::unknown(),
                                }
                            } else {
                                if provable {
                                    self.report_absent(body, expr, &name, declared.as_deref());
                                }
                                ITy::unknown()
                            }
                        }
                        Lookup::Opaque => ITy::unknown(),
                    }
                }
                None => ITy::unknown(),
            },
            Expr::Literal(Literal::Number(num)) => {
                // A fixed-position tuple (`---@type [string, number]`): `t[1]`
                // reads back the type at that position; past the end is lenient
                // (#86). Non-tuple receivers fall through to array/indexer.
                if let luabox_hir::Number::Int(idx) = num
                    && let Some(ty) = tuple_index(&recv, idx)
                {
                    ty
                } else {
                    self.elem_ty(&recv)
                }
            }
            _ => {
                let key = self.eval(body, index);
                let key_ty = self.reify(&key);
                if numberish(&key_ty) {
                    self.elem_ty(&recv)
                } else if let Ty::StringLit(name) = &key_ty {
                    // A dynamic expression that is nonetheless a known
                    // string literal narrows like `t.name`.
                    match self.lookup_field(&recv, name) {
                        Lookup::Found(ity) => ity,
                        _ => ITy::unknown(),
                    }
                } else {
                    // Dynamic key: fall back to indexer/value unions —
                    // never `any`.
                    let (_, values) = self.pairs_tys(&recv);
                    values
                }
            }
        }
    }

    fn eval_table(&mut self, body: BodyId, expr: ExprId, entries: &[TableEntry]) -> ITy {
        let shape = self.alloc_shape(body, expr);
        for entry in entries {
            match entry {
                TableEntry::Positional(value) => {
                    // A trailing multi-value producer contributes an
                    // unknown number of elements of its first-value type,
                    // which lands in the same array part anyway.
                    let ity = self.eval(body, *value);
                    self.extend_array(shape, ity);
                }
                TableEntry::Named { name, value } => {
                    let ity = self.eval(body, *value);
                    self.extend_field(shape, name, ity);
                }
                TableEntry::Keyed { key, value } => {
                    let key_expr = *key;
                    let value_ity = self.eval(body, *value);
                    match self.body(body).expr(key_expr).clone() {
                        Expr::Literal(Literal::String(s)) => match s.as_str() {
                            Some(name) => {
                                let name = name.to_string();
                                self.extend_field(shape, &name, value_ity);
                            }
                            None => self.extend_indexer(shape, Ty::String, value_ity),
                        },
                        Expr::Literal(Literal::Number(_)) => self.extend_array(shape, value_ity),
                        _ => {
                            let dynamic = self.eval(body, key_expr);
                            let general = generalize_key(&self.reify(&dynamic));
                            self.extend_indexer(shape, general, value_ity);
                        }
                    }
                }
            }
        }
        ITy::Shape(shape)
    }

    fn eval_binary(&mut self, body: BodyId, op: BinOp, lhs: ExprId, rhs: ExprId) -> ITy {
        let l = self.eval(body, lhs);
        let r = self.eval(body, rhs);
        match op {
            BinOp::And => {
                let falsy = self.narrow(&l, &Pred::Falsy);
                ity_union(vec![falsy, r])
            }
            BinOp::Or => {
                let truthy = self.narrow(&l, &Pred::Truthy);
                ity_union(vec![truthy, r])
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                ITy::Ty(Ty::Boolean)
            }
            BinOp::Concat => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if stringish(&lt) && stringish(&rt) {
                    ITy::Ty(Ty::String)
                } else {
                    self.operator_binary(op, &lt, &rt)
                }
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Mod | BinOp::IDiv => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if integerish(&lt) && integerish(&rt) {
                    ITy::Ty(Ty::Integer)
                } else if numberish(&lt) && numberish(&rt) {
                    ITy::Ty(Ty::Number)
                } else {
                    self.operator_binary(op, &lt, &rt)
                }
            }
            BinOp::Div | BinOp::Pow => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if numberish(&lt) && numberish(&rt) {
                    ITy::Ty(Ty::Number)
                } else {
                    self.operator_binary(op, &lt, &rt)
                }
            }
            BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if numberish(&lt) && numberish(&rt) {
                    ITy::Ty(Ty::Integer)
                } else {
                    self.operator_binary(op, &lt, &rt)
                }
            }
        }
    }

    /// Apply a `---@operator` overload to a binary expression whose operands
    /// are not primitive numbers/strings (#114). Mirrors Lua's metamethod
    /// dispatch: the LEFT operand's class is consulted first, then the RIGHT
    /// operand's (the reversed case — e.g. `scalar * Vec` resolving through
    /// `Vec`'s `mul` overload). The declared result type replaces what would
    /// otherwise be `unknown`; with no matching operator the behavior is
    /// unchanged (`unknown`). luals's exact right-operand rule is not
    /// separately documented, so this follows Lua's runtime left-then-right
    /// metamethod order.
    fn operator_binary(&self, op: BinOp, lt: &Ty, rt: &Ty) -> ITy {
        let Some(name) = binop_operator_name(op) else {
            return ITy::unknown();
        };
        self.operator_result(lt, name, Some(rt))
            .or_else(|| self.operator_result(rt, name, Some(lt)))
            .map_or_else(ITy::unknown, ITy::Ty)
    }

    /// The result type of `op` on a class operand, if the (possibly inherited)
    /// class declares a matching `---@operator`. For a binary operator `other`
    /// is the type of the opposite operand and the first overload whose
    /// declared parameter accepts it wins (first-match, mirroring `---@overload`
    /// selection, #86). For a unary operator `other` is `None` and the
    /// no-parameter overload is used.
    fn operator_result(&self, ty: &Ty, op: &str, other: Option<&Ty>) -> Option<Ty> {
        let Ty::Named(class) = ty else {
            return None;
        };
        for sig in self.env.class_operators(class, op) {
            match (&sig.input, other) {
                (Some(input), Some(other)) => {
                    if crate::assign::assignable(self.env, false, other, input) {
                        return Some(sig.result.clone());
                    }
                }
                (None, None) => return Some(sig.result.clone()),
                _ => {}
            }
        }
        None
    }

    // --- calls -----------------------------------------------------------

    /// Evaluate a call, returning its full (positional) return types and
    /// whether the list is open-ended.
    fn eval_call(&mut self, body: BodyId, expr: ExprId) -> (Vec<ITy>, bool) {
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
                    crate::assign::assignable(self.env, self.strict, arg, input)
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
            if !crate::assign::assignable(self.env, self.strict, arg, &expected) {
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
    fn dotted_callee(&self, body: BodyId, callee: ExprId) -> Option<String> {
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
    fn record_arg_seeds(&mut self, fn_body: BodyId, args: &[ITy], skip_self: bool) {
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
    fn seed_contextual(&mut self, body: BodyId, expr: ExprId, expected: &Ty) {
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
    fn collect_fn_returns(&mut self) -> Vec<InferredReturn> {
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

    fn eval_method_call(&mut self, body: BodyId, expr: ExprId) -> (Vec<ITy>, bool) {
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
                // checker can argument-check the `:` call (#118). Gated on:
                //  - the receiver resolving to a declared `---@class` — a plain
                //    inferred table's method carries no checkable contract;
                //  - the resolved member being an *annotated* function value
                //    (a `---@field m fun(...)` or a `function C:m` carrying a
                //    `---@param`/`---@return`/`---@deprecated` signature). An
                //    unannotated method reifies to a `fun` with `unknown`
                //    parameters and would manufacture arity errors, so — exactly
                //    as an unannotated free function is never arity-checked — it
                //    is left unpublished (mandatory conservatism).
                // Only pass 1's resolution is published, matching `expr_types`.
                if self.pass == 1
                    && recv_class.is_some()
                    && let ITy::Ty(Ty::Function(sig)) = &f
                    && let Some(key) = self.expr_range(body, expr)
                {
                    self.method_sigs.insert(key, (**sig).clone());
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
    fn eval_values(&mut self, body: BodyId, exprs: &[ExprId], want: Option<usize>) -> Vec<ITy> {
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
    fn iteration_tys(&mut self, body: BodyId, exprs: &[ExprId], nvars: usize) -> Vec<ITy> {
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

// === helpers ===

/// `---@cast x -T`: remove `T` from the union (exact member match; a
/// `Ty`-level union inside a single member is filtered member-wise).
fn remove_cast_member(ity: &ITy, ty: &Ty) -> ITy {
    let mut kept: Vec<ITy> = Vec::new();
    for member in ity_members(ity) {
        match member {
            ITy::Ty(Ty::Union(inner)) => {
                let inner: Vec<Ty> = inner.iter().filter(|t| *t != ty).cloned().collect();
                if !inner.is_empty() {
                    kept.push(ITy::Ty(Ty::union(inner)));
                }
            }
            ITy::Ty(ref t) if t == ty => {}
            other => kept.push(other),
        }
    }
    if kept.is_empty() {
        ITy::unknown()
    } else {
        ity_union(kept)
    }
}

fn first_value(rets: &[ITy], open: bool) -> ITy {
    match rets.first() {
        Some(first) => first.clone(),
        None if open => ITy::unknown(),
        None => ITy::Ty(Ty::Nil),
    }
}

/// The literal type of a HIR literal (decoded values re-rendered as
/// literal-type text).
fn literal_ty(lit: &Literal) -> Ty {
    match lit {
        Literal::Nil => Ty::Nil,
        Literal::Bool(b) => Ty::BoolLit(*b),
        Literal::Number(n) => Ty::NumberLit(render_number(n)),
        Literal::String(s) => match s.as_str() {
            Some(text) => Ty::StringLit(text.to_string()),
            None => Ty::String,
        },
    }
}

/// Render a decoded number as literal-type text. Floats always keep a
/// decimal point (or exponent) so integral-float values (`2.0`) do not
/// masquerade as integer literals.
fn render_number(n: &luabox_hir::Number) -> String {
    match n {
        luabox_hir::Number::Int(v) | luabox_hir::Number::I64(v) => v.to_string(),
        luabox_hir::Number::U64(v) => v.to_string(),
        luabox_hir::Number::Float(v) | luabox_hir::Number::Imaginary(v) => {
            let text = format!("{v}");
            if text.contains('.')
                || text.contains('e')
                || text.contains("inf")
                || text.contains("NaN")
            {
                text
            } else {
                format!("{text}.0")
            }
        }
    }
}

/// A fixed-position tuple read: the type at integer position `idx` (1-based)
/// of a tuple-typed receiver (a table whose integer positions are modeled as
/// `NumberLit` indexers, #86). `Some(unknown)` when the receiver is a tuple
/// but `idx` is past its end (lenient, luals-style); `None` when the receiver
/// is not a tuple (ordinary array/indexer read).
fn tuple_index(recv: &ITy, idx: i64) -> Option<ITy> {
    let ITy::Ty(Ty::Table(table)) = recv else {
        return None;
    };
    let mut is_tuple = false;
    for (key, value) in &table.indexers {
        let Ty::NumberLit(n) = key else { continue };
        if !crate::assign::is_integral_literal(n) {
            continue;
        }
        is_tuple = true;
        if n.parse::<i64>().ok() == Some(idx) {
            return Some(ITy::Ty(value.clone()));
        }
    }
    is_tuple.then(ITy::unknown)
}

/// The `---@operator` name a binary operator dispatches to (`+` → `add`,
/// `..` → `concat`, ...), matching luals's operator vocabulary (#114).
/// Logical (`and`/`or`) and comparison operators have no overload and return
/// `None`.
fn binop_operator_name(op: BinOp) -> Option<&'static str> {
    Some(match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Pow => "pow",
        BinOp::IDiv => "idiv",
        BinOp::Concat => "concat",
        BinOp::BAnd => "band",
        BinOp::BOr => "bor",
        BinOp::BXor => "bxor",
        BinOp::Shl => "shl",
        BinOp::Shr => "shr",
        BinOp::And
        | BinOp::Or
        | BinOp::Eq
        | BinOp::Ne
        | BinOp::Lt
        | BinOp::Le
        | BinOp::Gt
        | BinOp::Ge => return None,
    })
}

fn integerish(ty: &Ty) -> bool {
    match ty {
        Ty::Integer => true,
        Ty::NumberLit(text) => crate::assign::is_integral_literal(text),
        _ => false,
    }
}

fn numberish(ty: &Ty) -> bool {
    matches!(ty, Ty::Number | Ty::Integer | Ty::NumberLit(_))
}

fn stringish(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::StringLit(_)) || numberish(ty)
}

/// Generalize a key type for indexer entries (literals widen to their
/// base so indexer lists stay small).
fn generalize_key(ty: &Ty) -> Ty {
    match ty {
        Ty::StringLit(_) => Ty::String,
        Ty::NumberLit(text) => {
            if crate::assign::is_integral_literal(text) {
                Ty::Integer
            } else {
                Ty::Number
            }
        }
        Ty::BoolLit(_) => Ty::Boolean,
        Ty::Unknown | Ty::Any => Ty::Any,
        other => other.clone(),
    }
}

/// The control variable type of a numeric `for`: `integer` when the start
/// and step are provably integral, `number` otherwise.
fn numeric_for_var(start: &ITy, step: Option<&ITy>) -> ITy {
    let int = |ity: &ITy| matches!(ity, ITy::Ty(ty) if integerish(ty));
    if int(start) && step.is_none_or(int) {
        ITy::Ty(Ty::Integer)
    } else {
        ITy::Ty(Ty::Number)
    }
}

/// Recognized `type()` result strings.
fn type_name(s: &str) -> Option<&'static str> {
    match s {
        "nil" => Some("nil"),
        "boolean" => Some("boolean"),
        "number" => Some("number"),
        "string" => Some("string"),
        "table" => Some("table"),
        "function" => Some("function"),
        "userdata" => Some("userdata"),
        "thread" => Some("thread"),
        _ => None,
    }
}

/// The base type a `type(x) == "name"` narrowing produces from `unknown`.
fn type_base(name: &str) -> Ty {
    match name {
        "nil" => Ty::Nil,
        "boolean" => Ty::Boolean,
        "number" => Ty::Number,
        "string" => Ty::String,
        "table" => Ty::any_table(),
        "function" => Ty::Function(Box::new(FunctionTy::opaque())),
        _ => Ty::Unknown,
    }
}

/// Narrow one union member under `type(x) == name`; `None` = filtered out.
fn narrow_type_is(member: &ITy, name: &str) -> Option<ITy> {
    let keep = |cond: bool| if cond { Some(member.clone()) } else { None };
    match member {
        ITy::Ty(Ty::Unknown | Ty::Any) => Some(ITy::Ty(type_base(name))),
        ITy::Shape(_) => keep(name == "table"),
        ITy::Func(_) => keep(name == "function"),
        ITy::Union(_) => None, // members are pre-flattened
        ITy::Ty(ty) => {
            let matches = match name {
                "nil" => matches!(ty, Ty::Nil),
                "boolean" => matches!(ty, Ty::Boolean | Ty::BoolLit(_)),
                "number" => numberish(ty),
                "string" => matches!(ty, Ty::String | Ty::StringLit(_)),
                "table" => matches!(ty, Ty::Table(_) | Ty::Named(_)),
                "function" => matches!(ty, Ty::Function(_)),
                _ => false,
            };
            keep(matches)
        }
    }
}

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]
mod tests {
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;
    use crate::{Strictness, check_file};

    fn outcome(source: &str) -> Outcome {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let env = TypeEnv::build(&parsed);
        let lowered = luabox_hir::lower(&parsed);
        run(&lowered, &env, "test.lua", true, false, None)
    }

    /// Like [`outcome`], with call-site parameter seeding on (the
    /// display-mode inference behind inlay hints).
    fn display_outcome(source: &str) -> Outcome {
        display_outcome_ext(source, None)
    }

    /// Display-mode inference with cross-file inputs.
    fn display_outcome_ext(source: &str, externals: Option<&ExternalTypes>) -> Outcome {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let env = TypeEnv::build(&parsed);
        let lowered = luabox_hir::lower(&parsed);
        run(&lowered, &env, "test.lua", true, true, externals)
    }

    fn binding_ty(outcome: &Outcome, name: &str) -> Ty {
        outcome
            .binding_types
            .iter()
            .find(|b| b.name == name)
            .map_or_else(|| panic!("no binding named `{name}`"), |b| b.ty.clone())
    }

    fn codes(source: &str, strictness: Strictness) -> Vec<String> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        check_file(&parsed, "test.lua", strictness, Dialect::Lua54)
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }

    fn strict_codes(source: &str) -> Vec<String> {
        codes(source, Strictness::Strict)
    }

    /// Callee fixtures shared by the flow tests.
    const WANTS: &str = "\
---@param n number
local function wantn(n) end
---@param s string
local function wants(s) end
";

    // --- constructor shapes -------------------------------------------

    #[test]
    fn constructor_builds_per_field_shape() {
        let out = outcome("local point = { x = 1, y = 2 }\n");
        let Ty::Table(table) = binding_ty(&out, "point") else {
            panic!("expected a structural table");
        };
        assert_eq!(table.fields["x"].ty.to_string(), "1");
        assert_eq!(table.fields["y"].ty.to_string(), "2");
    }

    #[test]
    fn assignments_extend_the_shape() {
        let out = outcome("local t = {}\nt.x = 1\nt.y = \"s\"\n");
        let Ty::Table(table) = binding_ty(&out, "t") else {
            panic!("expected a structural table");
        };
        assert!(table.fields.contains_key("x"));
        assert_eq!(table.fields["y"].ty.to_string(), "\"s\"");
    }

    #[test]
    fn array_hash_and_mixed_parts_distinguished() {
        let src = "\
local arr = { 1, 2 }
local mixed = { \"a\", flag = true }
local keyed = { [\"x\"] = 1, [2] = \"e\" }
";
        let out = outcome(src);
        let Ty::Table(arr) = binding_ty(&out, "arr") else {
            panic!("array");
        };
        assert!(arr.array.is_some() && arr.fields.is_empty());
        let Ty::Table(mixed) = binding_ty(&out, "mixed") else {
            panic!("mixed");
        };
        assert!(mixed.array.is_some() && mixed.fields.contains_key("flag"));
        // Literal-keyed entries narrow like named/positional ones.
        let Ty::Table(keyed) = binding_ty(&out, "keyed") else {
            panic!("keyed");
        };
        assert!(keyed.fields.contains_key("x") && keyed.array.is_some());
    }

    #[test]
    fn dynamic_keys_become_indexers_never_any() {
        let out = outcome("local d = {}\nd[SOME_KEY] = 1\n");
        let Ty::Table(table) = binding_ty(&out, "d") else {
            panic!("expected a structural table");
        };
        assert_eq!(table.indexers.len(), 1);
        assert_eq!(table.indexers[0].1.to_string(), "1");
    }

    // --- unannotated OOP end to end ------------------------------------

    const CIRCLE: &str = "\
local Circle = {}
Circle.__index = Circle

function Circle.new(radius)
  local o = setmetatable({}, Circle)
  o.radius = radius or 0
  return o
end

function Circle:area()
  return self.radius * self.radius
end

local c = Circle.new(2)
local a = c:area()
";

    #[test]
    fn unannotated_oop_types_end_to_end() {
        assert_eq!(strict_codes(CIRCLE), Vec::<String>::new());
        let out = outcome(CIRCLE);
        // The instance shape resolves constructor fields AND methods
        // through the `Class.__index = Class` metatable chain.
        let c_ty = binding_ty(&out, "c");
        let Ty::Table(instance) = &c_ty else {
            panic!("instance must be structural, got {c_ty}");
        };
        assert!(instance.fields.contains_key("radius"), "{instance:?}");
        assert!(instance.fields.contains_key("area"), "{instance:?}");
        // `self.radius * self.radius` with `radius or 0` infers integer.
        assert_eq!(binding_ty(&out, "a").to_string(), "integer");
    }

    #[test]
    fn oop_field_typo_is_lb0306() {
        let src = CIRCLE.replace(
            "return self.radius * self.radius",
            "return self.radiuss * 2",
        );
        assert_eq!(strict_codes(&src), vec!["LB0306"]);
        // Warn mode downgrades to a warning.
        let parsed = parse(&src, Dialect::Lua54);
        let diags = check_file(&parsed, "test.lua", Strictness::Warn, Dialect::Lua54);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, luabox_diag::Severity::Warning);
    }

    #[test]
    fn oop_method_typo_is_lb0306() {
        let src = format!("{CIRCLE}local bad = c:aera()\n");
        assert_eq!(strict_codes(&src), vec!["LB0306"]);
    }

    #[test]
    fn inheritance_chain_resolves_through_index_delegation() {
        let src = "\
---@param s string
local function wants(s) end

local Base = {}
Base.__index = Base
function Base:name()
  return \"base\"
end

local Child = setmetatable({}, { __index = Base })
Child.__index = Child
function Child.new()
  return setmetatable({}, Child)
end

local c = Child.new()
wants(c:name())
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn escaped_tables_never_report_absent_fields() {
        let src = "\
local t = {}
some_unknown_function(t)
local v = t.anything
return v
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    #[test]
    fn self_read_of_undeclared_field_is_undefined_field() {
        // `self.size` types from the class (a declared field); `self.whatever`
        // is declared nowhere on `Thing` — reading it is luals'
        // `undefined-field` (#90), now enforced on the strictness ladder.
        let src = format!(
            "{WANTS}\
---@class Thing
---@field size number
local Thing = {{}}
Thing.__index = Thing

function Thing:grow()
  wantn(self.size)
  local x = self.whatever
  return x
end
"
        );
        let diags = check_file(
            &parse(&src, Dialect::Lua54),
            "test.lua",
            Strictness::Strict,
            Dialect::Lua54,
        );
        assert_eq!(
            diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
            vec!["LB0306"]
        );
        assert!(
            diags[0].message.contains("`whatever`") && diags[0].message.contains("`Thing`"),
            "message names field and class: {}",
            diags[0].message
        );
    }

    // --- iteration ------------------------------------------------------

    #[test]
    fn ipairs_iteration_is_typed_from_the_array_part() {
        let src = format!(
            "{WANTS}\
local xs = {{ 1, 2, 3 }}
for i, v in ipairs(xs) do
  wantn(i)
  wantn(v)
  wants(v)
end
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn pairs_iteration_is_typed_from_fields() {
        let src = format!(
            "{WANTS}\
local cfg = {{ host = \"x\", port = 80 }}
for k, v in pairs(cfg) do
  wants(k)
  wantn(v)
end
"
        );
        // `v` is `"x"|80`: not (always) a number — exactly one mismatch.
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn pairs_on_annotated_map_uses_the_indexer() {
        let src = format!(
            "{WANTS}\
---@type table<string, number>
local scores = {{}}
for k, v in pairs(scores) do
  wants(k)
  wantn(v)
end
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn next_style_iteration_is_typed() {
        let src = format!(
            "{WANTS}\
local flags = {{ on = true }}
for k in next, flags do
  wants(k)
end
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn numeric_for_var_is_integer_for_integral_bounds() {
        let src = "\
---@param i integer
local function wanti(i) end
for i = 1, 10 do
  wanti(i)
end
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- indexing --------------------------------------------------------

    #[test]
    fn literal_string_index_equals_field_access() {
        let src = format!(
            "{WANTS}\
local t = {{ x = 1 }}
wantn(t[\"x\"])
wants(t[\"x\"])
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn dynamic_key_reads_hit_indexer_types_not_any() {
        let src = format!(
            "{WANTS}\
local d = {{}}
d[SOME_KEY] = 1
wants(d[OTHER_KEY])
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    // --- narrowing --------------------------------------------------------

    #[test]
    fn truthiness_and_nil_checks_narrow_optionals() {
        let src = format!(
            "{WANTS}\
---@type number|nil
local x = 1
if x ~= nil then
  wantn(x)
end
if x then
  wantn(x)
end
wantn(x)
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn type_call_narrows_unknown() {
        let src = format!(
            "{WANTS}\
local u = SOME_GLOBAL
if type(u) == \"number\" then
  wantn(u)
end
if type(u) == \"string\" then
  wants(u)
end
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn nil_check_else_branch_narrows() {
        let src = format!(
            "{WANTS}\
---@type number|nil
local x = 1
if x == nil then
  local unused = 0
else
  wantn(x)
end
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn early_return_narrows_the_rest_of_the_body() {
        let src = format!(
            "{WANTS}\
---@type number|nil
local x = 1
local function g()
  if x == nil then
    return
  end
  wantn(x)
end
g()
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn branch_join_unions_assignments() {
        let src = format!(
            "{WANTS}\
local v
if SOME_COND then
  v = 1
else
  v = \"s\"
end
wantn(v)
"
        );
        // `v` is `1|"s"` at the join — not a number.
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    // --- `---@cast` and inline `--[[@as T]]` overrides (#73) ---------------

    #[test]
    fn cast_replaces_the_flow_type() {
        let src = format!(
            "{WANTS}\
---@type string|nil
local x = nil
---@cast x string
wants(x)
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
        // A cast to the WRONG type still errors downstream.
        let bad = format!(
            "{WANTS}\
---@type string|nil
local x = nil
---@cast x number
wants(x)
"
        );
        assert_eq!(strict_codes(&bad), vec!["LB0300"]);
    }

    #[test]
    fn cast_minus_removes_a_union_member() {
        let src = format!(
            "{WANTS}\
---@type number|nil
local x = 1
---@cast x -nil
wantn(x)
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn cast_plus_adds_a_union_member() {
        let src = format!(
            "{WANTS}\
---@type number
local x = 1
---@cast x +nil
wantn(x)
"
        );
        // `x` is now `number|nil`: not (always) a number.
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn inline_as_overrides_the_expression_type() {
        let src = format!(
            "{WANTS}\
local u = SOME_GLOBAL --[[@as number]]
wantn(u)
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
        // The override is authoritative — a wrong consumer still errors.
        let bad = format!(
            "{WANTS}\
local u = SOME_GLOBAL --[[@as number]]
wants(u)
"
        );
        assert_eq!(strict_codes(&bad), vec!["LB0300"]);
    }

    #[test]
    fn inline_as_applies_to_call_results() {
        let src = format!(
            "{WANTS}\
local function opaque()
  return SOME_GLOBAL
end
local v = opaque() --[[@as string]]
wants(v)
wantn(v)
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn inline_as_in_argument_position() {
        let src = format!(
            "{WANTS}\
wantn(SOME_GLOBAL --[[@as number]])
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    // --- functions, multi-return, Truncate --------------------------------

    #[test]
    fn multi_return_assigns_positionally() {
        let src = format!(
            "{WANTS}\
local function pair()
  return 1, \"s\"
end
local a, b = pair()
wantn(a)
wants(b)
wantn(b)
"
        );
        assert_eq!(strict_codes(&src), vec!["LB0300"]);
    }

    #[test]
    fn truncate_takes_the_first_value() {
        let src = format!(
            "{WANTS}\
local function pair()
  return 1, \"s\"
end
local t = (pair())
wantn(t)
"
        );
        assert_eq!(strict_codes(&src), Vec::<String>::new());
    }

    #[test]
    fn inferred_return_flows_into_annotated_call() {
        let src = "\
---@param n number
local function f(n) end
local function name()
  return \"x\"
end
f(name())
";
        // Previously invisible in warn mode (unknown flows freely); the
        // inferred `"x"` return now surfaces the real mismatch.
        assert_eq!(codes(src, Strictness::Warn), vec!["LB0300"]);
        assert_eq!(strict_codes(src), vec!["LB0300"]);
    }

    #[test]
    fn annotated_signatures_stay_authoritative() {
        let src = "\
---@param n number
---@return string
local function f(n)
  return \"ok\"
end
---@param s string
local function g(s) end
g(f(1))
";
        assert_eq!(strict_codes(src), Vec::<String>::new());
    }

    // --- the hard requirement, executable ---------------------------------

    /// SPEC.md §3: a bare/opaque `table` type never results from ANY
    /// locally-constructed table. Walk every inferred binding type in a
    /// broad fixture and assert structural-ness.
    #[test]
    fn locally_constructed_tables_are_never_bare_table() {
        let src = "\
local empty = {}
local point = { x = 1, y = 2 }
local arr = { 1, 2, 3 }
local mixed = { \"a\", flag = true }
local nested = { inner = { deep = \"v\" } }
local dyn = {}
dyn[SOME_KEY] = 1
local Klass = {}
Klass.__index = Klass
function Klass.new()
  return setmetatable({}, Klass)
end
function Klass:tag()
  self.tagged = true
  return self
end
local inst = Klass.new()
local chained = inst:tag()
local via_call = (function()
  return { r = 1 }
end)()
local alias = point
local reassigned = {}
reassigned = { z = 3 }
for _, item in ipairs({ { id = 1 } }) do
  local inner = item
  print(inner)
end
";
        let out = outcome(src);
        let table_bindings = [
            "empty",
            "point",
            "arr",
            "mixed",
            "nested",
            "dyn",
            "Klass",
            "inst",
            "chained",
            "via_call",
            "alias",
            "reassigned",
            "item",
            "inner",
        ];
        for name in table_bindings {
            let ty = binding_ty(&out, name);
            assert!(
                matches!(ty, Ty::Table(_)),
                "`{name}` degraded to a non-structural type: {ty}"
            );
            assert_ne!(
                ty.to_string(),
                "table",
                "`{name}` degraded to the opaque catch-all table type"
            );
        }
        // And no binding of any kind reifies to the opaque catch-all.
        for b in &out.binding_types {
            assert_ne!(
                b.ty.to_string(),
                "table",
                "binding `{}` is bare table",
                b.name
            );
        }
    }

    // --- display mode: call-site parameter seeding + return surface -------

    #[test]
    fn display_mode_seeds_params_from_call_sites() {
        let src = "\
local function area(w, h)
  local result = w * h
  return result
end
local a = area(3, 4)
";
        let out = display_outcome(src);
        assert_eq!(binding_ty(&out, "w").to_string(), "integer");
        assert_eq!(binding_ty(&out, "h").to_string(), "integer");
        assert_eq!(binding_ty(&out, "result").to_string(), "integer");
        assert_eq!(binding_ty(&out, "a").to_string(), "integer");
    }

    #[test]
    fn display_mode_unions_multiple_call_sites() {
        let src = "\
local function id(x)
  return x
end
id(1)
id(\"s\")
";
        let out = display_outcome(src);
        assert_eq!(binding_ty(&out, "x").to_string(), "integer|string");
    }

    #[test]
    fn display_mode_seeds_method_args_past_self() {
        let src = "\
local Greeter = {}
Greeter.__index = Greeter

function Greeter:greet(name)
  return name
end

local g = setmetatable({}, Greeter)
g:greet(\"world\")
";
        let out = display_outcome(src);
        assert_eq!(binding_ty(&out, "name").to_string(), "string");
    }

    #[test]
    fn display_mode_flows_constructor_args_into_self_fields() {
        let src = "\
local Circle = {}
Circle.__index = Circle

function Circle.new(radius)
  return setmetatable({ radius = radius }, Circle)
end

function Circle:area()
  local r = self.radius
  return r * r
end

local c = Circle.new(2)
";
        let out = display_outcome(src);
        assert_eq!(binding_ty(&out, "radius").to_string(), "integer");
        assert_eq!(binding_ty(&out, "r").to_string(), "integer");
    }

    #[test]
    fn display_mode_publishes_function_returns() {
        let src = "\
local function pair()
  return 1, \"x\"
end
";
        let out = display_outcome(src);
        assert_eq!(out.fn_returns.len(), 1, "{:?}", out.fn_returns);
        let rendered: Vec<String> = out.fn_returns[0]
            .returns
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(rendered, vec!["1", "\"x\""]);
        // The range covers the function in the source.
        assert_eq!(
            &src[out.fn_returns[0].range.clone()].lines().next(),
            &Some("local function pair()")
        );
    }

    #[test]
    fn display_mode_leaves_annotated_returns_to_the_editor() {
        // The editor renders `---@return` annotations verbatim (their type
        // names may not resolve in the per-file env); inference publishes
        // returns only for unannotated functions.
        let src = "\
---@return integer
local function one()
  return 1
end
";
        let out = display_outcome(src);
        assert!(out.fn_returns.is_empty(), "{:?}", out.fn_returns);
    }

    // --- display mode: cross-file (externals) ------------------------------

    #[test]
    fn module_export_is_the_chunk_return_type() {
        let src = "\
local M = {}

function M.area(w, h)
  return w * h
end

return M
";
        let out = display_outcome(src);
        let export = out.module_export.expect("module export");
        let rendered = export.to_string();
        assert!(
            rendered.contains("area: fun(w"),
            "export should carry the function: {rendered}"
        );
    }

    #[test]
    fn external_seeds_type_exported_function_params() {
        let src = "\
local M = {}

function M.area(w, h)
  local result = w * h
  return result
end

return M
";
        let mut externals = ExternalTypes::default();
        externals
            .fn_param_seeds
            .insert("area".to_string(), vec![Ty::Integer, Ty::Number]);
        let out = display_outcome_ext(src, Some(&externals));
        assert_eq!(binding_ty(&out, "w").to_string(), "integer");
        assert_eq!(binding_ty(&out, "h").to_string(), "number");
        assert_eq!(binding_ty(&out, "result").to_string(), "number");
    }

    #[test]
    fn external_seeds_reach_local_functions_by_name() {
        let src = "\
local function helper(x)
  return x
end
return { helper = helper }
";
        let mut externals = ExternalTypes::default();
        externals
            .fn_param_seeds
            .insert("helper".to_string(), vec![Ty::String]);
        let out = display_outcome_ext(src, Some(&externals));
        assert_eq!(binding_ty(&out, "x").to_string(), "string");
    }

    #[test]
    fn require_evaluates_to_the_external_export() {
        let src = "\
local M = require(\"geometry\")
local a = M.area(3, 4)
M.area(3.5, 2)
";
        // The export of \"geometry\": a table with an `area` function whose
        // inferred returns are published (display mode).
        let export = Ty::Table(Box::new(crate::ty::TableTy {
            fields: [(
                "area".to_string(),
                FieldTy {
                    ty: Ty::Function(Box::new(FunctionTy {
                        params: vec![
                            ParamTy {
                                name: "w".to_string(),
                                ty: Ty::Number,
                                optional: false,
                            },
                            ParamTy {
                                name: "h".to_string(),
                                ty: Ty::Number,
                                optional: false,
                            },
                        ],
                        returns: vec![Ty::Number],
                        has_return_annotation: true,
                        ..FunctionTy::default()
                    })),
                    optional: false,
                },
            )]
            .into(),
            ..crate::ty::TableTy::default()
        }));
        let mut externals = ExternalTypes::default();
        externals.requires.insert("geometry".to_string(), export);
        let out = display_outcome_ext(src, Some(&externals));
        // `M` is the module table, and the call result types through.
        assert!(binding_ty(&out, "M").to_string().contains("area: fun("));
        assert_eq!(binding_ty(&out, "a").to_string(), "number");
        // And the calls were recorded as outgoing args for the dependency.
        let seeds = out.outgoing_calls.get("area").expect("outgoing");
        assert_eq!(seeds[0].to_string(), "integer|number");
        assert_eq!(seeds[1].to_string(), "integer");
    }

    #[test]
    fn check_mode_never_seeds_params() {
        let src = "\
local function area(w, h)
  return w * h
end
local a = area(3, 4)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "w").to_string(), "unknown");
        assert_eq!(binding_ty(&out, "h").to_string(), "unknown");
    }

    // --- `---@operator` overloads (#114) ----------------------------------

    const VEC: &str = "\
---@class Vec
---@operator add(Vec): Vec
---@operator sub(Vec): Vec
---@operator mul(number): Vec
---@operator unm: Vec
---@operator len: integer
";

    #[test]
    fn binary_operator_types_the_result() {
        let src = format!(
            "{VEC}\
---@type Vec
local a
---@type Vec
local b
local c = a + b
"
        );
        let out = outcome(&src);
        // Without the overload `a + b` would degrade to `unknown`; luals (and
        // now luabox) types it as the declared result `Vec`.
        assert_eq!(binding_ty(&out, "c").to_string(), "Vec");
    }

    #[test]
    fn reversed_operand_dispatch_consults_right_class() {
        // `2 * v`: the LEFT operand is a plain number with no `mul` overload,
        // so dispatch falls to the RIGHT operand's class (Lua metamethod
        // order). Vec's `mul(number): Vec` matches with the number as its arg.
        let src = format!(
            "{VEC}\
---@type Vec
local v
local scaled = 2 * v
"
        );
        let out = outcome(&src);
        assert_eq!(binding_ty(&out, "scaled").to_string(), "Vec");
    }

    #[test]
    fn overloaded_operator_selects_by_param_type() {
        let src = "\
---@class Poly
---@operator add(Poly): Poly
---@operator add(number): number

---@type Poly
local p
---@type Poly
local q
local same = p + q
local shifted = p + 1
";
        let out = outcome(src);
        // First overload (param `Poly`) matches `p + q`.
        assert_eq!(binding_ty(&out, "same").to_string(), "Poly");
        // Second overload (param `number`) matches `p + 1`.
        assert_eq!(binding_ty(&out, "shifted").to_string(), "number");
    }

    #[test]
    fn unary_unm_and_len_operators_apply() {
        let src = format!(
            "{VEC}\
---@type Vec
local v
local neg = -v
local n = #v
"
        );
        let out = outcome(&src);
        assert_eq!(binding_ty(&out, "neg").to_string(), "Vec");
        // `len` is declared `integer` here (the default too, but this proves
        // the overload path returns the declared type).
        assert_eq!(binding_ty(&out, "n").to_string(), "integer");
    }

    #[test]
    fn len_operator_can_override_the_default_integer() {
        let src = "\
---@class Sized
---@operator len: string

---@type Sized
local s
local m = #s
";
        let out = outcome(src);
        // A declared `len` result replaces the built-in `integer` for `#`.
        assert_eq!(binding_ty(&out, "m").to_string(), "string");
    }

    #[test]
    fn undeclared_operator_preserves_unknown() {
        // A class with no `---@operator` behaves as before: the operator
        // expression degrades to `unknown` (no invented diagnostic).
        let src = "\
---@class Bare

---@type Bare
local a
---@type Bare
local b
local c = a + b
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "c").to_string(), "unknown");
    }

    #[test]
    fn operator_result_is_checked_against_annotations() {
        // Correct usage types clean under strict (proves the result is `Vec`,
        // not `unknown` — `unknown -> Vec` would itself error under strict).
        let ok = format!(
            "{VEC}\
---@type Vec
local a
---@type Vec
local b
---@type Vec
local c = a + b
"
        );
        assert_eq!(strict_codes(&ok), Vec::<String>::new());

        // Misusing the result is caught: `Vec + Vec` is not a `string`.
        let bad = format!(
            "{VEC}\
---@type Vec
local a
---@type Vec
local b
---@type string
local s = a + b
"
        );
        assert_eq!(strict_codes(&bad), vec!["LB0300"]);
    }

    #[test]
    fn operator_inherited_from_parent_class() {
        // `---@operator` rides the class surface like `---@field`s: a subclass
        // inherits its parent's operators.
        let src = "\
---@class Base
---@operator add(Base): Base

---@class Derived : Base

---@type Derived
local a
---@type Base
local b
local c = a + b
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "c").to_string(), "Base");
    }

    // --- `---@operator call` — callable class values (#122) ---------------

    #[test]
    fn call_operator_makes_a_value_callable_and_types_the_result() {
        // `obj(42)` on a class declaring `---@operator call(number): string`
        // yields the declared result `string` — flowing into an unannotated
        // binding (the inlay-hint path).
        let src = "\
---@class Callable
---@operator call(number): string
local M = {}
---@type Callable
local obj = M
local r = obj(42)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "r").to_string(), "string");
    }

    #[test]
    fn no_input_call_operator_result_types() {
        // A no-paren `call: T` operator accepts any arguments and yields `T`.
        let src = "\
---@class NoIn
---@operator call: boolean
local N = {}
---@type NoIn
local n = N
local a = n()
local b = n(1, \"two\")
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "a").to_string(), "boolean");
        assert_eq!(binding_ty(&out, "b").to_string(), "boolean");
    }

    #[test]
    fn overloaded_call_operator_selects_by_arg_type() {
        let src = "\
---@class Multi
---@operator call(number): string
---@operator call(boolean): integer
local Mu = {}
---@type Multi
local m = Mu
local s = m(1)
local i = m(true)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "s").to_string(), "string");
        assert_eq!(binding_ty(&out, "i").to_string(), "integer");
    }

    #[test]
    fn call_operator_inherited_from_parent_class() {
        let src = "\
---@class Base
---@operator call(string): integer
local B = {}
---@class Derived : Base
local D = {}
---@type Derived
local d = D
local i = d(\"hi\")
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "i").to_string(), "integer");
    }

    #[test]
    fn call_on_class_without_call_operator_is_unchanged() {
        // A class with no `call` operator is not callable: the result degrades
        // to `unknown` exactly as before (no invented result type).
        let src = "\
---@class Plain
local P = {}
---@type Plain
local p = P
local r = p(1)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "r").to_string(), "unknown");
    }

    #[test]
    fn primitive_operator_inference_unregressed() {
        // The overload path must never disturb primitive operator typing.
        let src = "\
local i = 1 + 2
local f = 1.5 + 2
local d = 3 / 2
local s = \"a\" .. \"b\"
local b = 1 & 2
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "i").to_string(), "integer");
        assert_eq!(binding_ty(&out, "f").to_string(), "number");
        assert_eq!(binding_ty(&out, "d").to_string(), "number");
        assert_eq!(binding_ty(&out, "s").to_string(), "string");
        assert_eq!(binding_ty(&out, "b").to_string(), "integer");
    }
}
