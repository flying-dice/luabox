//! Target lowering and polyfill injection — the tsc bit. **Emit** bounded
//! context (SPEC.md §2.1, §16).
//!
//! [`lower`] rewrites a check-clean source file from the `edition` dialect
//! (`from`) to the `target` dialect (`to`) as an ordered set of rules, each a
//! doc-commented module carrying its semantics-preservation argument:
//!
//! 1. [`gotos`] — `goto`/labels → 5.1 control-flow restructure (reducible
//!    shapes only; irreducible = hard `LB0601`).
//! 2. [`env`] — explicit `_ENV` (5.2+) → `setfenv`/`getfenv` (5.1/LuaJIT);
//!    exotic uses = hard `LB0604`.
//! 3. [`attribs`] — `<const>` → plain local + compile-time reassignment
//!    check (`LB0602`); `<close>` → scope-exit `pcall` rewrite via
//!    `__luabox_rt.close_scope` (`LB0603` warn/error tiers).
//! 4. [`floor_div`] + [`bitops`] + [`jit_ext`] — expression rewrites:
//!    `//` → `math.floor(a / b)`, bitwise operators → `__luabox_rt` helper
//!    calls, LuaJIT `bit.*`/`require("bit")` → `__luabox_rt`; `ffi` = hard
//!    `LB0605`.
//! 5. [`int_float`] — integer/float divergence heuristics (`LB0606`,
//!    warn tier, conservative).
//! 6. [`polyfill`] — the single tree-shaken `__luabox_rt` prelude; zero-cost
//!    (no prelude at all) when no helper is used.
//!
//! The engine operates as targeted text edits over the lossless
//! `luabox-syntax` tree (rust-analyzer style): untransformed input text is
//! preserved byte-for-byte, never reprinted.
//!
//! Like `luabox-syntax`, this crate does not depend on `luabox-diag`
//! (acyclic dep graph, SPEC.md §16): diagnostics carry plain `LBnnnn` code
//! strings the CLI maps onto registered diagnostics.
//!
//! # What this crate does *not* prove
//!
//! Semantics preservation is argued per rule (doc comments) and enforced
//! mechanically by the reparse-under-target property tests; differential
//! execution against real runtimes is CI-level (SPEC.md §16.1/§16.2) and
//! lands with the toolchain manager work (ticket #23).

use std::collections::{BTreeSet, HashMap};

use luabox_syntax::lua::SyntaxNode;
use luabox_syntax::{Dialect, lua};
use rowan::TextRange;

mod attribs;
mod bitops;
mod diag;
mod edit;
mod env;
mod floor_div;
mod gotos;
mod int_float;
mod jit_ext;
mod polyfill;
mod rewrite;

pub use diag::{LowerDiagnostic, Severity};
pub use polyfill::Helper;

/// The result of a successful [`lower`]: the emitted text plus which
/// `__luabox_rt` polyfill helpers it uses (empty = no prelude was injected)
/// and any warn-tier diagnostics.
#[derive(Debug, Clone)]
pub struct Lowered {
    /// The lowered source text, legal under the target dialect.
    pub text: String,
    /// Names of the `__luabox_rt` helpers the output uses, sorted. Empty
    /// means the output carries no polyfill prelude at all (zero-cost).
    pub polyfills: Vec<&'static str>,
    /// Warn-tier diagnostics (`LB0603` fidelity notes, `LB0606` divergence
    /// heuristics). Never contains errors — those make [`lower`] fail.
    pub warnings: Vec<LowerDiagnostic>,
}

/// Lower `source` from the `from` dialect to the `to` dialect (SPEC.md §2.1).
///
/// `from == to` is the identity: the output is byte-identical to the input
/// (`luabox build` invariant). Any error-tier diagnostic — irreducible
/// `goto` (`LB0601`), `<const>` reassignment (`LB0602`), non-lowerable
/// `<close>` scope tail (`LB0603`), exotic `_ENV` (`LB0604`), `ffi` and
/// other non-polyfillable LuaJIT extensions (`LB0605`), or a parse error
/// (`LB0001`) — fails the whole file; the `Err` vector carries every
/// diagnostic collected (warnings included) in source order.
///
/// The input is expected to be check-clean under `from` (`luabox build`
/// runs `luabox check` first); constructs the rules cannot lower (e.g. hex
/// floats targeting 5.1) are left untouched here and surface via the
/// caller's residual validation of the output under `to`.
pub fn lower(source: &str, from: Dialect, to: Dialect) -> Result<Lowered, Vec<LowerDiagnostic>> {
    lower_impl(source, from, to, true)
}

