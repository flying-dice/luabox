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
///   spelling this pass can see (see [`Carriers::index_is_settled`]).
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
                if carriers.index_is_settled(binding) {
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
                         on `t` looks the name up in `{name}.{INDEX}`, which is `nil` here — \
                         every `t:method()` is `attempt to call a nil value` at runtime"
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

/// Which carrier bindings have had their `__index` settled — either genuinely
/// assigned, or written in a way this pass cannot read, which counts the same
/// because a rule that guesses is a rule that cries wolf.
struct Carriers {
    settled: HashSet<BindingId>,
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
    /// - `C[k] = …` with a key this pass cannot evaluate, and
    ///   `rawset(C, …)` — the write *might* be `__index`, so the carrier is
    ///   treated as settled;
    /// - `C = <anything>` — the binding is reassigned, so the value the
    ///   `---@class` annotation described is not necessarily what reaches
    ///   `setmetatable`.
    fn build(ctx: &LintContext<'_>) -> Self {
        let mut settled: HashSet<BindingId> = HashSet::new();
        for (body_id, body) in ctx.lowered.bodies() {
            let resolve =
                |expr: ExprId| binding_of(ctx.lowered.resolution(HirId::expr(body_id, expr)));

            for (_, stmt) in body.stmts() {
                match stmt {
                    Stmt::Assign { targets, .. } => {
                        for &target in targets {
                            match body.expr(target) {
                                // `C.__index = …`, `C[k] = …`.
                                Expr::Index { base, index, .. } => {
                                    if !matches!(body.expr(*base), Expr::Name(_)) {
                                        continue;
                                    }
                                    if index_key(body.expr(*index)).is_none_or(|key| key == INDEX) {
                                        settled.extend(resolve(*base));
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
                    // own constructor. `Stmt::Local` pairs names with values
                    // positionally, exactly as Lua does.
                    Stmt::Local { names, init } => {
                        for (name, &value) in names.iter().zip(init) {
                            if let Expr::Table { entries } = body.expr(value)
                                && entries.iter().any(|entry| declares_index(body, entry))
                            {
                                settled.insert(name.binding);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // `rawset(C, …)` bypasses metamethods but still writes the field.
            for (_, expr) in body.exprs() {
                let Expr::Call { callee, args } = expr else {
                    continue;
                };
                if !matches!(body.expr(*callee), Expr::Name(n) if n == "rawset") {
                    continue;
                }
                if let Some(&first) = args.first() {
                    settled.extend(resolve(first));
                }
            }
        }
        Self { settled }
    }

    /// Whether this carrier's `__index` is assigned, or written in a way this
    /// pass cannot read — either way the rule stays silent.
    fn index_is_settled(&self, binding: BindingId) -> bool {
        self.settled.contains(&binding)
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

/// Whether a table-constructor entry declares `__index` — or a key this pass
/// cannot evaluate, which counts the same.
fn declares_index(body: &luabox_hir::Body, entry: &TableEntry) -> bool {
    match entry {
        TableEntry::Positional(_) => false,
        TableEntry::Named { name, .. } => name == INDEX,
        TableEntry::Keyed { key, .. } => index_key(body.expr(*key)).is_none_or(|k| k == INDEX),
    }
}
