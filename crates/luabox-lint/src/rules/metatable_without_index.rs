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
    BindingId, BindingKind, Body, Expr, ExprId, HirId, Literal, LoweredFile, Resolution, Stmt,
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
///   …) **and** carries no instance methods (see
///   [`Carriers::is_operator_table`] and [`Carriers::has_instance_methods`]).
///   `Vec.__tostring` on a carrier nothing is ever called on is a deliberate
///   operator metatable and there is nothing for a missing `__index` to
///   break; the same `__tostring` on a carrier that also declares
///   `function Vec:length()` is a class that happens to overload an operator,
///   and `v:length()` still crashes.
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
                let Expr::Call { callee, args } = expr else {
                    continue;
                };
                if !matches!(body.expr(*callee), Expr::Name(n) if n == "setmetatable") {
                    continue;
                }
                if !matches!(
                    ctx.lowered.resolution(HirId::expr(body_id, *callee)),
                    Some(Resolution::Global(name)) if name == "setmetatable"
                ) {
                    continue;
                }
                if args.len() != 2 {
                    continue;
                }
                let meta = args[1];
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
                // An operator metatable has no instance lookup to break — but
                // only when nothing is ever looked up through it. A carrier
                // with colon methods on it needs `__index` no matter how many
                // operators it also overloads.
                if carriers.is_operator_table(carrier) && !carriers.has_instance_methods(carrier) {
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
    instance_methods: HashSet<BindingId>,
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
    /// Separately it records two things the suppression decision needs:
    ///
    /// - **operator tables**: carriers that declare some *other* metafield
    ///   (`__call`, `__tostring`, `__add`, `__mode`, `__gc`, `__name`, …);
    /// - **instance methods**: carriers with a colon-declared function on them
    ///   (`function C:m()`), which is precisely what instance lookup — and so
    ///   `__index` — is needed for.
    ///
    /// Only a carrier with a metafield **and** no instance method is treated
    /// as an operator metatable. A colon method is the discriminator because
    /// it is unambiguous: `self` is implicit, so the function is written to be
    /// reached as `instance:m()`, which is the lookup a missing `__index`
    /// breaks. A `---@field` is not counted — the canonical carrier declares
    /// `---@field n integer` for a *data* field on the instance, not a method
    /// — and neither is a dot-assigned function field (`C.new = function() …`,
    /// the idiomatic constructor, is called as `C.new()`, never through the
    /// metatable). Both are false-negative-shaped omissions: a carrier that
    /// declares only `---@field`-style methods alongside a metafield stays
    /// silent.
    fn build(ctx: &LintContext<'_>) -> Self {
        let aliases = Aliases::build(ctx);
        let mut settled: HashSet<BindingId> = HashSet::new();
        let mut operator_tables: HashSet<BindingId> = HashSet::new();
        let mut instance_methods: HashSet<BindingId> = HashSet::new();
        for (body_id, body) in ctx.lowered.bodies() {
            let resolve =
                |expr: ExprId| binding_of(ctx.lowered.resolution(HirId::expr(body_id, expr)));
            // The table a write through this expression lands on, alias chains
            // followed to their root.
            let carrier_of = |expr: ExprId| resolve(expr).map(|b| aliases.root(b));

            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Assign { targets, values } => {
                        for (slot, &target) in targets.iter().enumerate() {
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
                                        // Only an ordinary key names an
                                        // instance method; `function
                                        // C:__call(…)` is a metafield spelled
                                        // with a colon, not a method lookup.
                                        KeyKind::Plain => {
                                            if values.get(slot).is_some_and(|&v| {
                                                is_colon_method(ctx.lowered, body, v)
                                            }) {
                                                instance_methods.insert(carrier);
                                            }
                                        }
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
        Self {
            aliases,
            settled,
            operator_tables,
            instance_methods,
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

    /// Whether this carrier declares a colon method (`function C:m()`) — the
    /// lookup a missing `__index` breaks.
    fn has_instance_methods(&self, binding: BindingId) -> bool {
        self.instance_methods.contains(&binding)
    }
}

/// `local mt = C`: which binding each local alias ultimately names.
///
/// Only `local` aliases of a *name* are followed. Storing the carrier in a
/// table, passing it to a function or returning it are all values this pass
/// does not track, and each stays exactly as unknown as it was.
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
        Self(direct)
    }

    /// Follow `local b = a; local a = C` to `C`. Every `local` introduces a
    /// fresh binding, so the chain is acyclic by construction; the step bound
    /// makes that structural rather than assumed.
    fn root(&self, binding: BindingId) -> BindingId {
        let mut current = binding;
        for _ in 0..=self.0.len() {
            match self.0.get(&current) {
                Some(&next) if next != current => current = next,
                _ => break,
            }
        }
        current
    }
}

/// Whether this value is a colon-declared function — `function C:m()`, which
/// lowers to a plain function whose first parameter is the implicit `self`.
fn is_colon_method(lowered: &LoweredFile, body: &Body, value: ExprId) -> bool {
    let Expr::Function(fn_body) = body.expr(value) else {
        return false;
    };
    lowered
        .body(*fn_body)
        .params
        .first()
        .is_some_and(|&param| lowered.binding(param).kind == BindingKind::SelfParam)
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
