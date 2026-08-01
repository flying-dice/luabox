//! `metatable-without-index` (suspicious): `setmetatable(t, C)` where the
//! `---@class` carrier `C` never gets an `__index`.
//!
//! # Why this rule exists
//!
//! The checker resolves `o:m()` through a `---@class` carrier even when the
//! metatable chain has no `__index` link (#33). That is deliberate luals
//! parity — luals folds carrier attachments into the class off the *carrier
//! binding*, with no metatable reasoning — and it is recorded in
//! `docs/03-reference/02-limitations.md`.
//!
//! Parity has a cost, though: the canonical carrier program
//!
//! ```lua
//! ---@class Counter
//! local Counter = {}
//! function Counter:value() return self.n end
//! local c = setmetatable({ n = 1 }, Counter)
//! print(c:value())   --> attempt to call a nil value (method 'value')
//! ```
//!
//! crashes in every reference Lua: `setmetatable` installs `Counter` as the
//! metatable, and instance lookup consults `metatable.__index`, which is
//! `nil`. luabox used to report `LB0306` here and stopped when #33 landed
//! (Shockwave round 2 asked whether the trade was deliberate: it is).
//!
//! So the *checker* keeps parity and the *linter* covers the runtime gap.
//! That split is the point — a lint is suppressible and configurable, which
//! is what a rule luals does not have should be.

use std::collections::{HashMap, HashSet};

use luabox_diag::Code;
use luabox_hir::{
    BindingId, BindingKind, Body, BodyId, Expr, ExprId, HirId, Literal, Resolution, Stmt,
    TableEntry,
};

use crate::context::{LintContext, binding_of};
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// The metafield instance lookup consults.
const INDEX: &str = "__index";

/// The prefix every metamethod name carries (`__call`, `__tostring`, `__add`,
/// `__mode`, …). A carrier declaring one of these is a metatable with a
/// deliberate purpose that is not instance lookup.
const METAFIELD_PREFIX: &str = "__";

/// The receiver name a function attached to the carrier reaches an instance
/// through — implicit under `function C:m()`, written out under
/// `function C.__call(self, …)`. Both spellings count (see
/// [`InstanceUses::seed_attached_self`]).
const SELF: &str = "self";

/// `setmetatable(<expr>, C)` where `C` is a `---@class` carrier declared in
/// this file and nothing ever assigns `C.__index` — so every `instance:m()`
/// through that metatable is `attempt to call a nil value` at runtime, even
/// though the checker (deliberately, for luals parity) resolves it.
///
/// Deliberately conservative — it fires only when all of these hold:
///
/// - the callee is the real global `setmetatable`, not a shadowing local;
/// - the second argument is a bare name resolving to a local/upvalue binding
///   that carries a `---@class` annotation in this file. A global carrier, a
///   carrier reached through `require`, a table literal, a call result or any
///   other expression is *unknown* and stays silent;
/// - no `__index` is written to that binding anywhere in the file, in any
///   spelling this pass can see (see [`Carriers::index_is_settled`]);
/// - the carrier is not an *operator table*: one that declares some other
///   metafield (`__call`, `__tostring`, `__add`, `__mode`, `__gc`, `__name`,
///   …) with **no observed instance-side use** (see
///   [`Carriers::is_operator_table`] and [`Carriers::has_instance_use`]).
///   `Vec.__tostring` on a carrier nothing is ever called on is a deliberate
///   operator metatable and there is nothing for a missing `__index` to
///   break; the same `__tostring` on a carrier some `v:length()` in the file
///   actually reaches is a class that happens to overload an operator, and
///   that call still crashes. The gate is behavioural, not structural:
///   *declaring* `function Cache:reset()` next to `Cache.__mode = "k"` is not
///   a lookup, and a `Cache` nothing is ever invoked on runs fine (Shockwave
///   round 6 measured eight such carriers).
///
/// It is **in-file only** — the carrier, the `__index` write and the
/// `setmetatable` call must all be in the file being linted — and, being a
/// lint, it never affects `luabox check`'s exit code. Both bounds are
/// recorded in `docs/03-reference/02-limitations.md`.
///
/// There is no fix attached: `C.__index = C` is the usual repair, but the
/// right line to insert depends on where the carrier is declared and whether
/// the class means to inherit, so it is offered as prose rather than an edit.
pub struct MetatableWithoutIndex;

