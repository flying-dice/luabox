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
    BinOp, BindingId, BindingKind, Block, Body, BodyId, Expr, ExprId, HirId, Literal, Resolution,
    Stmt, TableEntry,
};

use crate::context::{LintContext, binding_of};
use crate::diagnostic::LintDiagnostic;
use crate::rule::{Rule, Tier};

/// The metafield instance lookup consults.
const INDEX: &str = "__index";

/// The metafield a plain call `f(…)` on a table dispatches through.
const CALL: &str = "__call";

/// The prefix every metamethod name carries (`__call`, `__tostring`, `__add`,
/// `__mode`, …). A carrier declaring one of these is a metatable with a
/// deliberate purpose that is not instance lookup.
const METAFIELD_PREFIX: &str = "__";

/// The receiver name a function attached to the carrier reaches an instance
/// through — implicit under `function C:m()`, written out under
/// `function C.__call(self, …)`. Both spellings count (see
/// [`InstanceUses::calling_reaches_a_method`]).
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
///   really does execute is a class that happens to overload an operator, and
///   *that* call crashes. The gate is behavioural, not structural, and
///   "behavioural" is about what the file **reaches**, not about what it
///   contains:
///
///   - *declaring* `function Cache:reset()` next to `Cache.__mode = "k"` is
///     not a lookup, and a `Cache` no call site invokes runs fine (Shockwave
///     round 6 measured eight such carriers);
///   - nor is *writing* `self:clear()` inside that `Cache:reset`. The body of
///     a method nothing calls does not execute, so the colon call in it never
///     happens; round 7 measured this arm firing on exactly that program,
///     which `lua5.4` runs to completion. What makes `Cache:reset` reachable
///     is `c:reset()` on a derived value, and that is a use the derivations
///     see directly;
///   - nor, round 8, is a colon call written *anywhere* the file does not
///     go. `local c = setmetatable({}, Cache)` beside
///     `local function boom() return c:reset() end` and `print(type(boom))`
///     is a lookup that never happens, and so is one inside
///     `if false then … end`. Uses are counted only in bodies this file
///     [enters](InstanceUses::reached_bodies) and only in statements no
///     literal condition prunes.
///
///   The three ways this file can *reach* a method through the metatable are
///   a colon call on a derived value, a plain call on one whose `__call` runs
///   a colon call on its own receiver, and dot dispatch with an explicit self
///   (`c.m(c)`). All are in [`InstanceUses`], with the one approximation that
///   remains.
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
                // file actually *reaches* a method on an instance of it.
                // Neither declaring `function Cache:reset()` nor writing
                // `self:clear()` inside it is a lookup — an unreached body
                // does not execute. `c:value()` on a value derived from
                // `setmetatable(_, Cache)` is one, and so is `c()` when
                // `Cache.__call` runs a colon call on its own receiver (see
                // [`InstanceUses`]).
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

