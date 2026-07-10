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
//! `---@class`/`.luab` struct (their declarations govern instead). Unknown
//! stays `unknown` — never `any`.

use std::collections::{BTreeMap, HashMap, HashSet};

use luabox_diag::{Code, Diagnostic, Label, Severity, Span};
use luabox_hir::{
    BinOp, Binding, BindingId, BindingKind, Block, Body, BodyId, Expr, ExprId, HirId, IfBranch,
    Literal, LoweredFile, Resolution, Stmt, StmtId, TableEntry, UnOp,
};

use crate::env::TypeEnv;
use crate::ty::{FieldTy, FunctionTy, ParamTy, TableTy, Ty};

/// Diagnostic code emitted here (block `LB03xx` — Semantics).
const FIELD_NOT_FOUND: u16 = 306;

/// A byte range key, matching the annotation checker's convention.
type Key = (usize, usize);

/// What inference hands back to the checker.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Reified expression types keyed by byte range. Only expressions the
    /// annotation checker cannot type itself are published; `unknown`
    /// results are omitted.
    pub(crate) expr_types: HashMap<Key, Ty>,
    /// Inference's own diagnostics (`LB0306`).
    pub(crate) diags: Vec<Diagnostic>,
    /// Final reified type of every binding (test/debug surface — the
    /// "bare `table` never appears" acceptance check walks this).
    #[cfg_attr(not(test), allow(dead_code, reason = "consumed by the test surface"))]
    pub(crate) binding_types: Vec<(String, Ty)>,
}