impl Rule for MetatableWithoutIndex {
    fn id(&self) -> &'static str {
        "metatable-without-index"
    }

    fn tier(&self) -> Tier {
        // Same sharpness as `undefined-global`: a call that will fail at
        // runtime, found without guessing. Warn by default, not deny — the
        // checker stays silent here on purpose, and a lint that fails the
        // build on a parity decision would be the wrong trade.
        Tier::Suspicious
    }

    fn code(&self) -> Code {
        Code::new(510)
    }

    fn description(&self) -> &'static str {
        "`setmetatable` with a `---@class` carrier that has no `__index`"
    }

    fn check(&self, ctx: &LintContext<'_>) -> Vec<LintDiagnostic> {
        // A `---@meta` file declares a surface; it never runs, so there is no
        // runtime lookup to fail. (`global-write`/`unused-local` bow out of
        // defs files for the same reason, ticket #76.)
        if ctx.facts.is_meta() {
            return Vec::new();
        }
        let carriers = Carriers::build(ctx);
        let mut out = Vec::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (_, expr) in body.exprs() {
                // One diagnostic per `setmetatable` call site, by
                // construction: each call expression is visited once and
                // pushes at most one finding, so a carrier used as a
                // metatable twice reports twice — once per site — and never
                // twice for the same site.
                let Some(meta) = setmetatable_arg(ctx, body_id, body, expr) else {
                    continue;
                };
                let Some(binding) = binding_of(ctx.lowered.resolution(HirId::expr(body_id, meta)))
                else {
                    continue;
                };
                let Some(class) = ctx.facts.class_carrier(binding) else {
                    continue;
                };
                // Aliases share the carrier's identity: everything the pass
                // learned through `local mt = C` was recorded against `C`.
                let carrier = carriers.root(binding);
                if carriers.index_is_settled(carrier) {
                    continue;
                }
                // An operator metatable has no instance lookup to break — so
                // a carrier that declares a metafield only fires once this
                // file actually *reaches* a method on an instance of it. A
                // declared-but-never-invoked `function Cache:reset()` is not
                // a lookup; `c:value()` on a value derived from
                // `setmetatable(_, Cache)` is (see [`InstanceUses`]).
                //
                // Carriers with no metafield at all never reach this arm:
                // that region is the rule's original, measured-clean
                // behaviour and is deliberately left structural.
                if carriers.is_operator_table(carrier) && !carriers.has_instance_use(carrier) {
                    continue;
                }
                let Some(range) = ctx.node_range(HirId::expr(body_id, meta)) else {
                    continue;
                };
                let name = ctx.text(&range).to_owned();
                out.push(
                    LintDiagnostic::new(
                        range,
                        format!(
                            "`{name}` is used as a metatable but never sets `{INDEX}`, so \
                             instance lookups will not reach `{class}`"
                        ),
                    )
                    .with_note(format!(
                        "`setmetatable(t, {name})` makes `{name}` the metatable; a method call \
                         on `t` resolves through `{name}.{INDEX}`, which is `nil` here — so any \
                         `t:method()` fails at runtime with `attempt to call a nil value`"
                    ))
                    .with_note(format!(
                        "add `{name}.{INDEX} = {name}` after the declaration"
                    )),
                );
            }
        }
        out
    }
}

/// The metatable argument of a real `setmetatable(t, C)` call — the second of
/// exactly two arguments to the *global* `setmetatable`, not a shadowing
/// local. `None` for every other expression.
///
/// Shared by [`MetatableWithoutIndex::check`], which reports on the call, and
/// [`InstanceUses`], which treats it as the origin of an instance value: both
/// must agree on what counts as a construction, or the behavioural gate would
/// be answering a different question from the one the rule asks.
fn setmetatable_arg(
    ctx: &LintContext<'_>,
    body_id: BodyId,
    body: &Body,
    expr: &Expr,
) -> Option<ExprId> {
    let Expr::Call { callee, args } = expr else {
        return None;
    };
    if !matches!(body.expr(*callee), Expr::Name(n) if n == "setmetatable") {
        return None;
    }
    if !matches!(
        ctx.lowered.resolution(HirId::expr(body_id, *callee)),
        Some(Resolution::Global(name)) if name == "setmetatable"
    ) {
        return None;
    }
    match args.as_slice() {
        [_, meta] => Some(*meta),
        _ => None,
    }
}

