//! Rich table inference over the HIR (SPEC.md §3 — hard requirement).
//!
//! Tables never degrade to a bare `table` type: every table constructed in
//! the file gets an identity-tracked *shape* ([`InferredShape`]) that later
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

// The `impl Infer<'_>` surface is sectioned across these submodules: each is
// one more `impl Infer<'_>` block, no state and no behaviour of its own.
mod call;
mod cast;
mod narrow;
mod reify;
mod visibility;

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_diag::{Diagnostic, Label, Severity, Span};
use luabox_hir::{
    BinOp, Binding, BindingId, BindingKind, Block, Body, BodyId, Expr, ExprId, HirId, Literal,
    LoweredFile, Resolution, Stmt, StmtId, TableEntry, UnOp,
};

use crate::assign::Exactness;
use crate::codes::FIELD_NOT_FOUND;
use crate::env::{TypeEnv, merge_block_tags};
use crate::ty::{FunctionTy, Ty};

/// A byte range key, matching the annotation checker's convention.
type Key = (usize, usize);

/// A `:` method call's resolved signature, as published to the annotation
/// checker by [`Outcome::method_sigs`].
#[derive(Debug, Clone)]
pub(crate) struct MethodSig {
    /// The resolved member's signature, with its implicit `self` (if declared)
    /// still in place.
    pub(crate) sig: FunctionTy,
    /// Whether the call's explicit arguments may be arity- and type-checked
    /// against `sig`. Only when the receiver resolves to a declared
    /// `---@class`: a plain inferred table's method is resolved structurally
    /// and its contract is not authoritative enough to manufacture argument
    /// errors from (SPEC §19 conservatism). The callee's *use-site tags* —
    /// `---@deprecated` (LB0308), `---@async` (LB0316), `---@version` — carry
    /// no such risk: they are what the author wrote on the method itself, so
    /// they are reported for every resolved receiver (#33).
    pub(crate) args_checkable: bool,
}

/// What inference hands back to the checker.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Reified expression types keyed by byte range. Only expressions the
    /// annotation checker cannot type itself are published; `unknown`
    /// results are omitted.
    pub(crate) expr_types: HashMap<Key, Ty>,
    /// The resolved function signature of a `:` method call, keyed by the
    /// method-call expression's byte range. Published whenever the receiver
    /// resolves to a concrete field whose type is a function — the engine's
    /// method resolution the annotation checker consumes to flag the callee's
    /// use-site tags and (when [`MethodSig::args_checkable`]) to
    /// argument-check the call (#118, #33). The signature is as-declared
    /// (including its `self` parameter, if any, and `---@deprecated`); the
    /// checker strips the implicit `self` before matching explicit arguments.
    pub(crate) method_sigs: HashMap<Key, MethodSig>,
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

impl Outcome {
    /// The borrowed view of just the four fields the annotation checker reads
    /// ([`crate::check::run`]), leaving the display-only surface untouched.
    pub(crate) fn view(&self) -> crate::check::InferenceView<'_> {
        crate::check::InferenceView {
            expr_types: &self.expr_types,
            method_sigs: &self.method_sigs,
            carrier_final: &self.carrier_final,
            carrier_class_final: &self.carrier_class_final,
        }
    }
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

/// Which surface inference is being run for.
///
/// The two modes differ in exactly one way — whether *guessing* is allowed.
/// [`InferMode::Display`] may seed an unannotated parameter from the argument
/// types observed at the function's call sites; [`InferMode::Check`] may not,
/// because a diagnostic must never rest on a guess (SPEC §19 conservatism).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InferMode {
    /// The checker's surface: annotations and literals only, no call-site
    /// parameter seeding, so no diagnostic can arise from inference about a
    /// call site elsewhere.
    Check,
    /// The editor's inlay-hint surface: call-site parameter seeding on, so the
    /// bodies of unannotated functions type through. Never feeds diagnostics.
    Display,
}

impl InferMode {
    /// Whether unannotated parameters take the union of the (widened) argument
    /// types observed at the function's call sites during the first pass.
    const fn seeds_params(self) -> bool {
        matches!(self, InferMode::Display)
    }
}