/// Like [`lower`], but the returned text carries **no** `__luabox_rt`
/// prelude even when helpers are used ([`Lowered::polyfills`] still lists
/// them). For callers that hoist one shared prelude over many lowered
/// files — the bundler dedupes the prelude across all modules of a bundle
/// and emits it once at bundle top via [`rt_prelude`].
pub fn lower_bare(
    source: &str,
    from: Dialect,
    to: Dialect,
) -> Result<Lowered, Vec<LowerDiagnostic>> {
    lower_impl(source, from, to, false)
}

/// Render the `__luabox_rt` prelude a set of helpers needs, or `None` when
/// the set is empty (zero-cost invariant). The bundler pairs this with
/// [`lower_bare`]: lower every module bare, union their helper sets
/// ([`Helper::from_name`] round-trips [`Lowered::polyfills`]), emit one
/// prelude. `from`/`to` select the backend exactly as [`lower`] does.
pub fn rt_prelude(used: &BTreeSet<Helper>, from: Dialect, to: Dialect) -> Option<String> {
    polyfill::prelude(used, from, to)
}

fn lower_impl(
    source: &str,
    from: Dialect,
    to: Dialect,
    with_prelude: bool,
) -> Result<Lowered, Vec<LowerDiagnostic>> {
    if from == to {
        return Ok(Lowered {
            text: source.to_owned(),
            polyfills: Vec::new(),
            warnings: Vec::new(),
        });
    }

    let parse = lua::parse(source, from);
    if !parse.errors().is_empty() {
        return Err(parse
            .errors()
            .iter()
            .map(|e| LowerDiagnostic::error(diag::PARSE_ERROR, e.message.clone(), e.range))
            .collect());
    }

    let root = parse.syntax();
    let mut ctx = Ctx::new(source, from, to);
    let mut edits = Vec::new();

    // Ordered rule passes. Statement-level restructures run first so the
    // expression pass can skip the ranges they replaced (`Ctx::replaced`);
    // replacement text that embeds expressions is produced through
    // `rewrite::render`, so nested expression rules still apply inside it.
    gotos::run(&root, &mut ctx, &mut edits);
    env::run(&root, &mut ctx, &mut edits);
    attribs::run(&root, &mut ctx, &mut edits);
    rewrite::run(&root, &mut ctx, &mut edits);
    jit_ext::scan_diags(&root, &mut ctx);
    int_float::run(&root, &mut ctx);

    let mut diags = ctx.diags;
    diags.sort_by_key(|d| (d.range.start(), d.range.end(), d.code));
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(diags);
    }

    let mut text = edit::apply(source, edits);
    if with_prelude && let Some(prelude) = polyfill::prelude(&ctx.helpers, from, to) {
        text.insert_str(0, &prelude);
    }
    Ok(Lowered {
        text,
        polyfills: ctx.helpers.iter().map(|h| h.name()).collect(),
        warnings: diags,
    })
}

/// `//` and the bitwise operators are 5.3+ (never LuaJIT) — SPEC.md §2.
fn has_53_ops(dialect: Dialect) -> bool {
    matches!(dialect, Dialect::Lua53 | Dialect::Lua54)
}

/// `_ENV` exists from 5.2 on; 5.1 and LuaJIT use `setfenv`/`getfenv`.
fn has_env(dialect: Dialect) -> bool {
    matches!(dialect, Dialect::Lua52 | Dialect::Lua53 | Dialect::Lua54)
}