/// Which value expression, and which of its result slots, supplies each name
/// in a `local a, b, c = …` or `a, b, c = …` — Lua's adjustment rule, spelled
/// out.
///
/// Every initializer but the last is truncated to one value, so name `i` takes
/// slot 0 of initializer `i`. The *last* initializer is the only one that can
/// expand: a call or `...` fills every remaining name, so a name past the end
/// of the list takes slot `i - (len - 1)` of it. Anything else (a literal, a
/// name, a parenthesised call) yields exactly one value, and the names past it
/// are `nil` — [`None`].
///
/// `names.iter().zip(init)` is the shape this replaces, and it silently
/// *dropped* every name past the initializer list: `local _n, c = make()`
/// paired `_n` with `make()` and left `c` invisible to all three passes that
/// walk these statements — so a `c` holding a constructed instance was never
/// seeded, and the rule went silent on a program that crashes (Shockwave
/// round 7, issue W). It also mis-slotted the name it *did* pair, handing
/// `_n` a derivation that belongs to `c`.
///
/// The slot is reported rather than resolved: what a multi-value tail actually
/// yields is the caller's question ([`InstanceUses::carriers_of`] answers it
/// for the constructor pattern and gives up otherwise), and a name this
/// function reports as unknown-but-present is strictly better than one it
/// never mentions.
fn supplies<'a>(
    body: &'a Body,
    names: usize,
    init: &'a [ExprId],
) -> impl Iterator<Item = Option<(ExprId, usize)>> + 'a {
    (0..names).map(move |i| {
        let (&last, head) = init.split_last()?;
        if let Some(&expr) = head.get(i) {
            Some((expr, 0))
        } else if i == head.len() {
            Some((last, 0))
        } else if matches!(
            body.expr(last),
            Expr::Call { .. } | Expr::MethodCall { .. } | Expr::Vararg
        ) {
            Some((last, i - head.len()))
        } else {
            None
        }
    })
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
                    // own constructor. Names and values pair through
                    // [`supplies`], exactly as Lua adjusts them; a table
                    // constructor is single-valued, so only slot 0 can be one.
                    // (`local mt = C` is an alias and is handled by
                    // [`Aliases`], not here.)
                    Stmt::Local { names, init } => {
                        for (name, supply) in names.iter().zip(supplies(body, names.len(), init)) {
                            let Some((value, 0)) = supply else {
                                continue;
                            };
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

    /// Whether this file *reaches* a method on a value derived from
    /// `setmetatable(_, C)` — the lookup a missing `__index` breaks, observed
    /// at a call site rather than inferred from a declaration or from a colon
    /// call written inside a body nothing enters. See [`InstanceUses`].
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
                // A name can only alias a carrier when it is bound to a bare
                // name, which is single-valued — so only slot 0 aliases, and
                // a name fed by a multi-value tail aliases nothing.
                for (name, supply) in names.iter().zip(supplies(body, names.len(), init)) {
                    let Some((value, 0)) = supply else {
                        continue;
                    };
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

/// A value this pass can name and therefore track a derivation against: a
/// resolved binding (local, upvalue, parameter — alias-rooted), or a global.
///
/// Globals are here because `g = setmetatable({}, C)` followed by `g:m()` is
/// the same program as the `local` spelling and crashes the same way, and
/// [`binding_of`] answers `None` for both halves of it — there is no
/// [`BindingId`] for a free name. Keying on the name instead is exactly as
/// sound as the rest of this pass, which is flow-insensitive anyway: two
/// different tables reaching one global name merge their carriers, which is
/// the over-approximating direction on a gate whose failure mode is silence.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ValueRef {
    Binding(BindingId),
    Global(String),
}

/// The raw shapes one traversal of the file collects for [`InstanceUses`].
/// Kept apart from the derivation state so a phase can read the shapes while
/// writing the state.
#[derive(Default)]
struct Shapes {
    /// Every `setmetatable(_, C)` call expression, by the carrier root `C`.
    constructions: HashMap<(BodyId, ExprId), BindingId>,
    /// The **first argument** of each `setmetatable(t, C)`, when it is a value
    /// this pass can name: `t` holds an instance of `C` from that call on,
    /// whether or not anything is bound to the call's result. See
    /// [`InstanceUses::seed_construction_arguments`].
    construction_arguments: Vec<(ValueRef, BindingId)>,
    /// Function bodies reachable by name: `local function f`,
    /// `local f = function …`, `function g() … end`, `g = function …`.
    fn_of_name: HashMap<ValueRef, BodyId>,
    /// Function bodies reachable as a field of a table: `function C.new()`,
    /// `C.new = function …`, `local C = { new = function … }`. Keyed by the
    /// table's *value* — an alias-rooted binding or a global name — and the
    /// literal key.
    ///
    /// Keyed by binding alone until round 8, which made
    /// `M = {}; function M.new() … end; local c = M.new()` resolve to no body
    /// at all: a global name has no [`BindingId`], so the module-table
    /// factory — the shape half of Lua's ecosystem is written in — derived
    /// nothing. [`ValueRef`] already names both halves for the value side.
    fn_of_field: HashMap<(ValueRef, String), BodyId>,
    /// The body of each carrier's `__call` metamethod — the function a plain
    /// call on one of its instances dispatches to. See
    /// [`InstanceUses::calling_reaches_a_method`].
    call_metamethod: HashMap<BindingId, BodyId>,
    /// Every binding-or-global initialisation the pass can see, as
    /// (body, bound value, initialiser, result slot of that initialiser).
    /// `local x = <expr>` and `x = <expr>` alike.
    inits: Vec<(BodyId, ValueRef, ExprId, usize)>,
    /// Every `return <exprs>`, by the body it returns from. The list is kept
    /// whole: which *slot* a value is returned in is what a caller's
    /// `local _n, c = make()` needs.
    returns: Vec<(BodyId, Vec<ExprId>)>,
    /// Every `receiver:m(…)` in a **live** statement, as (body, receiver,
    /// method name). The method name is what says which body an instance
    /// colon call enters — see [`InstanceUses::reached_bodies`].
    receivers: Vec<(BodyId, ExprId, String)>,
    /// Every plain call in a **live** statement, as (body, the call, its
    /// callee). The callee names the body the call enters; a call on an
    /// *instance* is a `__call` dispatch, which is the other way a file
    /// reaches a method through a metatable.
    calls: Vec<(BodyId, ExprId, ExprId)>,
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
/// # Which values are derived
///
/// A value is *derived* from `C` when this pass can follow it back to a
/// `setmetatable(_, C)`:
///
/// 1. **the construction itself**: `setmetatable({}, C):m()`;
/// 2. **the construction's first argument**: `setmetatable` mutates the table
///    it is handed and returns it, so `local t = {}; setmetatable(t, C)` makes
///    `t` an instance as surely as binding the result does. Alias-rooted, and
///    linked on evaluation rather than on statement position. See
///    [`Self::seed_construction_arguments`];
/// 3. **a name bound to either** — `local c = setmetatable({}, C)`, and
///    equally `c = setmetatable({}, C)` on a name declared earlier or on a
///    global — plus any `local d = c` alias, through the same [`Aliases`] map
///    the carrier side uses. `a or b` and `a and b` are followed into the
///    operand the expression **definitively evaluates to**, which for a
///    construction (always truthy) means `ctor or x` and `x and ctor` are
///    followed and `x or ctor` and `ctor and x` are not — spelled out at the
///    [`BinOp`] arm of [`Self::carriers_of`];
/// 4. **the constructor pattern**: a function whose body returns a
///    construction (directly, or via a name it bound to one) is a *factory*
///    for that carrier, and a name bound to a call of it holds an instance.
///    `local c = Counter.new(); c:value()` is the canonical carrier program
///    and must keep firing. Return **slots** are tracked, so
///    `return 1, setmetatable({}, C)` seeds the second name of
///    `local n, c = make()` and not the first. The factory may be a field of
///    a local *or a global* module table (`M.new()`).
///
/// # Which uses count
///
/// Three, all of them *reaching* a method rather than declaring one, and all
/// of them counted only at a call site in a body this file
/// [enters](Self::reached_bodies):
///
/// - a **colon call on a derived value** — `c:m()`. This is the lookup a
///   missing `__index` breaks, observed directly;
/// - a **plain call on a derived value** — `f()` — when the carrier's
///   `__call` metamethod is a function body in this file whose own `self`
///   takes a colon call. That is the chain `f()` → `C.__call(self)` →
///   `self:m()`, and it does crash;
/// - **dot dispatch with an explicit self** — `c.m(c)` — when `m` names a
///   function attached to the carrier. Same lookup, same failure, different
///   spelling; see [`Self::dot_dispatched`] for what keeps a bare field
///   *read* out of it.
///
/// The second used to be spelled the other way round: `self` inside *any*
/// function attached to the carrier was treated as a derived value, so a
/// `self:m()` anywhere in any attached body counted as a use whether or not
/// the file ever reached that body. Shockwave round 7 measured the cost: the
/// `__call` fixture with its last line changed from `print(f())` to
/// `print(type(f))` produced byte-identical lint output against opposite
/// runtime verdicts, and `function Cache:reset() self:clear() end` on an
/// uninvoked `Cache` fired on a program that runs fine. Presence in a body is
/// not reaching it. A colon-declared method is reached only through an
/// instance colon call, which derivation (1)–(4) already sees; the `__call`
/// body is the one that a *plain* call reaches, so it is the one this asks
/// about.
///
/// # Where a use has to be
///
/// Round 7 fixed that *one shape at a time*, which left the mechanism
/// untouched: `observed()` still read receivers out of every body in the file
/// with no reachability test at all. Round 8 measured what that costs —
/// `local function boom() return c:reset() end` beside `print(type(boom))`
/// warned about a lookup no execution performs, and so did
/// `if false then c:reset() end`.
///
/// So a use is counted only where the file can perform it: in a body the
/// [reached set](Self::reached_bodies) contains, and in a statement no
/// literal condition prunes ([`Shapes::walk_block`]). Both are FN-biased at
/// their edges — an escaping closure is not reached, and a guard that is a
/// name is not decided.
///
/// **The remaining approximation is a false positive, and it is the
/// non-literal guard.** `local on = false; if on then c:reset() end` still
/// counts, because deciding it needs constant propagation rather than a look
/// at the token — the same bound the `__index`-write side has carried since
/// the rule shipped ("an `__index` write in a branch that never runs"). The
/// dead-branch `self:m()` inside a `__call` that round 7 disclosed here is
/// **closed**: the prune applies wherever statements are walked, the `__call`
/// body included.
///
/// Everything else is unknown and stays unknown: an instance stored in a
/// table, passed as a parameter, taken from a `for`-in variable, returned by a
/// method call, or reached through `require` derives nothing. That direction
/// is the safe one here — an underived instance means the operator table stays
/// silent, which is the false-negative axis, and this rule's false-positive
/// axis is the one that had eight measured members.
struct InstanceUses<'ctx, 'src> {
    ctx: &'ctx LintContext<'src>,
    aliases: &'ctx Aliases,
    shapes: Shapes,
    /// Values (alias-rooted bindings, and globals by name) and the carriers
    /// they may hold an instance of.
    derived: HashMap<ValueRef, Carrying>,
    /// Function bodies that return an instance, by **return slot**: entry `i`
    /// is the set of carriers the body's `i`th returned value may be an
    /// instance of.
    factories: HashMap<BodyId, Vec<Carrying>>,
    /// The bodies this file actually enters — see [`Self::reached_bodies`].
    /// A use written in a body outside this set is not a use.
    reached: HashSet<BodyId>,
}

impl<'ctx, 'src> InstanceUses<'ctx, 'src> {
    fn build(ctx: &'ctx LintContext<'src>, aliases: &'ctx Aliases) -> HashSet<BindingId> {
        let mut this = Self {
            ctx,
            aliases,
            shapes: Shapes::collect(ctx, aliases),
            derived: HashMap::new(),
            factories: HashMap::new(),
            reached: HashSet::new(),
        };
        // `setmetatable(t, C)` makes `t` an instance from that call on, so the
        // argument link is seeded before anything reads a derivation — it is
        // what makes the statement-form constructor inside a factory look
        // like a factory at all.
        this.seed_construction_arguments();
        // Two rounds of `seed_values`, with factory discovery between them,
        // and no more: the first round finds the names bound directly to a
        // construction, which is what makes a `return` recognisable as a
        // factory; the second finds the names bound to a call of one. That
        // is constructor depth one — `local c = Counter.new()` — which is the
        // pattern this is for. A factory that returns another factory's
        // result is not chased, deliberately: an unbounded fixpoint over
        // (value × carrier) is a shape a big file could pay for, and the
        // miss is a false negative, not a false positive.
        this.seed_values();
        this.seed_factories();
        this.seed_values();
        // Derivation is flow-insensitive and stays that way: which carrier a
        // value belongs to does not depend on whether the file runs the line.
        // Which *uses* count does, so reachability is answered once the
        // derivations it consults are complete.
        this.reached = this.reached_bodies();
        this.observed()
    }

    /// The carriers result `slot` of a value expression may hold an instance
    /// of. Slot 0 is the ordinary single-value question; higher slots are only
    /// ever answerable for a call of a known factory, which is what makes
    /// `local _n, c = make()` land on `c`.
    fn carriers_of(&self, body_id: BodyId, expr: ExprId, slot: usize) -> Carrying {
        match self.ctx.lowered.body(body_id).expr(expr) {
            // `(setmetatable({}, C)):m()` — the paren is not a value, and it
            // truncates to exactly one.
            Expr::Truncate(inner) if slot == 0 => self.carriers_of(body_id, *inner, 0),
            Expr::Name(_) if slot == 0 => self
                .value_at(body_id, expr)
                .and_then(|value| self.derived.get(&value))
                .cloned()
                .unwrap_or_default(),
            // `and` / `or` are followed into the operand the expression
            // **definitively evaluates to**, which is not the same as either
            // operand it might mention. A construction is a table, so it is
            // always truthy, and that is what decides three of the four:
            //
            // - `ctor or x` — the construction is truthy, so `x` never
            //   evaluates and the result *is* the construction. Followed.
            // - `x or ctor` — the construction is reached only when `x` is
            //   falsy, which this pass cannot decide. Not followed, which is
            //   a false negative when `x` really is falsy and closes the
            //   round-8 false positive when it is not (a truthy `x` is the
            //   whole value, and calling a method on it may be perfectly
            //   fine). Following the left operand alone spells both.
            // - `x and ctor` — the construction is the result when `x` is
            //   truthy, and when `x` is falsy the result is that falsy value,
            //   on which the same colon call fails just as hard. Followed:
            //   there is no assignment of `x` under which the use is safe.
            // - `ctor and x` — the construction is truthy, so the result is
            //   `x` and the construction is discarded. Not followed, which
            //   falls out of following the right operand alone.
            //
            // Both operators truncate to exactly one value.
            Expr::Binary { op, lhs, rhs } if slot == 0 => match op {
                BinOp::And => self.carriers_of(body_id, *rhs, 0),
                BinOp::Or => self.carriers_of(body_id, *lhs, 0),
                _ => Carrying::new(),
            },
            Expr::Call { .. } => {
                if slot == 0
                    && let Some(&carrier) = self.shapes.constructions.get(&(body_id, expr))
                {
                    return Carrying::from([carrier]);
                }
                self.callee_body(body_id, expr)
                    .and_then(|body| self.factories.get(&body))
                    .and_then(|slots| slots.get(slot))
                    .cloned()
                    .unwrap_or_default()
            }
            _ => Carrying::new(),
        }
    }

    /// The function body a call expression reaches, when this pass can name
    /// it: a local or global holding a function, a field of a named table, or
    /// a function expression the call site writes out itself.
    fn callee_body(&self, body_id: BodyId, call: ExprId) -> Option<BodyId> {
        let body = self.ctx.lowered.body(body_id);
        let Expr::Call { callee, .. } = body.expr(call) else {
            return None;
        };
        match body.expr(*callee) {
            Expr::Name(_) => self
                .value_at(body_id, *callee)
                .and_then(|value| self.shapes.fn_of_name.get(&value).copied()),
            Expr::Index { base, index, .. } => {
                if !matches!(body.expr(*base), Expr::Name(_)) {
                    return None;
                }
                let owner = self.value_at(body_id, *base)?;
                let key = index_key(body.expr(*index))?;
                self.shapes
                    .fn_of_field
                    .get(&(owner, key.to_owned()))
                    .copied()
            }
            // `(function() … end)()` — the callee *is* the body, so no name
            // needs resolving.
            Expr::Function(fn_body) => Some(*fn_body),
            _ => None,
        }
    }

    /// The value a name expression refers to: the alias root of its binding,
    /// or the global's name.
    fn value_at(&self, body_id: BodyId, expr: ExprId) -> Option<ValueRef> {
        value_ref(
            self.ctx.lowered.resolution(HirId::expr(body_id, expr)),
            |b| self.aliases.root(b),
        )
    }

    /// The alias root of the binding a name expression resolves to, ignoring
    /// globals — the carrier positions, where a `---@class` annotation is only
    /// ever attached to a binding.
    fn binding_at(&self, body_id: BodyId, expr: ExprId) -> Option<BindingId> {
        binding_of(self.ctx.lowered.resolution(HirId::expr(body_id, expr)))
            .map(|binding| self.aliases.root(binding))
    }

    /// `setmetatable(t, C)` — record that **`t`** may hold an instance of
    /// `C`, not only whatever the call's result is bound to.
    ///
    /// `setmetatable` mutates its first argument and returns it, so the two
    /// spellings
    ///
    /// ```lua
    /// local c = setmetatable({}, Cache)   -- expression form
    /// local t = {}; setmetatable(t, Cache) -- statement form
    /// ```
    ///
    /// produce the same instance, and the second is the idiomatic one — it is
    /// what a constructor that wants to name its table before wiring it
    /// writes. Only the result was tracked until round 8, so
    /// `local t = {}; setmetatable(t, Cache); t:reset()` was silent on a
    /// crash. Linking the argument also feeds factory detection for free: a
    /// `new()` that builds this way ends in `return t`, and `t` is now
    /// derived, which is exactly what [`Self::seed_factories`] reads.
    ///
    /// The link is alias-rooted, so constructing through `local u = t` seeds
    /// the table both names share, and it is made on *evaluation* rather than
    /// on statement position — the same call used as an expression links its
    /// argument too.
    fn seed_construction_arguments(&mut self) {
        for (target, carrier) in self.shapes.construction_arguments.clone() {
            self.derived.entry(target).or_default().insert(carrier);
        }
    }

    /// `local c = <expr>` and `c = <expr>` — record what `c` may hold, from
    /// the initialiser slot [`supplies`] said feeds it.
    fn seed_values(&mut self) {
        let mut found: Vec<(ValueRef, BindingId)> = Vec::new();
        for (body_id, target, value, slot) in &self.shapes.inits {
            for carrier in self.carriers_of(*body_id, *value, *slot) {
                found.push((target.clone(), carrier));
            }
        }
        for (target, carrier) in found {
            self.derived.entry(target).or_default().insert(carrier);
        }
    }

    /// A body that returns an instance is a factory for that carrier, in the
    /// slot it returns it in.
    ///
    /// Only the value written in each slot is read: a trailing `return f()`
    /// that would itself expand to several values contributes its slot 0 and
    /// nothing beyond, the same constructor-depth-one bound the seeding rounds
    /// take.
    fn seed_factories(&mut self) {
        let mut found: Vec<(BodyId, usize, BindingId)> = Vec::new();
        for (body_id, values) in &self.shapes.returns {
            for (slot, &value) in values.iter().enumerate() {
                for carrier in self.carriers_of(*body_id, value, 0) {
                    found.push((*body_id, slot, carrier));
                }
            }
        }
        for (body_id, slot, carrier) in found {
            let slots = self.factories.entry(body_id).or_default();
            if slots.len() <= slot {
                slots.resize_with(slot + 1, Carrying::new);
            }
            if let Some(at) = slots.get_mut(slot) {
                at.insert(carrier);
            }
        }
    }

    /// The bodies this file **enters**: the chunk, plus a fixpoint over the
    /// in-file call graph.
    ///
    /// A body is added when a body already in the set contains a live call
    /// that names it, by any of the edges the pass already tracks:
    ///
    /// - a plain call of a named function or of a field of a named table
    ///   (`boom()`, `Cache.run()`, `M.new()`) or of a function expression the
    ///   call site writes out — [`Self::callee_body`];
    /// - a plain call on a *derived value*, which dispatches to the carrier's
    ///   `__call` metamethod body;
    /// - a colon call, which enters the method attached under that name —
    ///   either to the table the receiver itself names (`h:go()`, `C:new()`)
    ///   or, when the receiver is a derived value, to its carrier
    ///   (`r:go()` enters `Runner.go`).
    ///
    /// **What is not reached is the point.** A closure that escapes — stored
    /// in a table, passed as an argument, returned to whoever required the
    /// chunk — has no call site naming its body, so this file does not enter
    /// it. That is a false negative when the closure really is called
    /// elsewhere, and it is the direction this arm errs in; the alternative
    /// is what round 8 measured, where `local function boom() c:reset() end`
    /// beside `print(type(boom))` warned about a lookup that never happens.
    ///
    /// Cost is linear in call sites: the call and receiver lists are indexed
    /// by body once, and each body is expanded at most once.
    fn reached_bodies(&self) -> HashSet<BodyId> {
        let mut calls: HashMap<BodyId, Vec<(ExprId, ExprId)>> = HashMap::new();
        for &(body_id, call, callee) in &self.shapes.calls {
            calls.entry(body_id).or_default().push((call, callee));
        }
        let mut receivers: HashMap<BodyId, Vec<(ExprId, &str)>> = HashMap::new();
        for (body_id, receiver, method) in &self.shapes.receivers {
            receivers
                .entry(*body_id)
                .or_default()
                .push((*receiver, method.as_str()));
        }

        let chunk = self.ctx.lowered.chunk();
        let mut reached = HashSet::from([chunk]);
        let mut work = vec![chunk];
        while let Some(body_id) = work.pop() {
            for &(call, callee) in calls.get(&body_id).into_iter().flatten() {
                if let Some(target) = self.callee_body(body_id, call) {
                    enter(target, &mut reached, &mut work);
                }
                for carrier in self.carriers_of(body_id, callee, 0) {
                    if let Some(&target) = self.shapes.call_metamethod.get(&carrier) {
                        enter(target, &mut reached, &mut work);
                    }
                }
            }
            for &(receiver, method) in receivers.get(&body_id).into_iter().flatten() {
                let owners = self.value_at(body_id, receiver).into_iter().chain(
                    self.carriers_of(body_id, receiver, 0)
                        .into_iter()
                        .map(ValueRef::Binding),
                );
                for owner in owners {
                    if let Some(&target) = self.shapes.fn_of_field.get(&(owner, method.to_owned()))
                    {
                        enter(target, &mut reached, &mut work);
                    }
                }
            }
        }
        reached
    }

    /// Every carrier this file actually reaches a method on — counted only at
    /// call sites in bodies the file [enters](Self::reached_bodies), and only
    /// in statements no literal condition prunes.
    ///
    /// Three shapes reach a method: a colon call on a derived value, a plain
    /// call on one whose `__call` runs a colon call on its own `self`, and
    /// dot dispatch with an explicit self.
    fn observed(&self) -> HashSet<BindingId> {
        let mut out = HashSet::new();
        for &(body_id, receiver, _) in &self.shapes.receivers {
            if self.reached.contains(&body_id) {
                out.extend(self.carriers_of(body_id, receiver, 0));
            }
        }
        for &(body_id, _, callee) in &self.shapes.calls {
            if !self.reached.contains(&body_id) {
                continue;
            }
            for carrier in self.carriers_of(body_id, callee, 0) {
                if self.calling_reaches_a_method(carrier) {
                    out.insert(carrier);
                }
            }
            out.extend(self.dot_dispatched(body_id, callee));
        }
        out
    }

    /// `c.m(c)` — the explicit-self spelling of `c:m()`, and the same lookup.
    ///
    /// With no `__index`, `c.m` is `nil`, so the call fails with `attempt to
    /// call a nil value (field 'm')` where the colon form says `method 'm'`:
    /// one defect, two spellings, and Lua code writes both.
    ///
    /// Two things keep this from counting more than it should. It is a
    /// **call**, not a field access — `local r = c.reset` and
    /// `print(type(c.reset))` perform the lookup, get `nil`, and run fine, so
    /// only an [`Expr::Index`] in *callee* position is read. And the key must
    /// name a function **attached to the carrier**: `c.m(…)` where `m` was
    /// put in the instance's own table by the construction resolves without
    /// the metatable, and a metafield (`c.__mode`) is not served by `__index`
    /// at all.
    fn dot_dispatched(&self, body_id: BodyId, callee: ExprId) -> Carrying {
        let body = self.ctx.lowered.body(body_id);
        let Expr::Index { base, index, .. } = body.expr(callee) else {
            return Carrying::new();
        };
        let Some(key) = index_key(body.expr(*index)) else {
            return Carrying::new();
        };
        if key_kind(Some(key)) != KeyKind::Plain {
            return Carrying::new();
        }
        self.carriers_of(body_id, *base, 0)
            .into_iter()
            .filter(|&carrier| {
                self.shapes
                    .fn_of_field
                    .contains_key(&(ValueRef::Binding(carrier), key.to_owned()))
            })
            .collect()
    }

    /// Whether *calling* an instance of this carrier reaches a method on it:
    /// the carrier's `__call` is a function body in this file, its first
    /// parameter is the receiver, and that receiver takes a colon call inside
    /// the body.
    ///
    /// Only the `__call` body is asked, not every attached body. A carrier
    /// whose `__call` is `function(self) return 1 end` beside a
    /// `function C:reset() self:clear() end` that nothing invokes runs fine
    /// under `c()`, and asking the wider question would report it.
    ///
    /// Only the *direct* body counts: a colon call inside a closure nested in
    /// the `__call`, or in a helper it calls, is not followed. That is a false
    /// negative, and the direction this arm errs in.
    fn calling_reaches_a_method(&self, carrier: BindingId) -> bool {
        let Some(&body_id) = self.shapes.call_metamethod.get(&carrier) else {
            return false;
        };
        let Some(&param) = self.ctx.lowered.body(body_id).params.first() else {
            return false;
        };
        let binding = self.ctx.lowered.binding(param);
        // Implicit under `function C:__call()`, written out under
        // `function C.__call(self, …)`. Both spellings count.
        if binding.kind != BindingKind::SelfParam && binding.name != SELF {
            return false;
        }
        self.shapes.receivers.iter().any(|&(body, receiver, _)| {
            body == body_id && self.binding_at(body, receiver) == Some(param)
        })
    }
}

/// Add `target` to the reached set, queueing it for expansion the first time.
fn enter(target: BodyId, reached: &mut HashSet<BodyId>, work: &mut Vec<BodyId>) {
    if reached.insert(target) {
        work.push(target);
    }
}

/// [`binding_of`], extended to name globals — see [`ValueRef`]. The `root`
/// callback applies the alias map to the binding case only; a global has no
/// alias chain to walk.
fn value_ref(
    res: Option<&Resolution>,
    root: impl FnOnce(BindingId) -> BindingId,
) -> Option<ValueRef> {
    match res? {
        Resolution::Local(b) => Some(ValueRef::Binding(root(*b))),
        Resolution::Upvalue { binding, .. } => Some(ValueRef::Binding(root(*binding))),
        Resolution::Global(name) => Some(ValueRef::Global(name.clone())),
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
            let value_of = |expr: ExprId| {
                value_ref(ctx.lowered.resolution(HirId::expr(body_id, expr)), |b| {
                    aliases.root(b)
                })
            };
            // Constructions are collected flat, over every expression the body
            // holds: `check` reports on the *site*, wherever it sits, and a
            // value's derivation is a property of the expression rather than
            // of the statement around it. Call sites are not — which calls
            // this body performs is a control-flow question, so they come
            // from the live-statement walk below.
            for (expr_id, expr) in body.exprs() {
                let Some(meta) = setmetatable_arg(ctx, body_id, body, expr) else {
                    continue;
                };
                let Some(carrier) = carrier_of(meta) else {
                    continue;
                };
                this.constructions.insert((body_id, expr_id), carrier);
                if let Expr::Call { args, .. } = expr
                    && let Some(&target) = args.first()
                    && let Some(value) = value_of(target)
                {
                    this.construction_arguments.push((value, carrier));
                }
            }
            this.walk_block(body_id, body, &body.block);
            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Local { names, init } => {
                        for (name, supply) in names.iter().zip(supplies(body, names.len(), init)) {
                            let Some((value, slot)) = supply else {
                                continue;
                            };
                            let target = ValueRef::Binding(aliases.root(name.binding));
                            this.inits.push((body_id, target.clone(), value, slot));
                            if slot != 0 {
                                continue;
                            }
                            match body.expr(value) {
                                Expr::Function(fn_body) => {
                                    this.fn_of_name.insert(target, *fn_body);
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
                            this.fn_of_name
                                .insert(ValueRef::Binding(aliases.root(*binding)), *fn_body);
                        }
                    }
                    // Both target shapes matter. An `Expr::Index` target is
                    // `function C:m()` / `C.__call = function …` — a function
                    // attached to a carrier. An `Expr::Name` target is
                    // `c = setmetatable({}, C)` on a name declared earlier
                    // (`local c` then `c = …`), on a global, or
                    // `function make() … end`. Reading only the first left
                    // `local c` + `c = setmetatable({}, Cache)` + `c:reset()`
                    // silent on a crash (Shockwave round 7).
                    Stmt::Assign { targets, values } => {
                        for (&target, supply) in
                            targets.iter().zip(supplies(body, targets.len(), values))
                        {
                            let Some((value, slot)) = supply else {
                                continue;
                            };
                            match body.expr(target) {
                                Expr::Index { base, index, .. } => {
                                    if slot != 0 || !matches!(body.expr(*base), Expr::Name(_)) {
                                        continue;
                                    }
                                    let (Some(owner), Expr::Function(fn_body)) =
                                        (value_of(*base), body.expr(value))
                                    else {
                                        continue;
                                    };
                                    if let Some(key) = index_key(body.expr(*index)) {
                                        this.attach(owner, key, *fn_body);
                                    }
                                }
                                Expr::Name(_) => {
                                    let Some(bound) = value_of(target) else {
                                        continue;
                                    };
                                    this.inits.push((body_id, bound.clone(), value, slot));
                                    if slot == 0
                                        && let Expr::Function(fn_body) = body.expr(value)
                                    {
                                        this.fn_of_name.insert(bound, *fn_body);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Stmt::Return(values) => this.returns.push((body_id, values.clone())),
                    _ => {}
                }
            }
        }
        this
    }

    /// Record a function attached to a table under a literal key: reachable
    /// as `owner.key(…)`, and — when the owner is a carrier binding and the
    /// key is [`CALL`] — as the metamethod a plain call on one of that
    /// carrier's instances dispatches to.
    ///
    /// A `---@class` annotation only ever attaches to a binding, so the
    /// metamethod map stays keyed by [`BindingId`]; `fn_of_field` does not,
    /// because a module table is as often a global as a local.
    fn attach(&mut self, owner: ValueRef, key: &str, fn_body: BodyId) {
        if key == CALL
            && let ValueRef::Binding(carrier) = owner
        {
            self.call_metamethod.insert(carrier, fn_body);
        }
        self.fn_of_field.insert((owner, key.to_owned()), fn_body);
    }

    /// Attach every keyed function value in a carrier's own table
    /// constructor — `local C = { __call = function(self) … end }` declares
    /// the metamethod exactly as `C.__call = …` does.
    fn attach_entries(&mut self, body: &Body, carrier: BindingId, entries: &[TableEntry]) {
        for entry in entries {
            let (key, value) = match entry {
                TableEntry::Positional(_) => continue,
                TableEntry::Named { name, value } => (Some(name.as_str()), *value),
                TableEntry::Keyed { key, value } => (index_key(body.expr(*key)), *value),
            };
            if let (Some(key), Expr::Function(fn_body)) = (key, body.expr(value)) {
                self.attach(ValueRef::Binding(carrier), key, *fn_body);
            }
        }
    }

    /// Collect the call sites in one block, **skipping blocks a literal
    /// condition proves never run**.
    ///
    /// This is the only control-flow reasoning in the pass, and it is
    /// deliberately the shallowest kind: a condition that *is* `false` or
    /// `nil` in the source. `if false then c:reset() end` next to a
    /// construction was a measured round-8 false positive, and it needs
    /// nothing more than reading the literal to answer. A condition that is a
    /// name, a comparison or a call is not decided — `local on = false; if on
    /// then …` still counts, and that bound is disclosed in
    /// `docs/03-reference/02-limitations.md` beside the `__index`-write
    /// side's identical one.
    ///
    /// `repeat` is the case worth naming: it tests *after* its body, so the
    /// body always executes and no condition can prune it.
    fn walk_block(&mut self, body_id: BodyId, body: &Body, block: &Block) {
        for &stmt_id in &block.stmts {
            match body.stmt(stmt_id) {
                Stmt::Local { init, .. } => self.walk_exprs(body_id, body, init),
                Stmt::Assign { targets, values } => {
                    self.walk_exprs(body_id, body, targets);
                    self.walk_exprs(body_id, body, values);
                }
                Stmt::ExprStmt(expr) => self.walk_expr(body_id, body, *expr),
                Stmt::If {
                    branches,
                    else_block,
                } => {
                    for branch in branches {
                        self.walk_expr(body_id, body, branch.cond);
                        if !is_falsy_literal(body.expr(branch.cond)) {
                            self.walk_block(body_id, body, &branch.block);
                        }
                    }
                    // The `else` of a literal-false `if` is the branch that
                    // *does* run, so it is never pruned with the `then`.
                    if let Some(block) = else_block {
                        self.walk_block(body_id, body, block);
                    }
                }
                Stmt::While {
                    cond,
                    body: loop_body,
                } => {
                    self.walk_expr(body_id, body, *cond);
                    if !is_falsy_literal(body.expr(*cond)) {
                        self.walk_block(body_id, body, loop_body);
                    }
                }
                Stmt::Repeat {
                    body: loop_body,
                    cond,
                } => {
                    self.walk_block(body_id, body, loop_body);
                    self.walk_expr(body_id, body, *cond);
                }
                Stmt::NumericFor {
                    start,
                    end,
                    step,
                    body: loop_body,
                    ..
                } => {
                    self.walk_expr(body_id, body, *start);
                    self.walk_expr(body_id, body, *end);
                    if let Some(step) = step {
                        self.walk_expr(body_id, body, *step);
                    }
                    self.walk_block(body_id, body, loop_body);
                }
                Stmt::GenericFor {
                    exprs,
                    body: loop_body,
                    ..
                } => {
                    self.walk_exprs(body_id, body, exprs);
                    self.walk_block(body_id, body, loop_body);
                }
                Stmt::Do { body: inner } => self.walk_block(body_id, body, inner),
                Stmt::Return(values) => self.walk_exprs(body_id, body, values),
                Stmt::LocalFunction { func, .. } => self.walk_expr(body_id, body, *func),
                Stmt::Break | Stmt::Goto { .. } | Stmt::Label { .. } | Stmt::Error => {}
            }
        }
    }

    fn walk_exprs(&mut self, body_id: BodyId, body: &Body, exprs: &[ExprId]) {
        for &expr in exprs {
            self.walk_expr(body_id, body, expr);
        }
    }

    /// Collect the call sites inside one expression.
    ///
    /// A nested [`Expr::Function`] is **not** descended into: a closure is its
    /// own body, walked at its own top level, and whether this file ever
    /// enters it is the reachability question rather than a lexical one.
    fn walk_expr(&mut self, body_id: BodyId, body: &Body, expr: ExprId) {
        match body.expr(expr) {
            Expr::Call { callee, args } => {
                self.calls.push((body_id, expr, *callee));
                self.walk_expr(body_id, body, *callee);
                self.walk_exprs(body_id, body, args);
            }
            Expr::MethodCall {
                receiver,
                method,
                args,
            } => {
                self.receivers.push((body_id, *receiver, method.clone()));
                self.walk_expr(body_id, body, *receiver);
                self.walk_exprs(body_id, body, args);
            }
            Expr::Index { base, index, .. } => {
                self.walk_expr(body_id, body, *base);
                self.walk_expr(body_id, body, *index);
            }
            Expr::Binary { lhs, rhs, .. } => {
                self.walk_expr(body_id, body, *lhs);
                self.walk_expr(body_id, body, *rhs);
            }
            Expr::Unary { operand, .. } => self.walk_expr(body_id, body, *operand),
            Expr::Truncate(inner) => self.walk_expr(body_id, body, *inner),
            Expr::Table { entries } => {
                for entry in entries {
                    match entry {
                        TableEntry::Positional(value) | TableEntry::Named { value, .. } => {
                            self.walk_expr(body_id, body, *value);
                        }
                        TableEntry::Keyed { key, value } => {
                            self.walk_expr(body_id, body, *key);
                            self.walk_expr(body_id, body, *value);
                        }
                    }
                }
            }
            Expr::Function(_) | Expr::Literal(_) | Expr::Name(_) | Expr::Vararg | Expr::Error => {}
        }
    }
}

/// Whether an expression is a literal Lua treats as false — the only
/// conditions [`Shapes::walk_block`] decides.
fn is_falsy_literal(expr: &Expr) -> bool {
    matches!(expr, Expr::Literal(Literal::Nil | Literal::Bool(false)))
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