/// Which carrier bindings the rule must not fire on: those whose `__index` is
/// settled, and those that are metatables for some purpose other than
/// instance lookup.
///
/// Both are conservative suppressions. A rule that guesses is a rule that
/// cries wolf, and this one has no `LB0306` behind it to correct a bad guess.
struct Carriers {
    aliases: Aliases,
    settled: HashSet<BindingId>,
    operator_tables: HashSet<BindingId>,
    instance_uses: HashSet<BindingId>,
}

impl Carriers {
    /// Every spelling of "`__index` is dealt with" the HIR can show:
    ///
    /// - `C.__index = …` / `C["__index"] = …` — an assignment to the field,
    ///   whatever the right-hand side and wherever it sits relative to the
    ///   `setmetatable` call (Lua evaluates it before the lookup runs either
    ///   way, and a file-order rule would be a false-positive machine);
    /// - `local C = { __index = … }` / `{ ["__index"] = … }` — the key in the
    ///   carrier's own constructor;
    /// - `C[k] = …` with a key this pass cannot evaluate, and `rawset(C, k,
    ///   …)` with a key it cannot evaluate or that *is* `__index` — the write
    ///   might be the one that matters, so the carrier is treated as settled.
    ///   `rawset(C, "n", 0)` is not: `index_key` already reads literal keys,
    ///   and settling on any `rawset` over-suppressed (Shockwave round 4);
    /// - a write *through an alias*. `local mt = C` binds a second name to the
    ///   same table, so every write through `mt` is a write to `C` — the pass
    ///   propagates carrier identity along local aliases ([`Aliases`]) and
    ///   applies exactly the rules above to the root carrier. `mt.__index =
    ///   mt` settles `C`; `mt[k] = v` settles it; `mt.n = 1` does not, and
    ///   `function mt.__tostring(v)` marks `C` an operator table. Wave 15
    ///   settled on the *alias binding itself*, with no write required, which
    ///   silenced `local mt = C; print(type(mt))` — a program that still
    ///   crashes (Shockwave round 5);
    /// - `C = <anything>` — the binding is reassigned, so the value the
    ///   `---@class` annotation described is not necessarily what reaches
    ///   `setmetatable`. Reassignment is *not* followed through aliases:
    ///   `mt = {}` rebinds the name `mt`, it does not touch `C`'s table.
    ///
    /// Separately it records the **operator tables**: carriers that declare
    /// some *other* metafield (`__call`, `__tostring`, `__add`, `__mode`,
    /// `__gc`, `__name`, …). Those, and only those, are then put to the
    /// behavioural question [`InstanceUses`] answers — is a method ever
    /// actually reached on an instance of this carrier?
    ///
    /// Wave 16 asked a *structural* question instead ("does the carrier
    /// declare a colon method?"), and Shockwave round 6 measured eight
    /// carriers where the answer was yes and the program ran fine: declaring
    /// `function Cache:reset()` beside `Cache.__mode = "k"` is not a lookup.
    /// The colon-method set is therefore gone; what replaced it is in
    /// [`InstanceUses`].
    fn build(ctx: &LintContext<'_>) -> Self {
        let aliases = Aliases::build(ctx);
        let mut settled: HashSet<BindingId> = HashSet::new();
        let mut operator_tables: HashSet<BindingId> = HashSet::new();
        for (body_id, body) in ctx.lowered.bodies() {
            let resolve =
                |expr: ExprId| binding_of(ctx.lowered.resolution(HirId::expr(body_id, expr)));
            // The table a write through this expression lands on, alias chains
            // followed to their root.
            let carrier_of = |expr: ExprId| resolve(expr).map(|b| aliases.root(b));

            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Assign { targets, .. } => {
                        for &target in targets {
                            match body.expr(target) {
                                // `C.__index = …`, `C.__call = …`, `C[k] = …`
                                // — and `function C.__tostring(v) … end` and
                                // `function C:m() … end`, which lower to
                                // exactly this shape.
                                Expr::Index { base, index, .. } => {
                                    if !matches!(body.expr(*base), Expr::Name(_)) {
                                        continue;
                                    }
                                    let Some(carrier) = carrier_of(*base) else {
                                        continue;
                                    };
                                    match key_kind(index_key(body.expr(*index))) {
                                        KeyKind::Index => {
                                            settled.insert(carrier);
                                        }
                                        KeyKind::Metafield => {
                                            operator_tables.insert(carrier);
                                        }
                                        // An ordinary key says nothing about
                                        // instance lookup on its own — what
                                        // it is *used* for is [`InstanceUses`]
                                        // question.
                                        KeyKind::Plain => {}
                                    }
                                }
                                // `C = <anything>` — the annotated value is
                                // no longer necessarily the one in play. Not
                                // alias-resolved: this rebinds a name.
                                Expr::Name(_) => settled.extend(resolve(target)),
                                _ => {}
                            }
                        }
                    }
                    // `local C = { __index = … }` — the key in the carrier's
                    // own constructor. Names and values pair positionally,
                    // exactly as Lua does. (`local mt = C` is an alias and is
                    // handled by [`Aliases`], not here.)
                    Stmt::Local { names, init } => {
                        for (name, &value) in names.iter().zip(init) {
                            let Expr::Table { entries } = body.expr(value) else {
                                continue;
                            };
                            let kinds = || entries.iter().filter_map(|e| entry_kind(body, e));
                            if kinds().any(|kind| kind == KeyKind::Index) {
                                settled.insert(name.binding);
                            }
                            if kinds().any(|kind| kind == KeyKind::Metafield) {
                                operator_tables.insert(name.binding);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // `rawset(C, k, …)` bypasses metamethods but still writes the
            // field — when `k` could be `__index`.
            for (_, expr) in body.exprs() {
                let Expr::Call { callee, args } = expr else {
                    continue;
                };
                if !matches!(body.expr(*callee), Expr::Name(n) if n == "rawset") {
                    continue;
                }
                let Some(carrier) = args.first().and_then(|&t| carrier_of(t)) else {
                    continue;
                };
                // No key argument at all is malformed source; read it the way
                // an unreadable key is read.
                let kind = args
                    .get(1)
                    .map_or(KeyKind::Index, |&key| key_kind(index_key(body.expr(key))));
                match kind {
                    KeyKind::Index => {
                        settled.insert(carrier);
                    }
                    KeyKind::Metafield => {
                        operator_tables.insert(carrier);
                    }
                    KeyKind::Plain => {}
                }
            }
        }
        // The behavioural gate only ever gates the operator-table arm, so it
        // is only ever paid for by a file that has one. A file with no
        // metafield-declaring carrier skips the traversal entirely.
        let instance_uses = if operator_tables.is_empty() {
            HashSet::new()
        } else {
            InstanceUses::build(ctx, &aliases)
        };
        Self {
            aliases,
            settled,
            operator_tables,
            instance_uses,
        }
    }

    /// The table a binding names: itself, or — for `local mt = C` — the
    /// carrier it aliases.
    fn root(&self, binding: BindingId) -> BindingId {
        self.aliases.root(binding)
    }

    /// Whether this carrier's `__index` is assigned, or written in a way this
    /// pass cannot read — either way the rule stays silent.
    fn index_is_settled(&self, binding: BindingId) -> bool {
        self.settled.contains(&binding)
    }

    /// Whether this carrier declares a metafield other than `__index`.
    fn is_operator_table(&self, binding: BindingId) -> bool {
        self.operator_tables.contains(&binding)
    }

    /// Whether some colon call in this file lands on a value derived from
    /// `setmetatable(_, C)` — the lookup a missing `__index` breaks, observed
    /// rather than inferred from a declaration. See [`InstanceUses`].
    fn has_instance_use(&self, binding: BindingId) -> bool {
        self.instance_uses.contains(&binding)
    }
}

/// `local mt = C`: which binding each local alias ultimately names.
///
/// Only `local` aliases of a *name* are followed. Storing the carrier in a
/// table, passing it to a function or returning it are all values this pass
/// does not track, and each stays exactly as unknown as it was.
///
/// The map is **fully resolved at build time**: every entry points straight
/// at its root, so [`Self::root`] is one hash lookup. It used to hold the
/// direct `alias → next` edges and walk them per query, which made
/// [`Carriers::build`]'s per-write `carrier_of` quadratic in the chain depth
/// — Shockwave round 6 measured 15.3 s for a file with an 8000-deep chain and
/// 8000 indexed writes, against a flat control that was linear.
struct Aliases(HashMap<BindingId, BindingId>);

impl Aliases {
    fn build(ctx: &LintContext<'_>) -> Self {
        let mut direct: HashMap<BindingId, BindingId> = HashMap::new();
        for (body_id, body) in ctx.lowered.bodies() {
            for (_, stmt) in body.stmts() {
                let Stmt::Local { names, init } = stmt else {
                    continue;
                };
                for (name, &value) in names.iter().zip(init) {
                    if !matches!(body.expr(value), Expr::Name(_)) {
                        continue;
                    }
                    if let Some(target) =
                        binding_of(ctx.lowered.resolution(HirId::expr(body_id, value)))
                    {
                        direct.insert(name.binding, target);
                    }
                }
            }
        }
        Self(compress(&direct))
    }

    /// The binding `local b = a; local a = C` ultimately names — `C`. One
    /// lookup: [`compress`] already walked the chain.
    fn root(&self, binding: BindingId) -> BindingId {
        self.0.get(&binding).copied().unwrap_or(binding)
    }
}

/// Resolve every alias to its root once, with path compression: each walk
/// stops at the first already-resolved node and writes the answer back to
/// every node it passed, so each binding is stepped over at most once across
/// the whole build and the result is linear in the number of aliases.
///
/// The self-loop guard (`next != current`) is kept, and the step bound with
/// it: every `local` introduces a fresh binding, so a chain is acyclic by
/// construction, but that is a property of the lowerer rather than of this
/// function's input, and a bound makes termination structural instead of
/// assumed. Neither guard costs anything on well-formed input — the bound is
/// only ever reached by a cycle, which the memo would otherwise spin on.
fn compress(direct: &HashMap<BindingId, BindingId>) -> HashMap<BindingId, BindingId> {
    let mut roots: HashMap<BindingId, BindingId> = HashMap::with_capacity(direct.len());
    let mut path: Vec<BindingId> = Vec::new();
    for &start in direct.keys() {
        if roots.contains_key(&start) {
            continue;
        }
        path.clear();
        let mut current = start;
        let root = loop {
            if let Some(&known) = roots.get(&current) {
                break known;
            }
            match direct.get(&current) {
                Some(&next) if next != current && path.len() <= direct.len() => {
                    path.push(current);
                    current = next;
                }
                _ => break current,
            }
        };
        for &node in &path {
            roots.insert(node, root);
        }
    }
    roots
}

/// A set of carrier bindings.
type Carrying = HashSet<BindingId>;

/// The raw shapes one traversal of the file collects for [`InstanceUses`].
/// Kept apart from the derivation state so a phase can read the shapes while
/// writing the state.
#[derive(Default)]
struct Shapes {
    /// Every `setmetatable(_, C)` call expression, by the carrier root `C`.
    constructions: HashMap<(BodyId, ExprId), BindingId>,
    /// Function bodies reachable by name: `local function f` and
    /// `local f = function …`.
    fn_of_local: HashMap<BindingId, BodyId>,
    /// Function bodies reachable as a field of a carrier: `function C.new()`,
    /// `C.new = function …`. Keyed by carrier root and literal key.
    fn_of_field: HashMap<(BindingId, String), BodyId>,
    /// Every `local x = <expr>`, in the order the file declares them.
    inits: Vec<(BodyId, BindingId, ExprId)>,
    /// Every `return <expr>`, by the body it returns from.
    returns: Vec<(BodyId, ExprId)>,
    /// Every `receiver:m(…)` receiver, by the body it appears in.
    receivers: Vec<(BodyId, ExprId)>,
    /// Function bodies attached to a carrier — `function C:m()`,
    /// `C.__call = function …`, `local C = { __lt = function … }` — with the
    /// carrier they hang off.
    attached: Vec<(BodyId, BindingId)>,
}

/// The carriers with an **observed instance-side use**: somewhere in this
/// file, a colon call lands on a value this pass can derive from
/// `setmetatable(_, C)`.
///
/// This is the behavioural half of the operator-table gate. Deriving a value
/// from a construction is what makes it *this* carrier's instance rather than
/// any instance — `local d = setmetatable({}, D); d:m()` is a use of `D` and
/// says nothing about a `C` that happens to declare an `m` of its own.
///
/// Four derivations are tracked, and they are exactly the four Shockwave's
/// round-6 shapes need:
///
/// 1. **the construction itself**, method-called on the spot:
///    `setmetatable({}, C):m()`;
/// 2. **a local bound to it** — `local c = setmetatable({}, C)` — and any
///    `local d = c` alias of that local, through the same [`Aliases`] map the
///    carrier side uses;
/// 3. **the constructor pattern**: a function whose body returns a
///    construction (directly, or via a local it bound to one) is a *factory*
///    for that carrier, and a local bound to a call of it holds an instance.
///    `local c = Counter.new(); c:value()` is the canonical carrier program
///    and must keep firing;
/// 4. **`self` inside a function attached to the carrier**, colon-declared
///    method or metafield alike. A `__call` factory whose body is
///    `return self:build()` reaches an instance method from *inside* the
///    carrier, and that program does crash.
///
/// Everything else is unknown and stays unknown: a carrier stored in a table,
/// passed to a function, or reached through `require` derives nothing. That
/// direction is the safe one here — an underived instance means the operator
/// table stays silent, which is the false-negative axis, and this rule's
/// false-positive axis is the one that had eight measured members.
struct InstanceUses<'ctx, 'src> {
    ctx: &'ctx LintContext<'src>,
    aliases: &'ctx Aliases,
    shapes: Shapes,
    /// Value bindings (alias roots) and the carriers they may hold an
    /// instance of.
    derived: HashMap<BindingId, Carrying>,
    /// Function bodies that return an instance, and of which carrier.
    factories: HashMap<BodyId, Carrying>,
}

impl<'ctx, 'src> InstanceUses<'ctx, 'src> {
    fn build(ctx: &'ctx LintContext<'src>, aliases: &'ctx Aliases) -> HashSet<BindingId> {
        let mut this = Self {
            ctx,
            aliases,
            shapes: Shapes::collect(ctx, aliases),
            derived: HashMap::new(),
            factories: HashMap::new(),
        };
        // Two rounds of `seed_locals`, with factory discovery between them,
        // and no more: the first round finds the locals bound directly to a
        // construction, which is what makes a `return` recognisable as a
        // factory; the second finds the locals bound to a call of one. That
        // is constructor depth one — `local c = Counter.new()` — which is the
        // pattern this is for. A factory that returns another factory's
        // result is not chased, deliberately: an unbounded fixpoint over
        // (binding × carrier) is a shape a big file could pay for, and the
        // miss is a false negative, not a false positive.
        this.seed_locals();
        this.seed_factories();
        this.seed_locals();
        this.seed_attached_self();
        this.observed()
    }

    /// The carriers a value expression may hold an instance of.
    fn carriers_of(&self, body_id: BodyId, expr: ExprId) -> Carrying {
        match self.ctx.lowered.body(body_id).expr(expr) {
            // `(setmetatable({}, C)):m()` — the paren is not a value.
            Expr::Truncate(inner) => self.carriers_of(body_id, *inner),
            Expr::Name(_) => self
                .binding_at(body_id, expr)
                .and_then(|binding| self.derived.get(&binding))
                .cloned()
                .unwrap_or_default(),
            Expr::Call { .. } => {
                if let Some(&carrier) = self.shapes.constructions.get(&(body_id, expr)) {
                    return Carrying::from([carrier]);
                }
                self.callee_body(body_id, expr)
                    .and_then(|body| self.factories.get(&body))
                    .cloned()
                    .unwrap_or_default()
            }
            _ => Carrying::new(),
        }
    }

    /// The function body a call expression reaches, when this pass can name
    /// it: a local holding a function, or a field of a carrier.
    fn callee_body(&self, body_id: BodyId, call: ExprId) -> Option<BodyId> {
        let body = self.ctx.lowered.body(body_id);
        let Expr::Call { callee, .. } = body.expr(call) else {
            return None;
        };
        match body.expr(*callee) {
            Expr::Name(_) => self
                .binding_at(body_id, *callee)
                .and_then(|binding| self.shapes.fn_of_local.get(&binding).copied()),
            Expr::Index { base, index, .. } => {
                if !matches!(body.expr(*base), Expr::Name(_)) {
                    return None;
                }
                let carrier = self.binding_at(body_id, *base)?;
                let key = index_key(body.expr(*index))?;
                self.shapes
                    .fn_of_field
                    .get(&(carrier, key.to_owned()))
                    .copied()
            }
            _ => None,
        }
    }

    /// The alias root of the binding a name expression resolves to.
    fn binding_at(&self, body_id: BodyId, expr: ExprId) -> Option<BindingId> {
        binding_of(self.ctx.lowered.resolution(HirId::expr(body_id, expr)))
            .map(|binding| self.aliases.root(binding))
    }

    /// `local c = <expr>` — record what `c` may hold.
    fn seed_locals(&mut self) {
        let mut found: Vec<(BindingId, BindingId)> = Vec::new();
        for &(body_id, binding, value) in &self.shapes.inits {
            for carrier in self.carriers_of(body_id, value) {
                found.push((self.aliases.root(binding), carrier));
            }
        }
        for (binding, carrier) in found {
            self.derived.entry(binding).or_default().insert(carrier);
        }
    }

    /// A body that returns an instance is a factory for that carrier.
    fn seed_factories(&mut self) {
        let mut found: Vec<(BodyId, BindingId)> = Vec::new();
        for &(body_id, value) in &self.shapes.returns {
            for carrier in self.carriers_of(body_id, value) {
                found.push((body_id, carrier));
            }
        }
        for (body_id, carrier) in found {
            self.factories.entry(body_id).or_default().insert(carrier);
        }
    }

    /// The `self` of a function attached to the carrier holds an instance of
    /// it — implicit under `function C:m()`, written out under
    /// `function C.__call(self, …)`. Only the first parameter counts, and
    /// only when it is the implicit `self` or is literally named [`SELF`].
    fn seed_attached_self(&mut self) {
        let mut found: Vec<(BindingId, BindingId)> = Vec::new();
        for &(fn_body, carrier) in &self.shapes.attached {
            let Some(&param) = self.ctx.lowered.body(fn_body).params.first() else {
                continue;
            };
            let binding = self.ctx.lowered.binding(param);
            if binding.kind == BindingKind::SelfParam || binding.name == SELF {
                found.push((param, carrier));
            }
        }
        for (binding, carrier) in found {
            self.derived.entry(binding).or_default().insert(carrier);
        }
    }

    /// Every carrier some `receiver:m(…)` in the file reaches.
    fn observed(&self) -> HashSet<BindingId> {
        let mut out = HashSet::new();
        for &(body_id, receiver) in &self.shapes.receivers {
            out.extend(self.carriers_of(body_id, receiver));
        }
        out
    }
}

impl Shapes {
    fn collect(ctx: &LintContext<'_>, aliases: &Aliases) -> Self {
        let mut this = Self::default();
        for (body_id, body) in ctx.lowered.bodies() {
            let carrier_of = |expr: ExprId| {
                binding_of(ctx.lowered.resolution(HirId::expr(body_id, expr)))
                    .map(|binding| aliases.root(binding))
            };
            for (expr_id, expr) in body.exprs() {
                match expr {
                    Expr::Call { .. } => {
                        if let Some(meta) = setmetatable_arg(ctx, body_id, body, expr)
                            && let Some(carrier) = carrier_of(meta)
                        {
                            this.constructions.insert((body_id, expr_id), carrier);
                        }
                    }
                    Expr::MethodCall { receiver, .. } => {
                        this.receivers.push((body_id, *receiver));
                    }
                    _ => {}
                }
            }
            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Local { names, init } => {
                        for (name, &value) in names.iter().zip(init) {
                            this.inits.push((body_id, name.binding, value));
                            match body.expr(value) {
                                Expr::Function(fn_body) => {
                                    this.fn_of_local.insert(name.binding, *fn_body);
                                }
                                // `local C = { __lt = function … }` attaches
                                // to the carrier its own constructor declares.
                                Expr::Table { entries } => {
                                    this.attach_entries(body, name.binding, entries);
                                }
                                _ => {}
                            }
                        }
                    }
                    Stmt::LocalFunction { binding, func } => {
                        if let Expr::Function(fn_body) = body.expr(*func) {
                            this.fn_of_local.insert(*binding, *fn_body);
                        }
                    }
                    Stmt::Assign { targets, values } => {
                        for (slot, &target) in targets.iter().enumerate() {
                            let Expr::Index { base, index, .. } = body.expr(target) else {
                                continue;
                            };
                            if !matches!(body.expr(*base), Expr::Name(_)) {
                                continue;
                            }
                            let (Some(carrier), Some(&value)) =
                                (carrier_of(*base), values.get(slot))
                            else {
                                continue;
                            };
                            let Expr::Function(fn_body) = body.expr(value) else {
                                continue;
                            };
                            this.attached.push((*fn_body, carrier));
                            if let Some(key) = index_key(body.expr(*index)) {
                                this.fn_of_field.insert((carrier, key.to_owned()), *fn_body);
                            }
                        }
                    }
                    Stmt::Return(values) => {
                        this.returns
                            .extend(values.iter().map(|&value| (body_id, value)));
                    }
                    _ => {}
                }
            }
        }
        this
    }

    /// Attach every function value in a carrier's own table constructor.
    fn attach_entries(&mut self, body: &Body, carrier: BindingId, entries: &[TableEntry]) {
        for entry in entries {
            let value = match entry {
                TableEntry::Positional(_) => continue,
                TableEntry::Named { value, .. } | TableEntry::Keyed { value, .. } => *value,
            };
            if let Expr::Function(fn_body) = body.expr(value) {
                self.attached.push((*fn_body, carrier));
            }
        }
    }
}

/// The literal string key of an index expression, or `None` when the key is
/// dynamic (which the caller reads as "could be `__index`").
fn index_key(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Literal(Literal::String(s)) => s.as_str(),
        _ => None,
    }
}

/// What one written key says about the carrier it is written to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    /// `__index` — or a key this pass cannot evaluate, which counts the same
    /// because the write *might* be `__index`.
    Index,
    /// Another metafield (`__call`, `__tostring`, `__add`, `__mode`, …).
    Metafield,
    /// An ordinary field, which says nothing about instance lookup.
    Plain,
}

fn key_kind(key: Option<&str>) -> KeyKind {
    match key {
        None | Some(INDEX) => KeyKind::Index,
        Some(key) if key.starts_with(METAFIELD_PREFIX) => KeyKind::Metafield,
        Some(_) => KeyKind::Plain,
    }
}

/// The kind of key a table-constructor entry names, or `None` for a
/// positional entry (which names no key at all).
fn entry_kind(body: &luabox_hir::Body, entry: &TableEntry) -> Option<KeyKind> {
    match entry {
        TableEntry::Positional(_) => None,
        TableEntry::Named { name, .. } => Some(key_kind(Some(name.as_str()))),
        TableEntry::Keyed { key, .. } => Some(key_kind(index_key(body.expr(*key)))),
    }
}