/// Run inference over one lowered file.
pub(crate) fn run(hir: &LoweredFile, env: &TypeEnv, file: &str, strict: bool) -> Outcome {
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
        pass: 0,
        shapes: Vec::new(),
        shape_of_expr: HashMap::new(),
        instances: HashMap::new(),
        funcs: HashMap::new(),
        state: HashMap::new(),
        declared: HashSet::new(),
        globals: HashMap::new(),
        expr_types: HashMap::new(),
        diags: Vec::new(),
        memo: HashMap::new(),
        reify_stack: Vec::new(),
    };
    infer.run_pass();
    infer.pass = 1;
    infer.run_pass();

    let mut binding_types = Vec::new();
    for (id, binding) in hir.bindings() {
        if let Some(ity) = infer.state.get(&id).cloned() {
            let ty = infer.reify(&ity);
            binding_types.push((binding.name.clone(), ty));
        }
    }
    Outcome {
        expr_types: infer.expr_types,
        diags: infer.diags,
        binding_types,
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
    /// The `---@class`/`.luab` struct name bound to this table's declaration,
    /// when any. Field lookups consult the declaration; `LB0306` defers to
    /// the declaration's own diagnostics.
    declared: Option<String>,
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
    Absent {
        provable: bool,
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
    /// 0 = build shapes, 1 = emit diagnostics + publish types.
    pass: u8,
    shapes: Vec<ShapeData>,
    /// Constructor site → shape, so pass 2 reuses pass 1's identities.
    shape_of_expr: HashMap<(BodyId, ExprId), usize>,
    /// Carrier shape → its shared instance shape.
    instances: HashMap<usize, usize>,
    funcs: HashMap<BodyId, FuncData>,
    /// Flow state: binding → current inferred type (flat across bodies).
    state: HashMap<BindingId, ITy>,
    /// Bindings with an authoritative annotated type (never overwritten).
    declared: HashSet<BindingId>,
    globals: HashMap<String, ITy>,
    expr_types: HashMap<Key, Ty>,
    diags: Vec<Diagnostic>,
    memo: HashMap<usize, Ty>,
    reify_stack: Vec<usize>,
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

    fn report_absent(&mut self, body: BodyId, expr: ExprId, name: &str) {
        if self.pass != 1 {
            return;
        }
        let Some((start, end)) = self.expr_range(body, expr) else {
            return;
        };
        self.diags.push(
            Diagnostic::new(
                Code::new(FIELD_NOT_FOUND),
                self.severity,
                format!("cannot find field `{name}` on this table"),
            )
            .with_label(Label::primary(
                Span::new(self.file, start..end),
                format!("`{name}` is not defined by the table's constructor, assignments, or metatable chain"),
            )),
        );
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
            has_return_annotation: false,
            overloads: Vec::new(),
        }
    }

    /// Reify a shape: own fields plus the flattened `__index` chain
    /// (nearest definition wins), skipping `__`-metafields. Cycles cut off
    /// with the catch-all table shape.
    fn reify_shape(&mut self, id: usize) -> Ty {
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
        let mut cur = Some(id);
        let mut seen: HashSet<usize> = HashSet::new();
        while let Some(s) = cur {
            if !seen.insert(s) {
                break;
            }
            if let Some(ity) = self.shapes[s].fields.get(name) {
                return Lookup::Found(ity.clone());
            }
            let data = &self.shapes[s];
            if data.escaped || data.meta_unknown || !data.indexers.is_empty() {
                provable = false;
            }
            if let Some(class) = data.declared.clone() {
                // Declared carriers are governed by their declaration:
                // fields resolve through it, absences defer to its own
                // diagnostics (LB0303 / LB2002).
                provable = false;
                if let Some(shape) = self.env.class_shape(&class)
                    && let Some(field) = shape.fields.get(name)
                {
                    let ty = if field.optional {
                        field.ty.clone().optional()
                    } else {
                        field.ty.clone()
                    };
                    return Lookup::Found(ITy::Ty(ty));
                }
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
        Lookup::Absent { provable }
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
                Lookup::Absent { provable: false }
            }
            Ty::Named(class) => match self.env.resolve_named(class) {
                Some(resolved) => self.lookup_ty_field(&resolved, name),
                None => Lookup::Opaque,
            },
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
                self_ty.cloned().unwrap_or_else(ITy::unknown)
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
            self.eval_values(body, init, Some(names.len()))
        };

        for (i, local) in names.iter().enumerate() {
            if let Some(ty) = declared_tys.as_ref().and_then(|t| t.get(i)) {
                self.declared.insert(local.binding);
                self.state.insert(local.binding, ITy::Ty(ty.clone()));
            } else if i == 0 && sig.is_some() {
                self.declared.insert(local.binding);
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
                self.shapes[*id].declared = Some(name);
            }
        }
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
            self.walk_body(fn_body, sig.as_ref(), self_ty.as_ref());
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
        let ity = self.eval_inner(body, expr);
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
                    UnOp::Len => ITy::Ty(Ty::Integer),
                    UnOp::Neg => {
                        let ty = self.reify(&operand_ty);
                        if integerish(&ty) {
                            ITy::Ty(Ty::Integer)
                        } else if numberish(&ty) {
                            ITy::Ty(Ty::Number)
                        } else {
                            ITy::unknown()
                        }
                    }
                    UnOp::BNot => {
                        let ty = self.reify(&operand_ty);
                        if numberish(&ty) {
                            ITy::Ty(Ty::Integer)
                        } else {
                            ITy::unknown()
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
                        Lookup::Found(ity) => ity,
                        Lookup::Absent { provable } => {
                            if provable {
                                self.report_absent(body, expr, &name);
                            }
                            ITy::unknown()
                        }
                        Lookup::Opaque => ITy::unknown(),
                    }
                }
                None => ITy::unknown(),
            },
            Expr::Literal(Literal::Number(_)) => self.elem_ty(&recv),
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
                    ITy::unknown()
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
                    ITy::unknown()
                }
            }
            BinOp::Div | BinOp::Pow => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if numberish(&lt) && numberish(&rt) {
                    ITy::Ty(Ty::Number)
                } else {
                    ITy::unknown()
                }
            }
            BinOp::BAnd | BinOp::BOr | BinOp::BXor | BinOp::Shl | BinOp::Shr => {
                let lt = self.reify(&l);
                let rt = self.reify(&r);
                if numberish(&lt) && numberish(&rt) {
                    ITy::Ty(Ty::Integer)
                } else {
                    ITy::unknown()
                }
            }
        }
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

        let callee_ity = self.eval(body, callee);
        for &arg in &args {
            let ity = self.eval(body, arg);
            self.mark_escaped(&ity);
        }
        self.returns_of(&callee_ity)
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
        for &arg in &args {
            let ity = self.eval(body, arg);
            self.mark_escaped(&ity);
        }
        match self.lookup_field(&recv, &method) {
            Lookup::Found(f) => self.returns_of(&f),
            Lookup::Absent { provable } => {
                if provable {
                    self.report_absent(body, expr, &method);
                }
                (vec![ITy::unknown()], true)
            }
            Lookup::Opaque => (vec![ITy::unknown()], true),
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
                        let (rets, open) = self.eval_call(body, expr);
                        self.publish_call(body, expr, &rets, open);
                        Self::push_expansion(&mut values, rets, open, want);
                    }
                    Expr::MethodCall { .. } => {
                        let (rets, open) = self.eval_method_call(body, expr);
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
mod tests {
    use luabox_syntax::lua::{Dialect, parse};

    use super::*;
    use crate::{Strictness, check_file};

    fn outcome(source: &str) -> Outcome {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let env = TypeEnv::build(&parsed);
        let lowered = luabox_hir::lower(&parsed);
        run(&lowered, &env, "test.lua", true)
    }

    fn binding_ty(outcome: &Outcome, name: &str) -> Ty {
        outcome
            .binding_types
            .iter()
            .find(|(n, _)| n == name)
            .map_or_else(|| panic!("no binding named `{name}`"), |(_, ty)| ty.clone())
    }

    fn codes(source: &str, strictness: Strictness) -> Vec<String> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        check_file(&parsed, "test.lua", strictness)
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
        let diags = check_file(&parsed, "test.lua", Strictness::Warn);
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
    fn class_bound_carrier_defers_to_declaration() {
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
        // `self.size` types from the class; `self.whatever` is governed by
        // the declaration, not LB0306.
        assert_eq!(strict_codes(&src), Vec::<String>::new());
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
        for (name, ty) in &out.binding_types {
            assert_ne!(ty.to_string(), "table", "binding `{name}` is bare table");
        }
    }
}
