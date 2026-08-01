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

use std::collections::HashSet;

use luabox_diag::Code;
use luabox_hir::{BindingId, Expr, ExprId, HirId, Literal, Resolution, Stmt, TableEntry};

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
/// - the carrier declares no *other* metafield (see
///   [`Carriers::is_operator_table`]): a table carrying `__call`,
///   `__tostring`, `__add` or `__mode` is a metatable with a purpose that is
///   not instance lookup, and there is nothing for a missing `__index` to
///   break.
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
                if carriers.index_is_settled(binding) || carriers.is_operator_table(binding) {
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
    settled: HashSet<BindingId>,
    operator_tables: HashSet<BindingId>,
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
    /// - `local mt = C` — an alias. Every write through `mt` is a write to the
    ///   same table, and this pass follows bindings rather than values, so
    ///   `mt.__index = mt` settles nothing it can see. Suppressing on the
    ///   alias itself is the same trade as the computed key above: the write
    ///   might be `__index`, and `local mt = Counter; mt.__index = mt;
    ///   setmetatable({}, Counter)` is idiomatic, correct code the rule used
    ///   to warn on (Shockwave round 4);
    /// - `C = <anything>` — the binding is reassigned, so the value the
    ///   `---@class` annotation described is not necessarily what reaches
    ///   `setmetatable`.
    ///
    /// Separately it records **operator tables**: carriers that declare some
    /// *other* metafield (`__call`, `__tostring`, `__add`, `__mode`, …).
    /// `setmetatable(t, Vec)` where `Vec` declares only `__tostring` is a
    /// deliberate operator metatable, not a broken class — there is no
    /// instance lookup to fail, and the rule used to warn on it.
    fn build(ctx: &LintContext<'_>) -> Self {
        let mut settled: HashSet<BindingId> = HashSet::new();
        let mut operator_tables: HashSet<BindingId> = HashSet::new();
        for (body_id, body) in ctx.lowered.bodies() {
            let resolve =
                |expr: ExprId| binding_of(ctx.lowered.resolution(HirId::expr(body_id, expr)));

            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Assign { targets, .. } => {
                        for &target in targets {
                            match body.expr(target) {
                                // `C.__index = …`, `C.__call = …`, `C[k] = …`
                                // — and `function C.__tostring(v) … end`,
                                // which lowers to exactly this shape.
                                Expr::Index { base, index, .. } => {
                                    if !matches!(body.expr(*base), Expr::Name(_)) {
                                        continue;
                                    }
                                    match key_kind(index_key(body.expr(*index))) {
                                        KeyKind::Index => settled.extend(resolve(*base)),
                                        KeyKind::Metafield => {
                                            operator_tables.extend(resolve(*base));
                                        }
                                        KeyKind::Plain => {}
                                    }
                                }
                                // `C = <anything>` — the annotated value is
                                // no longer necessarily the one in play.
                                Expr::Name(_) => settled.extend(resolve(target)),
                                _ => {}
                            }
                        }
                    }
                    // `local C = { __index = … }` — the key in the carrier's
                    // own constructor — and `local mt = C`, an alias. Both
                    // pair names with values positionally, exactly as Lua
                    // does.
                    Stmt::Local { names, init } => {
                        for (name, &value) in names.iter().zip(init) {
                            match body.expr(value) {
                                Expr::Table { entries } => {
                                    let kinds =
                                        || entries.iter().filter_map(|e| entry_kind(body, e));
                                    if kinds().any(|kind| kind == KeyKind::Index) {
                                        settled.insert(name.binding);
                                    }
                                    if kinds().any(|kind| kind == KeyKind::Metafield) {
                                        operator_tables.insert(name.binding);
                                    }
                                }
                                // The aliased binding, not the new one: it is
                                // the carrier that reaches `setmetatable`.
                                Expr::Name(_) => settled.extend(resolve(value)),
                                _ => {}
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
                let Some(&carrier) = args.first() else {
                    continue;
                };
                // No key argument at all is malformed source; read it the way
                // an unreadable key is read.
                let kind = args
                    .get(1)
                    .map_or(KeyKind::Index, |&key| key_kind(index_key(body.expr(key))));
                match kind {
                    KeyKind::Index => settled.extend(resolve(carrier)),
                    KeyKind::Metafield => operator_tables.extend(resolve(carrier)),
                    KeyKind::Plain => {}
                }
            }
        }
        Self {
            settled,
            operator_tables,
        }
    }

    /// Whether this carrier's `__index` is assigned, or written in a way this
    /// pass cannot read — either way the rule stays silent.
    fn index_is_settled(&self, binding: BindingId) -> bool {
        self.settled.contains(&binding)
    }

    /// Whether this carrier declares a metafield other than `__index`, which
    /// makes it a metatable with a purpose instance lookup is not.
    fn is_operator_table(&self, binding: BindingId) -> bool {
        self.operator_tables.contains(&binding)
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