/// Run inference over one lowered file.
///
/// `exact` is the assignability end of the strictness ladder, which also
/// selects the severity inference's own diagnostics carry. `mode` selects the
/// checker vs. editor surface ([`InferMode`]). `externals` carries the
/// cross-file inputs (require exports + dependent files' call args); together
/// with `InferMode::Display`'s seeding they are display-only, so neither can
/// manufacture a diagnostic.
pub(crate) fn run(
    hir: &LoweredFile,
    env: &TypeEnv,
    file: &str,
    exact: Exactness,
    mode: InferMode,
    externals: Option<&ExternalTypes>,
) -> Outcome {
    let severity = if exact.is_strict() {
        Severity::Error
    } else {
        Severity::Warning
    };
    let mut infer = Infer {
        env,
        hir,
        file,
        severity,
        exact,
        pass: 0,
        mode,
        externals,
        shapes: Vec::new(),
        shape_of_expr: HashMap::new(),
        instances: HashMap::new(),
        declared_carriers: HashMap::new(),
        carrier_locals: HashMap::new(),
        class_carriers: HashMap::new(),
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
        .map(|ity| infer.reify_export(&ity));
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
    //
    // A class may be carried more than once in one file (`---@class Two` over
    // two different tables, each with its own methods). Those carriers
    // **union**, exactly as duplicate `---@class` declarations do (#49) — and
    // the fold is done in statement order rather than `HashMap` order, so a
    // repeated carrier folds the same way on every run.
    let mut class_carriers: Vec<(Key, CarrierRef, String)> = infer
        .class_carriers
        .iter()
        .map(|(k, (c, n))| (*k, c.clone(), n.clone()))
        .collect();
    class_carriers.sort_by_key(|(key, _, _)| *key);
    let mut carrier_class_final: HashMap<String, Ty> = HashMap::new();
    for (key, carrier, name) in class_carriers {
        if let Some(ity) = infer.carrier_ity(&carrier) {
            let ty = infer.reify(&ity);
            carrier_final.entry(key).or_insert_with(|| ty.clone());
            match carrier_class_final.remove(&name) {
                Some(existing) => {
                    carrier_class_final.insert(name, union_carrier_shapes(existing, ty));
                }
                None => {
                    carrier_class_final.insert(name, ty);
                }
            }
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
    /// An unannotated function literal; its signature lives in [`InferredFn`].
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
struct InferredShape {
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
struct InferredFn {
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

/// What a `---@class` carrier statement binds its table to.
///
/// A `local X = {}` carrier is a [`BindingId`]; a `Glob = {}` carrier is a
/// *free* name, which has no binding at all — the reason global carriers used
/// to lose every member they collected (#50). Wave 19 met the same shape in
/// `luabox-lint` (`ValueRef::Global`) and answered it the same way: key the
/// global half by name.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CarrierRef {
    Local(BindingId),
    Global(String),
}

struct Infer<'a> {
    env: &'a TypeEnv,
    hir: &'a LoweredFile,
    file: &'a str,
    severity: Severity,
    /// The assignability end of the strictness ladder (drives assignability
    /// inside carrier classification).
    exact: Exactness,
    /// 0 = build shapes, 1 = emit diagnostics + publish types.
    pass: u8,
    /// The surface this run serves — [`InferMode::Display`] seeds unannotated
    /// parameters from call-site argument types; [`InferMode::Check`] does not.
    mode: InferMode,
    /// Cross-file inputs (require exports + dependents' call args), when
    /// the analysis layer supplied them. Display-only.
    externals: Option<&'a ExternalTypes>,
    shapes: Vec<InferredShape>,
    /// Constructor site → shape, so pass 2 reuses pass 1's identities.
    shape_of_expr: HashMap<(BodyId, ExprId), usize>,
    /// Carrier shape → its shared instance shape.
    instances: HashMap<usize, usize>,
    /// Declared name → the carrier shapes bound to that declaration, so an
    /// annotated instance value (`---@return Circle`) still resolves
    /// methods and inferred extensions through the carrier (#73).
    ///
    /// A `Vec` because one class may be carried more than once in a file:
    /// duplicate `---@class` declarations union rather than replace (#49), so
    /// a member is looked up across every carrier, in declaration order,
    /// first-wins — the rule the rest of the crate merges classes by.
    declared_carriers: HashMap<String, Vec<usize>>,
    /// `---@type` carrier locals (`local X = {}` whose object annotation is
    /// satisfied only by later extension), keyed by the `local` statement's
    /// byte range → the carrier binding. Their final shape is published as
    /// [`Outcome::carrier_final`] for the deferred conformance check.
    carrier_locals: HashMap<Key, BindingId>,
    /// `---@class Name` carriers (`local X = {}`, `Glob = {}`, or a
    /// re-assignment that re-carries an existing binding), keyed by the
    /// carrier statement's byte range → what the class is carried *by*, plus
    /// the class name. Their final reified shape feeds the checker's
    /// `: Interface` conformance check (#107) and the file's exported class
    /// surface ([`crate::FileTypes::collect`]).
    class_carriers: HashMap<Key, (CarrierRef, String)>,
    /// The statement keys classified as carriers in pass 0, reused verbatim
    /// in pass 1 so the keep-the-shape decision (rather than freeze to the
    /// annotated type) is identical across passes.
    carrier_keys: HashSet<Key>,
    funcs: HashMap<BodyId, InferredFn>,
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
    method_sigs: HashMap<Key, MethodSig>,
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
        self.shapes.push(InferredShape::default());
        self.shape_of_expr.insert((body, expr), id);
        id
    }

    /// The shared instance shape of a carrier/metatable (created lazily).
    fn instance_of(&mut self, carrier: usize) -> usize {
        if let Some(&id) = self.instances.get(&carrier) {
            return id;
        }
        let id = self.shapes.len();
        self.shapes.push(InferredShape {
            metatable: Some(carrier),
            declared: self.shapes[carrier].declared.clone(),
            is_instance: true,
            ..InferredShape::default()
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
    /// `undefined-field` phrasing, names the three ways to declare the
    /// member (round 3 review F71 — least-surprise for a newly-firing
    /// diagnostic is naming the remedy, not just the symptom), and carries a
    /// "declared here" secondary label pointing at the class's own file —
    /// same-file via [`crate::env::TypeEnv::class_decl_span`], cross-file via
    /// [`crate::env::TypeEnv::cross_file_class_decl_span`] (previously
    /// silently omitted for a class declared anywhere but the file currently
    /// checking, which is exactly the shape a `require`d carrier's members
    /// take). For an inferred table it keeps the constructor/metatable
    /// phrasing.
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
                format!(
                    "`{class}` declares no field `{name}` — add `---@field {name} <type>`, \
                     attach `function {class}.{name}(...)` / `function {class}:{name}(...)`, \
                     or declare a `---@field [string] <type>` key space"
                ),
            ),
            None => (
                format!("cannot find field `{name}` on this table"),
                format!(
                    "`{name}` is not defined by the table's constructor, assignments, or metatable chain"
                ),
            ),
        };
        let mut diag = Diagnostic::new(FIELD_NOT_FOUND, self.severity, message)
            .with_label(Label::primary(Span::new(self.file, start..end), label));
        if let Some(class) = declared {
            if let Some(range) = self.env.class_decl_span(class) {
                diag = diag.with_label(Label::secondary(
                    Span::new(self.file.to_string(), range),
                    format!("`{class}` declared here"),
                ));
            } else if let Some((decl_file, range)) = self.env.cross_file_class_decl_span(class) {
                diag = diag.with_label(Label::secondary(
                    Span::new(decl_file, range),
                    format!("`{class}` declared here"),
                ));
            }
        }
        self.diags.push(diag);
    }

    // --- field lookup ------------------------------------------------------

    /// Fold a same-file carrier attachment's use-site tags onto a member that
    /// resolved through its class's *declared* shape.
    ///
    /// A `---@field m fun(...)` line is authoritative for the member's type,
    /// and `class_shape` lets it shadow the `function C:m()` attachment
    /// accordingly. But `fun(...)` syntax has nowhere to write
    /// `---@deprecated`/`---@async`/`---@version`: those tags only ever live on
    /// the carrier. Without carrying them across, declaring a method as a
    /// `---@field` *and* defining it silently disables LB0308/LB0316 at every
    /// `c:m()` site (#33). Only the tags travel — parameters, returns,
    /// overloads and generics stay as declared.
    ///
    /// The attachment is read straight off the carrier shape (`local C = {}`,
    /// which `function C:m()` extends), not through another class-shape
    /// lookup, so this cannot re-enter the resolution it is refining.
    fn carrier_tagged(&self, class: &str, name: &str, found: ITy) -> ITy {
        let ITy::Ty(Ty::Function(sig)) = &found else {
            return found;
        };
        // Already tagged, or no same-file carrier to consult.
        if sig.deprecated || sig.is_async || sig.version.is_some() {
            return found;
        }
        let Some(attached) = self
            .declared_carriers
            .get(class)
            .into_iter()
            .flatten()
            .find_map(|&carrier| self.shapes[carrier].fields.get(name))
        else {
            return found;
        };
        let Some(tags) = self.tags_of(attached) else {
            return found;
        };
        if !tags.deprecated && !tags.is_async && tags.version.is_none() {
            return found;
        }
        let mut tagged = (**sig).clone();
        tagged.deprecated = tags.deprecated;
        tagged.is_async = tags.is_async;
        tagged.version.clone_from(&tags.version);
        ITy::Ty(Ty::Function(Box::new(tagged)))
    }

    /// The declared signature behind a function-valued member, whether it is an
    /// already-reified type or a body this file defines.
    fn tags_of<'s>(&'s self, ity: &'s ITy) -> Option<&'s FunctionTy> {
        match ity {
            ITy::Ty(Ty::Function(sig)) => Some(sig),
            ITy::Func(body) => self.funcs.get(body)?.sig.as_ref(),
            _ => None,
        }
    }

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
                        return Lookup::Found(self.carrier_tagged(&class, name, ITy::Ty(ty)));
                    }
                    // Absence is diagnosable only for a real LuaCATS `---@class`
                    // with no indexer/array part. A dynamic-access class stays
                    // lenient — and so does a class whose ancestry the
                    // `class_shape` call just above truncated (`LB0317`,
                    // production readiness review finding 1): its shape is
                    // admittedly incomplete, so an absent field here might
                    // simply be declared past the cutoff. `LB0317` alone is
                    // the honest diagnostic; a false `LB0306` on top of it is
                    // not.
                    if self.env.is_class(&class)
                        && shape.indexers.is_empty()
                        && shape.array.is_none()
                        && !self.env.class_ancestry_truncated(&class)
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
                        // An *instance* shape's metatable is the carrier it was
                        // minted from, and a carrier attachment (`function C:m()`)
                        // is a member of the class whether or not the author also
                        // wrote the runtime `C.__index = C` link — luals folds
                        // `function C:m()` into `---@class C` off the carrier
                        // binding, with no metatable reasoning at all. Without
                        // this fall-through the canonical `---@class C` + carrier
                        // shape (no `__index`) reports LB0306 at every `o:m()`
                        // and drops the method's `---@deprecated`/`---@async`
                        // tags (#33 residue). Only *adds* resolutions, so no new
                        // undefined-field can arise from it.
                        if self.shapes[s].is_instance {
                            cur = Some(meta);
                            continue;
                        }
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
                    if crate::assign::assignable(self.env, crate::assign::Exactness::Loose, &key, k)
                    {
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
                    return Lookup::Found(self.carrier_tagged(class, name, ity));
                }
                // An annotated instance (`---@return Circle`, `---@type
                // Circle`) still resolves methods and inferred extensions
                // through the declared carrier's shared instance shape (#73) —
                // through *every* carrier of the class, since duplicate
                // declarations union (#49).
                let carriers = self
                    .declared_carriers
                    .get(class.as_str())
                    .cloned()
                    .unwrap_or_default();
                for carrier in carriers {
                    let instance = self.instance_of(carrier);
                    if let Lookup::Found(ity) = self.lookup_shape_field(instance, name) {
                        return Lookup::Found(ity);
                    }
                }
                // Absent on a declared class → luals `undefined-field` (#90),
                // provable only for a real LuaCATS `---@class` with no
                // indexer/array part (dynamic access), that resolved to a
                // table (not an enum union), and whose ancestry the
                // `resolve_named` call above did not truncate (`LB0317`,
                // production readiness review finding 1 — a truncated shape
                // stays lenient on member reads, same as `lookup_shape_field`).
                let dynamic = match &resolved {
                    Ty::Table(t) => !t.indexers.is_empty() || t.array.is_some(),
                    _ => true,
                };
                let provable = self.env.is_class(class)
                    && !dynamic
                    && !self.env.class_ancestry_truncated(class);
                Lookup::Absent {
                    provable,
                    declared: provable.then(|| class.clone()),
                }
            }
            Ty::String | Ty::StringLit(_) => self.lookup_string_member(name),
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

    /// A member read off a string value, resolved through the `string`
    /// library — which is what Lua itself does.
    ///
    /// Every string in a Lua state shares one metatable whose `__index` is the
    /// `string` table (`lstrlib.c`'s `createmetatable`, run by
    /// `luaopen_string`). So `s:upper()` *is* `string.upper(s)` and `s.upper`
    /// *is* `string.upper` — one resolution, the `:` form with `self` bound.
    /// luals models it the same way and types both.
    ///
    /// Resolution goes through the ordinary dotted-function registry, so a
    /// project that extends the library (`function string.trim(s) end`) gets
    /// `s:trim()` for free, exactly as it does at runtime.
    fn lookup_string_member(&mut self, name: &str) -> Lookup {
        if let Some(sig) = self.env.function(&format!("string.{name}")) {
            return Lookup::Found(ITy::Ty(Ty::Function(Box::new(sig.clone()))));
        }
        // A definition package may spell the library as a table type
        // (`---@class stringlib` with `---@field upper fun(...)`) rather than
        // as `function string.upper` statements; both reach the same members.
        if let Some(ty) = self.env.global_type("string").cloned()
            && let Lookup::Found(ity) = self.lookup_ty_field(&ty, name)
        {
            return Lookup::Found(ity);
        }
        // The string metatable is fixed, so a member the library does not
        // declare is a genuine `undefined-field`: `("x"):nope()` raises
        // "attempt to call a nil value (method 'nope')" at runtime.
        Lookup::Absent {
            provable: true,
            declared: Some("string".to_string()),
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
            } else if self.mode.seeds_params() {
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
                // `---@param`-annotated `return function(…) end`: the doc block
                // above the `return` binds to the returned function literal
                // (the `direct` module shape of #46), so it supplies that
                // function's signature — its body walks against the declared
                // parameters, and the value the module exports is the declared
                // `fun(…)` rather than a signature inferred off an unannotated
                // body. Exactly the rule `local f = function(…) end` follows in
                // `walk_local`, applied to the one other place a doc block can
                // sit above a function literal.
                let return_sig = self
                    .stmt_range(body, stmt)
                    .and_then(|key| self.env.fn_sig(key))
                    .cloned();
                let returned_fn = exprs.first().and_then(|&e| match self.body(body).expr(e) {
                    Expr::Function(b) => Some(*b),
                    _ => None,
                });
                let values = match (&return_sig, returned_fn) {
                    (Some(sig), Some(fn_body)) => {
                        self.walk_body(fn_body, Some(sig), None);
                        let mut values = vec![ITy::Ty(Ty::Function(Box::new(sig.clone())))];
                        // The annotated function is the first returned value;
                        // anything after it evaluates normally.
                        values.extend(self.eval_values(
                            body,
                            exprs.get(1..).unwrap_or_default(),
                            None,
                        ));
                        values
                    }
                    _ => self.eval_values(body, &exprs, None),
                };
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
        if let Some((key, local)) = key.zip(names.first()) {
            self.record_class_carrier(key, &CarrierRef::Local(local.binding));
        }
    }

    /// The live inference type a carrier is currently bound to.
    ///
    /// A `local` carrier lives in the flow state; a global one has no binding
    /// at all and lives in the globals map (#50).
    fn carrier_ity(&self, carrier: &CarrierRef) -> Option<ITy> {
        match carrier {
            CarrierRef::Local(binding) => self.state.get(binding).cloned(),
            CarrierRef::Global(name) => self.globals.get(name).cloned(),
        }
    }

    /// Associate the table a `---@class` statement carries with the class it
    /// declares, if the statement carries one.
    ///
    /// Shared by both carrier spellings — `local X = {}` and `Glob = {}` /
    /// `X = {}` — because luals draws no distinction between them: the class
    /// is carried by whatever the statement binds, and every later
    /// `function X:m()` is a member of it (#50). A carrier that is not a table
    /// shape (a call result, a re-assignment frozen by an annotation) records
    /// nothing, exactly as before.
    fn record_class_carrier(&mut self, key: Key, carrier: &CarrierRef) {
        let Some(name) = self.env.declared_target(key) else {
            return;
        };
        let name = name.to_string();
        let Some(ITy::Shape(id)) = self.carrier_ity(carrier) else {
            return;
        };
        self.shapes[id].declared = Some(name.clone());
        // Appended, not overwritten: a class declared twice in one file is
        // carried twice, and both carriers' members belong to it (#49). Pass 1
        // re-walks the same statements and re-derives the same shape ids, so
        // the list is kept a set.
        let carriers = self.declared_carriers.entry(name.clone()).or_default();
        if !carriers.contains(&id) {
            carriers.push(id);
        }
        // A `---@class Name : Parent` carrier: record it so its final reified
        // shape can be checked for `: Interface` conformance (#107) and folded
        // into the file's exported class surface. Parentless classes carry no
        // obligation, so the checker skips them; recording them is harmless.
        self.class_carriers.insert(key, (carrier.clone(), name));
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
                            crate::assign::classify_literal(self.env, self.exact, &lit, &target),
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
        let key = self.stmt_range(body, stmt);
        // `---@type fun(…)` on an assignment, positional exactly as on a
        // `local` (`---@type A, B` covers `a, b = …`), so a lone annotation on
        // a multi-assignment applies to the first target only.
        let declared_tys: Option<Vec<Ty>> = key
            .and_then(|k| self.env.typed_local(k))
            .map(<[Ty]>::to_vec);
        // Desugared method/function declaration: `function T:m() ... end`
        // becomes `T.m = function(self) ... end`. The carrier must be known
        // *before* the body walks so `self` gets the instance shape.
        if let ([target], [value]) = (targets, values)
            && let Expr::Function(fn_body) = self.body(body).expr(*value)
        {
            let fn_body = *fn_body;
            let block_sig = key.and_then(|k| self.env.fn_sig(k)).cloned();
            // An explicit `---@type fun(…)` is authoritative for the value
            // (SPEC §3), so it supplies the signature — parameters, returns,
            // overloads and generics — while the block's own tags
            // (`---@deprecated`/`---@async`/`---@nodiscard`/`---@version`),
            // which `fun(…)` syntax cannot express, ride along (#38).
            // `walk_body` then types the literal's parameters from it, the
            // same bidirectional rule `---@type` on a `local` follows.
            let sig = match declared_fn_sig(declared_tys.as_deref(), 0) {
                Some(declared) => Some(merge_block_tags(declared, block_sig.as_ref())),
                None => block_sig,
            };
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
        // A `---@type T` seeds the initializer in its slot before the value is
        // walked, exactly as on a `local` — a `fun(…)` types a function
        // literal's parameters, a class/table target types a table literal's
        // members. Positional: slot `i` seeds target `i`.
        for (i, &value) in values.iter().enumerate() {
            if let Some(ty) = declared_tys.as_ref().and_then(|t| t.get(i)) {
                let ty = ty.clone();
                self.seed_contextual(body, value, &ty);
            }
        }
        let values = self.eval_values(body, values, Some(targets.len()));
        // The annotation is *authoritative* for the slot it names (#48): the
        // assigned target reads back as the declared type, not as whatever the
        // initializer inferred. This is the `local` rule — `---@type string`
        // over `local a = 1` declares `a: string` — applied to the other
        // spellings of the same statement, which luals does not distinguish:
        // `M.a = 1`, `M["a"] = 1`, `M.a.b = 1`, `G = 1`. `assign_into` still
        // refuses to overwrite an already-declared local binding, so an
        // annotation cannot silently redeclare one.
        // Whether this statement carries a `---@class`, which changes how its
        // first target is written: see below.
        let carries_class = key.is_some_and(|k| self.env.declared_target(k).is_some());
        for (i, target) in resolved.iter().enumerate() {
            let value = match declared_tys.as_ref().and_then(|t| t.get(i)) {
                Some(ty) => ITy::Ty(ty.clone()),
                // A target with no value of its own (`a, b = f()` past the
                // expansion) is left exactly as it was, not nil-ed.
                None => match values.get(i) {
                    Some(value) => value.clone(),
                    None => continue,
                },
            };
            // A global write normally *unions* with whatever the global held —
            // globals are written from anywhere, so the conservative merge is
            // right. A `---@class Glob = {}` carrier statement is the exception:
            // it rebinds the carrier outright, so a variable carried twice
            // answers with its most recent carrier, the way Lua resolves the
            // name and the way the defs-side `carrier_var_classes` already
            // answered (waves 3/4 — defs/project parity, #50). A `local`
            // carrier already replaced, so this only aligns the global half.
            if carries_class
                && i == 0
                && let Target::Global(name) = target
            {
                self.globals.insert(name.clone(), value);
            } else {
                self.assign_into(target, value);
            }
        }
        // `---@class` carried by an assignment — `Glob = {}` (a global, which
        // has no binding to key on) or `X = {}` re-carrying an existing one
        // (#50). Recorded after the assignment so the carrier's shape is the
        // one this statement just bound.
        if let Some((key, target)) = key.zip(resolved.first())
            && let Some(carrier) = assign_carrier_ref(target)
        {
            self.record_class_carrier(key, &carrier);
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
                    if crate::assign::assignable(
                        self.env,
                        crate::assign::Exactness::Loose,
                        other,
                        input,
                    ) {
                        return Some(sig.result.clone());
                    }
                }
                (None, None) => return Some(sig.result.clone()),
                _ => {}
            }
        }
        None
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

/// The `fun(…)` signature a `---@type` annotation declares for slot `index`
/// of the statement it annotates, if that slot declares a function type.
///
/// `---@type A, B` is positional (one type per assigned name, exactly as on a
/// `local`), so a lone `---@type fun(…)` above `a, b = f1, f2` declares only
/// `a` — `b` keeps its inferred type (#38).
fn declared_fn_sig(declared_tys: Option<&[Ty]>, index: usize) -> Option<FunctionTy> {
    match declared_tys?.get(index)? {
        Ty::Function(sig) => Some((**sig).clone()),
        _ => None,
    }
}

/// The carrier an assignment target names, for a `---@class` bound to an
/// assignment statement (#50).
///
/// Only a whole variable carries a class — a global (`Glob = {}`, which has no
/// binding, hence the name key) or a local re-assigned in place (`M = {}`).
/// A field, index or opaque target names no variable and carries nothing,
/// matching [`crate::env::carrier_var_name`], the defs-side half of the same
/// rule.
fn assign_carrier_ref(target: &Target) -> Option<CarrierRef> {
    match target {
        Target::Binding { id, .. } => Some(CarrierRef::Local(*id)),
        Target::Global(name) => Some(CarrierRef::Global(name.clone())),
        _ => None,
    }
}

/// Fold a second carrier's reified shape into the first's, for a class carried
/// by more than one table in one file (#49).
///
/// Member-wise union, **first carrier wins** a same-name collision — the rule
/// [`TypeEnv::merge_file_types`] applies to the cross-file case and
/// [`TypeEnv::absorb_block`] to duplicate `---@field`s, so a class's members
/// resolve the same way however they were split up. A carrier that reified to
/// something other than a table contributes nothing.
fn union_carrier_shapes(first: Ty, second: Ty) -> Ty {
    let (mut first_table, second_table) = match (first, second) {
        (Ty::Table(f), Ty::Table(s)) => (f, s),
        (first, _) => return first,
    };
    for (name, field) in second_table.fields {
        first_table.fields.entry(name).or_insert(field);
    }
    for indexer in second_table.indexers {
        if !first_table.indexers.contains(&indexer) {
            first_table.indexers.push(indexer);
        }
    }
    if first_table.array.is_none() {
        first_table.array = second_table.array;
    }
    Ty::Table(first_table)
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
    use crate::ty::{FieldTy, ParamTy};
    use crate::{Strictness, check_file};

    fn outcome(source: &str) -> Outcome {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let env = TypeEnv::build(&parsed);
        let lowered = luabox_hir::lower(&parsed);
        run(
            &lowered,
            &env,
            "test.lua",
            Exactness::Strict,
            InferMode::Check,
            None,
        )
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
        run(
            &lowered,
            &env,
            "test.lua",
            Exactness::Strict,
            InferMode::Display,
            externals,
        )
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
    fn check_mode_export_keeps_unannotated_returns_uncontractual() {
        // #58 mutation audit: `has_return_annotation` on a reified
        // unannotated function must stay `false` in Check mode — it is the
        // flag that says "these returns are a description, not a contract"
        // (#46), and the seams that read it live in other crates, so
        // nothing here noticed it flipping.
        let src = "\
local M = {}

function M.f()
  return 1
end

return M
";
        let out = outcome(src);
        let export = out.module_export.expect("module export");
        let Ty::Table(table) = export else {
            panic!("plain module exports structurally, got {export}");
        };
        let Ty::Function(f) = &table.fields.get("f").expect("field f").ty else {
            panic!("f reifies as a function");
        };
        assert!(
            !f.has_return_annotation,
            "an unannotated body's returns are not a contract in Check mode"
        );
    }

    #[test]
    fn reified_shapes_exclude_metafields() {
        // #58 mutation audit: `__index` and friends are wiring, not members
        // — a reified export must not carry them, or every consumer of a
        // metatable-using module sees phantom fields.
        let src = "\
local M = {}
M.__index = M

function M.real()
  return 1
end

return M
";
        let out = outcome(src);
        let export = out.module_export.expect("module export");
        let rendered = export.to_string();
        assert!(rendered.contains("real"), "{rendered}");
        assert!(
            !rendered.contains("__index"),
            "metafields must not reify as members: {rendered}"
        );
    }

    #[test]
    fn a_returned_class_carrier_exports_as_the_class_it_carries() {
        // The export position mirrors what luals resolves a `require` to:
        // the returned carrier IS the class — the workspace-global identity —
        // not the structural table it happens to be inside its own file
        // (#56). Inside the file nothing changes; only what crosses the
        // `require` boundary does.
        let src = "\
---@class Point
---@field x number
local P = {}
return P
";
        let out = outcome(src);
        let export = out.module_export.expect("module export");
        assert_eq!(export.to_string(), "Point");
    }

    #[test]
    fn a_returned_plain_table_still_exports_structurally() {
        // Control: the overwhelmingly common `local M = {} … return M`
        // module has no class to become — its structural surface is the
        // export, exactly as before.
        let src = "\
local M = {}

function M.area(w, h)
  return w * h
end

return M
";
        let out = outcome(src);
        let export = out.module_export.expect("module export");
        let rendered = export.to_string();
        assert!(
            rendered.contains("area"),
            "a plain module must keep its structural export: {rendered}"
        );
    }

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

    #[test]
    fn every_binary_operator_dispatches_to_its_declared_overload() {
        // One `---@operator` per non-comparison binary operator, all on a
        // class operand so the primitive fast paths cannot apply. Each result
        // is a *distinct* type, so a mis-mapped operator name would show up as
        // the wrong result rather than merely `unknown`.
        let src = "\
---@class Ops
---@operator add(Ops): integer
---@operator sub(Ops): number
---@operator mul(Ops): string
---@operator div(Ops): boolean
---@operator mod(Ops): Ops
---@operator pow(Ops): integer
---@operator idiv(Ops): number
---@operator concat(Ops): string
---@operator band(Ops): boolean
---@operator bor(Ops): Ops
---@operator bxor(Ops): integer
---@operator shl(Ops): number
---@operator shr(Ops): string

---@type Ops
local a
---@type Ops
local b
local r_add = a + b
local r_sub = a - b
local r_mul = a * b
local r_div = a / b
local r_mod = a % b
local r_pow = a ^ b
local r_idiv = a // b
local r_concat = a .. b
local r_band = a & b
local r_bor = a | b
local r_bxor = a ~ b
local r_shl = a << b
local r_shr = a >> b
";
        let out = outcome(src);
        for (binding, expected) in [
            ("r_add", "integer"),
            ("r_sub", "number"),
            ("r_mul", "string"),
            ("r_div", "boolean"),
            ("r_mod", "Ops"),
            ("r_pow", "integer"),
            ("r_idiv", "number"),
            ("r_concat", "string"),
            ("r_band", "boolean"),
            ("r_bor", "Ops"),
            ("r_bxor", "integer"),
            ("r_shl", "number"),
            ("r_shr", "string"),
        ] {
            assert_eq!(binding_ty(&out, binding).to_string(), expected, "{binding}");
        }
    }

    #[test]
    fn comparison_operators_never_consult_overloads() {
        // `==`/`<`/… are always `boolean`; no `---@operator` lookup happens
        // (there is no LuaCATS operator name for them).
        let src = "\
---@class Cmp
---@operator add(Cmp): Cmp
---@type Cmp
local a
---@type Cmp
local b
local eq = a == b
local lt = a < b
local ne = a ~= b
";
        let out = outcome(src);
        for binding in ["eq", "lt", "ne"] {
            assert_eq!(
                binding_ty(&out, binding).to_string(),
                "boolean",
                "{binding}"
            );
        }
    }

    // --- loops: `while` / `repeat` / `do` ---------------------------------

    #[test]
    fn while_loop_joins_the_body_state_with_the_entry_state() {
        // The loop body may run zero times, so a binding reassigned inside it
        // carries the union of "unentered" and "entered" at the exit.
        let src = "\
local acc = 1
while SOME_GLOBAL do
  acc = \"text\"
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "acc").to_string(), "\"text\"|1");
    }

    #[test]
    fn while_condition_narrows_inside_the_body() {
        let src = "\
---@type string|nil
local s
while s do
  local inside = s
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "inside").to_string(), "string");
    }

    #[test]
    fn repeat_body_is_walked_before_its_until_condition() {
        // `repeat` bodies run at least once and the `until` expression is
        // evaluated in the body's scope — both halves must be typed.
        let src = "\
local n = 0
repeat
  local point = { x = 1, y = 2 }
  n = n + 1
until n > 3
";
        let out = outcome(src);
        let Ty::Table(table) = binding_ty(&out, "point") else {
            panic!("expected a structural table for `point`");
        };
        assert_eq!(table.fields["x"].ty.to_string(), "1");
        assert_eq!(binding_ty(&out, "n").to_string(), "integer");
    }

    #[test]
    fn do_block_statements_are_walked() {
        let src = "\
do
  local scoped = { name = \"x\" }
  local len = #scoped.name
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "len").to_string(), "integer");
    }

    #[test]
    fn returns_inside_loops_and_branches_reach_the_inferred_return() {
        // `collect_returns` must descend into every nested statement shape, or
        // a function whose only `return`s live inside control flow would look
        // like it returns nothing.
        let src = "\
local function pick(flag)
  if flag then
    return \"a\"
  end
  while flag do
    return 1
  end
  repeat
    return true
  until true
  for _ = 1, 2 do
    return nil
  end
  do
    return \"z\"
  end
end
local got = pick(true)
";
        let out = outcome(src);
        // One member per `return`, in source order — every nested statement
        // shape contributed.
        assert_eq!(
            binding_ty(&out, "got").to_string(),
            "\"a\"|1|true|nil|\"z\""
        );
    }

    // --- `---@cast` target resolution -------------------------------------

    /// A `---@cast` above a statement resolves the variable through a use
    /// *inside that statement*, whatever statement shape it is.
    #[test]
    fn cast_resolves_the_variable_through_every_statement_shape() {
        let src = "\
---@type string|integer
local w
---@cast w string
while #w > 0 do
  break
end

---@type string|integer
local r
---@cast r string
repeat
  local _ = r
until #r == 0

---@type string|integer
local nf
---@cast nf integer
for _ = nf, 10, 1 do
end

---@type string|integer
local gf
---@cast gf string
for _, _ in ipairs({ gf }) do
end

---@type string|integer
local blk
---@cast blk string
do
  local _ = blk
end

---@type string|integer
local br
---@cast br string
if #br > 0 then
  local _ = br
else
  local _ = br
end

---@type string|integer
local sink
---@type string|integer
local asg
---@cast asg string
sink = asg

---@type string|integer
local lf
---@cast lf string
local function reads_it()
  return lf
end
";
        let out = outcome(src);
        for name in ["w", "r", "gf", "blk", "br", "asg", "lf"] {
            assert_eq!(binding_ty(&out, name).to_string(), "string", "{name}");
        }
        assert_eq!(binding_ty(&out, "nf").to_string(), "integer");
    }

    /// …and through every *expression* shape the use may be buried in.
    #[test]
    fn cast_resolves_the_variable_through_every_expression_shape() {
        let src = "\
local host = { m = function(self, x) return x end }

---@type string|integer
local ix
---@cast ix string
local _ix = host[ix]

---@type string|integer
local mc
---@cast mc string
local _mc = host:m(mc)

---@type string|integer
local tb
---@cast tb string
local _tb = { tb, keyed = tb, [tb] = 1 }

---@type string|integer
local bin
---@cast bin string
local _bin = bin .. \"!\"

---@type string|integer
local un
---@cast un string
local _un = #un

---@type string|integer
local par
---@cast par string
local _par = (par)
";
        let out = outcome(src);
        for name in ["ix", "mc", "tb", "bin", "un", "par"] {
            assert_eq!(binding_ty(&out, name).to_string(), "string", "{name}");
        }
    }

    #[test]
    fn cast_falls_back_to_the_most_recent_binding_of_that_name() {
        // The annotated statement does not mention `v` at all, so the precise
        // in-statement lookup fails and the approximate fallback picks the
        // latest binding declared with that name.
        let src = "\
---@type string|integer
local v
---@type string|integer
local v
---@cast v string
local unrelated = 1
";
        let out = outcome(src);
        let vs: Vec<String> = out
            .binding_types
            .iter()
            .filter(|b| b.name == "v")
            .map(|b| b.ty.to_string())
            .collect();
        // Two `v` bindings: the later one took the cast, the earlier is intact.
        assert_eq!(vs, vec!["string|integer".to_string(), "string".to_string()]);
        assert_eq!(binding_ty(&out, "unrelated").to_string(), "1");
    }

    #[test]
    fn cast_on_a_global_updates_the_global_table() {
        // No local of that name exists, so the cast lands on the global slot
        // and the following read picks it up.
        let src = "\
---@cast G string
local read = G
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "read").to_string(), "string");
    }

    #[test]
    fn cast_reaches_a_use_that_only_appears_in_an_else_block() {
        let src = "\
---@type string|integer
local e
---@cast e string
if SOME_GLOBAL then
  local _ = 1
else
  local _ = e
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "e").to_string(), "string");
    }

    #[test]
    fn cast_on_a_jump_statement_uses_the_name_fallback() {
        // `break` / `goto` / `::label::` carry no expressions, so the precise
        // in-statement lookup finds nothing and the fallback resolves the
        // name — the cast still applies.
        let src = "\
---@type string|integer
local j
---@cast j string
goto done
::done::
local after = j
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "after").to_string(), "string");
    }

    #[test]
    fn cast_reaches_named_and_keyed_table_entries_and_parenthesised_uses() {
        let src = "\
---@type string|integer
local named
---@cast named string
local _named = { field = named }

---@type string|integer
local keyed
---@cast keyed string
local _keyed = { [keyed] = 1 }

---@type string|integer
local kv
---@cast kv string
local _kv = { [1] = kv }
";
        let out = outcome(src);
        for name in ["named", "keyed", "kv"] {
            assert_eq!(binding_ty(&out, name).to_string(), "string", "{name}");
        }
    }

    // --- writes through values that are not tracked shapes -------------------

    #[test]
    fn writes_through_annotated_containers_do_not_extend_a_shape() {
        // The receiver is an annotated type, not a locally-constructed table,
        // so the write has nowhere to accumulate: the annotation still governs
        // every read, and the written value escapes.
        let src = "\
---@class Holder
---@field f integer
---@type Holder
local obj
obj.f = 7

---@type number[]
local arr
arr[1] = 9

---@type table<string, boolean>
local map
map[SOME_KEY] = true

local read_field = obj.f
local read_elem = arr[1]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "read_field").to_string(), "integer");
        assert_eq!(binding_ty(&out, "read_elem").to_string(), "number");
        assert_eq!(out.diags, Vec::new());
    }

    // --- recursion & multi-value padding ------------------------------------

    #[test]
    fn a_recursive_unannotated_call_does_not_diverge() {
        // The in-progress function's own return is not yet known; the call
        // yields `unknown` instead of recursing forever.
        let src = "\
local function countdown(n)
  if n == 0 then
    return 0
  end
  return countdown(n - 1)
end
local got = countdown(3)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "0");
    }

    #[test]
    fn a_short_return_list_pads_the_remaining_slots_with_nil() {
        let src = "\
---@return string
local function one() return \"a\" end
local a, b, c = one()
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "a").to_string(), "string");
        assert_eq!(binding_ty(&out, "b").to_string(), "nil");
        assert_eq!(binding_ty(&out, "c").to_string(), "nil");
    }

    #[test]
    fn an_inline_as_cast_overrides_a_method_call_result() {
        let src = "\
---@class Src
---@field get fun(self): string
---@type Src
local s
local v = s:get() --[[@as integer]]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "v").to_string(), "integer");
    }

    // --- narrowing: falsy / union / `type()` families ---------------------

    #[test]
    fn truthiness_narrows_a_declared_union_member_wise() {
        let src = "\
---@type string|false|nil
local u
if u then
  local truthy = u
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "truthy").to_string(), "string");
    }

    #[test]
    fn falsy_branch_keeps_only_the_falsy_union_members() {
        let src = "\
---@type string|nil
local s
if not s then
  local only_nil = s
end
---@type boolean|nil
local b
if not b then
  local nil_or_false = b
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "only_nil").to_string(), "nil");
        assert_eq!(binding_ty(&out, "nil_or_false").to_string(), "nil|false");
    }

    #[test]
    fn falsy_branch_of_an_unknown_is_nil_or_false() {
        let src = "\
local function f(u)
  if not u then
    local falsy = u
  end
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "falsy").to_string(), "nil|false");
    }

    #[test]
    fn negated_or_narrows_both_operands() {
        // `if not (a or b)` — the *negative* branch of an `or` is the only
        // place both operands are known falsy.
        let src = "\
---@type string|nil
local a
---@type string|nil
local b
if not (a or b) then
  local na = a
  local nb = b
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "na").to_string(), "nil");
        assert_eq!(binding_ty(&out, "nb").to_string(), "nil");
    }

    #[test]
    fn type_call_narrows_every_recognized_type_name() {
        // `type(x) == "<name>"` on an `unknown` produces that name's base
        // type; `userdata`/`thread` have no IR base and stay `unknown`.
        let src = "\
local function f(v)
  if type(v) == \"nil\" then
    local as_nil = v
  elseif type(v) == \"boolean\" then
    local as_bool = v
  elseif type(v) == \"table\" then
    local as_table = v
  elseif type(v) == \"function\" then
    local as_fn = v
  elseif type(v) == \"userdata\" then
    local as_ud = v
  elseif type(v) == \"thread\" then
    local as_thread = v
  end
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "as_nil").to_string(), "nil");
        assert_eq!(binding_ty(&out, "as_bool").to_string(), "boolean");
        assert_eq!(binding_ty(&out, "as_table").to_string(), "table");
        assert_eq!(binding_ty(&out, "as_fn").to_string(), "fun(...: any)");
        assert_eq!(binding_ty(&out, "as_ud").to_string(), "unknown");
        assert_eq!(binding_ty(&out, "as_thread").to_string(), "unknown");
    }

    #[test]
    fn unrecognized_type_name_string_does_not_narrow() {
        // `type(x) == "Circle"` is not a `type()` result: no predicate at all,
        // so the value keeps its declared union.
        let src = "\
---@type string|integer
local v
if type(v) == \"Circle\" then
  local inside = v
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "inside").to_string(), "string|integer");
    }

    #[test]
    fn type_call_narrows_inferred_shapes_and_function_literals() {
        // The `table`/`function` predicates recognize inference-side shapes
        // and function literals, not only reified `Ty`s.
        let src = "\
local shape = { x = 1 }
local fn = function() end
if type(shape) == \"table\" then
  local kept_shape = shape
end
if type(shape) == \"string\" then
  local dropped_shape = shape
end
if type(fn) == \"function\" then
  local kept_fn = fn
end
";
        let out = outcome(src);
        let Ty::Table(kept) = binding_ty(&out, "kept_shape") else {
            panic!("expected the shape to survive the `table` predicate");
        };
        assert!(kept.fields.contains_key("x"));
        // No member survives `type(<table>) == "string"`: the branch is
        // statically impossible and degrades to the predicate's base type.
        assert_eq!(binding_ty(&out, "dropped_shape").to_string(), "string");
        assert_eq!(binding_ty(&out, "kept_fn").to_string(), "fun()");
    }

    #[test]
    fn type_call_narrows_annotated_table_and_function_values() {
        let src = "\
---@type table
local t
if type(t) == \"table\" then
  local as_table = t
end
---@type fun(): integer
local g
if type(g) == \"function\" then
  local as_fn = g
end
---@class Boxed
---@field v integer
---@type Boxed
local named
if type(named) == \"table\" then
  local as_named = named
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "as_table").to_string(), "table");
        assert_eq!(binding_ty(&out, "as_fn").to_string(), "fun(): integer");
        assert_eq!(binding_ty(&out, "as_named").to_string(), "Boxed");
    }

    // --- field lookup / indexing through unions and named classes ---------

    #[test]
    fn field_read_on_a_union_of_named_classes_unions_the_results() {
        let src = "\
---@class LA
---@field n integer
---@class LB
---@field n string
---@type LA|LB
local ab
local got = ab.n
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "integer|string");
    }

    #[test]
    fn field_absent_from_one_union_member_is_opaque_not_a_diagnostic() {
        let src = "\
---@class HasBoth
---@field n integer
---@field extra string
---@class HasOne
---@field n integer
---@type HasBoth|HasOne
local ab
local got = ab.extra
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "unknown");
        assert_eq!(out.diags, Vec::new());
    }

    #[test]
    fn field_read_on_a_union_of_inferred_shapes_unions_the_results() {
        let src = "\
local u
if SOME_GLOBAL then
  u = { n = 1 }
else
  u = { n = \"s\" }
end
local got = u.n
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "1|\"s\"");
    }

    #[test]
    fn integer_indexing_reads_through_arrays_indexers_and_named_classes() {
        let src = "\
---@class Grid
---@field [integer] string

---@type number[]
local arr
---@type table<integer, boolean>
local map
---@type Grid
local grid
local k = 1 + 1
local from_arr = arr[k]
local from_map = map[k]
local from_grid = grid[k]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "from_arr").to_string(), "number");
        assert_eq!(binding_ty(&out, "from_map").to_string(), "boolean");
        assert_eq!(binding_ty(&out, "from_grid").to_string(), "string");
    }

    #[test]
    fn integer_indexing_unions_across_a_union_receiver() {
        let src = "\
---@type number[]
local nums
---@type string[]
local strs
local xs
if SOME_GLOBAL then
  xs = nums
else
  xs = strs
end
local k = 1 + 1
local elem = xs[k]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "elem").to_string(), "number|string");
    }

    #[test]
    fn integer_indexing_reads_an_inferred_integer_indexer() {
        // A dynamically-keyed write generalizes the key to `integer`; the
        // matching read comes back with the written value type, never `any`.
        let src = "\
local t = {}
local k = 1 + 1
t[k] = \"v\"
local got = t[k]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "\"v\"");
    }

    #[test]
    fn dynamic_key_reads_union_the_values_of_a_named_class() {
        // A non-numeric dynamic key falls back to the `pairs` value union.
        let src = "\
---@class Bag
---@field a integer
---@field b string

---@type Bag
local bag
---@type string|boolean
local key
local got = bag[key]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "got").to_string(), "integer|string");
    }

    #[test]
    fn pairs_over_a_union_receiver_unions_keys_and_values() {
        let src = "\
---@type table<string, integer>
local si
---@type table<boolean, number>
local bn
local m
if SOME_GLOBAL then
  m = si
else
  m = bn
end
for k, v in pairs(m) do
  local kk = k
  local vv = v
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "kk").to_string(), "string|boolean");
        assert_eq!(binding_ty(&out, "vv").to_string(), "integer|number");
    }

    #[test]
    fn pairs_over_a_named_class_resolves_through_the_name() {
        let src = "\
---@class Named
---@field a integer
---@field b integer

---@type Named
local n
for k, v in pairs(n) do
  local nk = k
  local nv = v
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "nk").to_string(), "string");
        assert_eq!(binding_ty(&out, "nv").to_string(), "integer");
    }

    // --- assignment targets ------------------------------------------------

    #[test]
    fn global_assignment_unions_across_writes() {
        let src = "\
GLOBAL_SLOT = 1
GLOBAL_SLOT = \"two\"
local read = GLOBAL_SLOT
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "read").to_string(), "1|\"two\"");
    }

    #[test]
    fn numeric_index_assignment_extends_the_array_part() {
        let src = "\
local t = {}
t[1] = \"a\"
t[2] = \"b\"
local first = t[1]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "first").to_string(), "\"a\"|\"b\"");
    }

    #[test]
    fn upvalue_assignment_unions_with_the_outer_type() {
        // An assignment through a closure cannot be treated as a
        // flow-sensitive overwrite of the outer binding: the two possible
        // values join.
        let src = "\
local slot = 1
local function set()
  slot = \"text\"
end
set()
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "slot").to_string(), "1|\"text\"");
    }

    #[test]
    fn assert_narrows_its_first_argument_to_the_truthy_part() {
        let src = "\
---@type string|nil
local s
local sure = assert(s)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "sure").to_string(), "string");
    }

    // --- `undefined-field` provability guards ------------------------------

    #[test]
    fn a_class_with_an_indexer_never_proves_a_field_absent() {
        // Dynamic access is *declared*, so any string key is admissible: no
        // LB0306, and the read stays `unknown` rather than being invented.
        let src = "\
---@class Dynamic
---@field [string] integer
local D = {}
function D.probe()
  return D.whatever
end
";
        let out = outcome(src);
        assert_eq!(out.diags, Vec::new());
    }

    #[test]
    fn an_index_metamethod_function_suspends_provability() {
        // `__index = function(...)` can synthesise any key, so an absent field
        // is not provably undefined.
        let src = "\
local Proxy = {}
local obj = setmetatable({}, { __index = function(_, k) return k end })
local got = obj.anything
";
        let out = outcome(src);
        assert_eq!(out.diags, Vec::new());
        assert_eq!(binding_ty(&out, "got").to_string(), "unknown");
    }

    #[test]
    fn an_untracked_metatable_suspends_provability() {
        // `setmetatable(t, <unknown>)`: the metatable is not a tracked shape,
        // so nothing about `t`'s key set is provable any more.
        let src = "\
---@class Guarded
---@field known integer
local G = {}
function G.build()
  local o = setmetatable({ known = 1 }, SOME_GLOBAL)
  return o.unknown_member
end
";
        let out = outcome(src);
        assert_eq!(out.diags, Vec::new());
    }

    #[test]
    fn setmetatable_with_no_arguments_is_unknown() {
        let src = "\
local nothing = setmetatable()
local scalar = setmetatable(\"str\", {})
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "nothing").to_string(), "unknown");
        assert_eq!(binding_ty(&out, "scalar").to_string(), "\"str\"");
    }

    #[test]
    fn setmetatable_merges_array_and_indexer_parts_into_the_instance() {
        // The constructed table carries an array part and a dynamic indexer;
        // both must survive the merge into the class's shared instance shape.
        let src = "\
local Bag = {}
Bag.__index = Bag
function Bag.new()
  local o = { 1, 2 }
  o[SOME_KEY] = \"v\"
  return setmetatable(o, Bag)
end
local b = Bag.new()
local first = b[1]
local dynamic = b[SOME_OTHER]
";
        let out = outcome(src);
        // The array part merged through `setmetatable` into the instance...
        assert_eq!(binding_ty(&out, "first").to_string(), "1|2");
        // ...and so did the dynamic indexer (a non-numeric key reads the
        // value union, never `any`).
        assert_eq!(binding_ty(&out, "dynamic").to_string(), "1|2|\"v\"");
    }

    // --- annotated function-expression locals -------------------------------

    #[test]
    fn a_signature_on_a_function_expression_local_is_authoritative() {
        // `---@param`/`---@return` on `local f = function(...)` binds `f` at
        // the declared function type (not the inferred body), so the call
        // result comes from the annotation.
        let src = "\
---@param x number
---@return string
local f = function(x)
  return 1
end
local r = f(1)
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "f").to_string(), "fun(x: number): string");
        assert_eq!(binding_ty(&out, "r").to_string(), "string");
    }

    // --- contextual seeding descends into control flow ---------------------

    #[test]
    fn contextual_return_seeding_reaches_returns_inside_every_block() {
        // `collect_returns` must find the `return`s nested in `if`, `while`,
        // `repeat`, `for` and `do` — each returned lambda then takes the
        // expected `fun(s: string)` type, so feeding its parameter to a
        // `number` slot is a mismatch. Five nested returns => five findings.
        let src = "\
---@param outer fun(): fun(s: string)
local function reg(outer) end
---@param n number
local function wantn(n) end
reg(function()
  if SOME_GLOBAL then
    return function(s) wantn(s) end
  end
  while SOME_GLOBAL do
    return function(s) wantn(s) end
  end
  repeat
    return function(s) wantn(s) end
  until true
  for _ = 1, 2 do
    return function(s) wantn(s) end
  end
  do
    return function(s) wantn(s) end
  end
end)
";
        assert_eq!(strict_codes(src), vec!["LB0300"; 5]);
    }

    // --- generic-for iterator shapes ---------------------------------------

    #[test]
    fn next_style_iteration_types_both_variables() {
        let src = "\
---@type table<string, integer>
local m
for k, v in next, m do
  local kk = k
  local vv = v
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "kk").to_string(), "string");
        assert_eq!(binding_ty(&out, "vv").to_string(), "integer");
    }

    #[test]
    fn unrecognized_iterator_forms_leave_the_variables_unknown() {
        // Only the `ipairs`/`pairs`/`next` global forms are modeled; anything
        // else stays `unknown` rather than being guessed at.
        let src = "\
local custom = {}
local function make() end
for a in custom do
  local from_name = a
end
for b in custom.iter() do
  local from_field = b
end
for c in make() do
  local from_local = c
end
for d in pairs() do
  local from_argless = d
end
";
        let out = outcome(src);
        for name in ["from_name", "from_field", "from_local", "from_argless"] {
            assert_eq!(binding_ty(&out, name).to_string(), "unknown", "{name}");
        }
    }

    // --- vararg expansion ---------------------------------------------------

    #[test]
    fn vararg_expands_to_fill_every_requested_slot() {
        let src = "\
local function spread(...)
  local a, b, c = ...
  return a
end
";
        let out = outcome(src);
        for name in ["a", "b", "c"] {
            assert_eq!(binding_ty(&out, name).to_string(), "unknown", "{name}");
        }
    }

    // --- narrowing: the remaining predicates --------------------------------

    #[test]
    fn literal_equality_narrows_to_the_literal() {
        let src = "\
---@type \"a\"|\"b\"|\"c\"
local tag
if tag == \"a\" then
  local is_a = tag
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "is_a").to_string(), "\"a\"");
    }

    #[test]
    fn literal_inequality_removes_that_member() {
        let src = "\
---@type \"a\"|\"b\"|\"c\"
local tag
if tag ~= \"a\" then
  local not_a = tag
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "not_a").to_string(), "\"a\"|\"b\"|\"c\"");
    }

    #[test]
    fn literal_equality_on_an_unknown_adopts_the_literal_type() {
        let src = "\
local function f(v)
  if v == 42 then
    local fixed = v
  end
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "fixed").to_string(), "42");
    }

    #[test]
    fn negated_type_call_keeps_the_other_members() {
        let src = "\
---@type string|integer
local v
if type(v) ~= \"string\" then
  local not_string = v
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "not_string").to_string(), "string|integer");
    }

    #[test]
    fn nil_equality_narrows_both_ways_on_a_plain_value() {
        // A non-optional value compared to `nil`: the positive branch is
        // statically impossible and degrades to `nil`, the negative keeps it.
        let src = "\
---@type string
local s
if s == nil then
  local impossible = s
else
  local present = s
end
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "impossible").to_string(), "nil");
        assert_eq!(binding_ty(&out, "present").to_string(), "string");
    }

    // --- unary operator overloads -------------------------------------------

    #[test]
    fn bnot_and_unm_consult_declared_operators() {
        let src = "\
---@class Bits
---@operator bnot: Bits
---@operator unm: string

---@type Bits
local b
local flipped = ~b
local negated = -b
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "flipped").to_string(), "Bits");
        assert_eq!(binding_ty(&out, "negated").to_string(), "string");
    }

    #[test]
    fn unary_operators_on_numbers_ignore_overloads() {
        let src = "\
local i = ~5
local n = -1.5
local ni = -7
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "i").to_string(), "integer");
        assert_eq!(binding_ty(&out, "n").to_string(), "number");
        assert_eq!(binding_ty(&out, "ni").to_string(), "integer");
    }

    #[test]
    fn unary_operators_without_an_overload_stay_unknown() {
        let src = "\
---@class Bare
---@type Bare
local b
local flipped = ~b
local negated = -b
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "flipped").to_string(), "unknown");
        assert_eq!(binding_ty(&out, "negated").to_string(), "unknown");
    }

    // --- indexer key generalisation ------------------------------------------

    #[test]
    fn dynamic_write_keys_generalize_to_their_base_type() {
        // A computed key widens to its base type so the indexer list stays
        // small; an unknown key becomes `any`. Never a bare `table`.
        let src = "\
---@type \"n\"
local skey
---@type 1.5
local fkey
---@type 2
local ikey
local strs = {}
strs[skey] = 1
local floats = {}
floats[fkey] = false
local ints = {}
ints[ikey] = \"i\"
local bools = {}
bools[true] = \"flag\"
local dyn = {}
dyn[SOME_GLOBAL] = 0
local by_bool = bools[true]
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "strs").to_string(), "{ [string]: 1 }");
        assert_eq!(
            binding_ty(&out, "floats").to_string(),
            "{ [number]: false }"
        );
        assert_eq!(binding_ty(&out, "ints").to_string(), "{ [integer]: \"i\" }");
        assert_eq!(
            binding_ty(&out, "bools").to_string(),
            "{ [boolean]: \"flag\" }"
        );
        assert_eq!(binding_ty(&out, "dyn").to_string(), "{ [any]: 0 }");
        assert_eq!(binding_ty(&out, "by_bool").to_string(), "\"flag\"");
    }

    #[test]
    fn a_parenthesised_string_key_is_the_same_slot_as_a_named_field() {
        let src = "\
local t = {}
t[(\"name\")] = 1
local by_string = t[(\"name\")]
local by_field = t.name
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "t").to_string(), "{ name: 1 }");
        assert_eq!(binding_ty(&out, "by_string").to_string(), "1");
        assert_eq!(binding_ty(&out, "by_field").to_string(), "1");
    }

    // --- number-literal rendering ---------------------------------------------

    #[test]
    fn float_literals_keep_a_decimal_point_or_exponent() {
        // An integral-valued float must not masquerade as an integer literal.
        let src = "\
local plain = 2.0
local exp = 1e30
local huge = 1e400
local frac = 0.5
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "plain").to_string(), "2.0");
        assert_eq!(
            binding_ty(&out, "exp").to_string(),
            "1000000000000000000000000000000.0"
        );
        assert_eq!(binding_ty(&out, "huge").to_string(), "inf");
        assert_eq!(binding_ty(&out, "frac").to_string(), "0.5");
    }

    // --- `---@cast -T` on inference-side unions ---------------------------------

    #[test]
    fn cast_minus_removes_a_member_from_an_inferred_union() {
        let src = "\
local v
if SOME_GLOBAL then
  v = 1
else
  v = \"s\"
end
---@cast v -integer
local narrowed = v
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "narrowed").to_string(), "1|\"s\"");
    }

    #[test]
    fn cast_minus_that_removes_everything_degrades_to_unknown() {
        let src = "\
---@type string
local s
---@cast s -string
local gone = s
";
        let out = outcome(src);
        assert_eq!(binding_ty(&out, "gone").to_string(), "unknown");
    }
}