/// Shared state for one [`lower`] call: which rules are active for the
/// `(from, to)` pair, plus everything the rules accumulate.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one independent gate per lowering rule, not a state machine"
)]
pub(crate) struct Ctx<'a> {
    pub source: &'a str,
    pub to: Dialect,
    /// `//` → `math.floor(a / b)` (5.3+ source, pre-5.3 target).
    pub floor_div: bool,
    /// `& | ~ << >>` and unary `~` → `__luabox_rt` calls.
    pub bitops: bool,
    /// `goto`/labels → control-flow restructure (5.1 target).
    pub gotos: bool,
    /// `<const>`/`<close>` → const check / scope-exit rewrite (5.4 source).
    pub attribs: bool,
    /// Explicit `_ENV` → `setfenv`/`getfenv` (5.1/LuaJIT target).
    pub env: bool,
    /// LuaJIT `bit.*`/`require("bit")` → `__luabox_rt`; `ffi` diagnostics.
    pub jit_bit: bool,
    /// Integer/float divergence heuristics (`LB0606`).
    pub int_float: bool,
    /// Tree-shaken set of `__luabox_rt` helpers the output uses.
    pub helpers: BTreeSet<Helper>,
    /// Every diagnostic collected, warn and error tier alike.
    pub diags: Vec<LowerDiagnostic>,
    /// Ranges fully replaced by statement-level rules; the expression pass
    /// skips them (their replacement text was rendered with the expression
    /// rules already applied).
    pub replaced: Vec<TextRange>,
    /// Counter for `__luabox_skip_<n>` flag names (forward-goto rewrite).
    pub skip_flags: u32,
    /// `LB0606` is emitted once per file for `//` lowering, not per operator.
    pub floor_div_warned: bool,
    /// Per-block goto-rewrite intervals over statement indices, recorded by
    /// [`gotos`] and consulted by [`attribs`] for the `<close>` nesting
    /// check.
    pub goto_intervals: HashMap<SyntaxNode, Vec<(usize, usize)>>,
}

impl<'a> Ctx<'a> {
    fn new(source: &'a str, from: Dialect, to: Dialect) -> Self {
        let op_delta = has_53_ops(from) && !has_53_ops(to);
        Ctx {
            source,
            to,
            floor_div: op_delta,
            bitops: op_delta,
            gotos: from.has_goto() && !to.has_goto(),
            attribs: from == Dialect::Lua54 && to != Dialect::Lua54,
            env: has_env(from) && !has_env(to),
            jit_bit: from == Dialect::LuaJit && to != Dialect::LuaJit,
            int_float: op_delta,
            helpers: BTreeSet::new(),
            diags: Vec::new(),
            replaced: Vec::new(),
            skip_flags: 0,
            floor_div_warned: false,
            goto_intervals: HashMap::new(),
        }
    }

    /// Whether `range` lies inside a range a statement-level rule replaced.
    pub(crate) fn is_replaced(&self, range: TextRange) -> bool {
        self.replaced.iter().any(|r| r.contains_range(range))
    }

    /// The whitespace indentation of the line `offset` sits on, when the
    /// text between the preceding newline and `offset` is all whitespace
    /// (i.e. the construct starts its line); empty otherwise. Used to keep
    /// inserted wrapper lines aligned with the statements they wrap.
    #[expect(
        clippy::string_slice,
        reason = "`offset` is a TextSize from the syntax tree over `self.source` (a char boundary within bounds); `line_start` is one past an ASCII '\\n', also a char boundary"
    )]
    pub(crate) fn indent_at(&self, offset: rowan::TextSize) -> &'a str {
        let upto = &self.source[..usize::from(offset)];
        let line_start = upto.rfind('\n').map_or(0, |i| i + 1);
        let prefix = &upto[line_start..];
        if prefix.chars().all(char::is_whitespace) {
            prefix
        } else {
            ""
        }
    }

    /// A fresh `__luabox_skip_<n>` flag name (forward-goto rewrite).
    pub(crate) fn fresh_skip_flag(&mut self) -> String {
        self.skip_flags += 1;
        format!("__luabox_skip_{}", self.skip_flags)
    }
}
