//! The `LB03xx` diagnostic codes this crate emits — the Semantics block of the
//! `luabox-diag` registry.
//!
//! Three of these are `pub`: `LB0317`, `LB0318` and `LB0319` are emitted by
//! `luabox-cli`'s own syntactic pre-check as well as by this crate, and a
//! code number written as a `Code::new(317)` literal on the far side of a
//! crate boundary is a constant with two owners and no compiler-enforced
//! link between them (local merge-gate finding; the *suppression-name* half
//! of the same pair had already drifted once). The rest stay `pub(crate)` —
//! nothing outside this crate emits them.
//!
//! One `const` per code, so both emission sites and the code *comparisons* on
//! the check path (`---@diagnostic` suppression, cascade collapsing) speak in
//! [`Code`] values. `Code` is a `Copy` newtype over a `u16` with derived
//! equality: comparing two of them is an integer compare, where matching on
//! `code.to_string()` allocated a `String` per diagnostic per check.

use luabox_diag::Code;

/// A value does not fit the type of the slot it flows into.
pub(crate) const TYPE_MISMATCH: Code = Code::new(300);
/// A call passes too few or too many arguments.
pub(crate) const WRONG_ARG_COUNT: Code = Code::new(301);
/// A table literal omits a required `---@field`.
pub(crate) const MISSING_FIELD: Code = Code::new(302);
/// A table literal declares a field its target type does not have.
pub(crate) const UNKNOWN_FIELD: Code = Code::new(303);
/// A `return` value does not fit the declared `---@return`.
pub(crate) const RETURN_MISMATCH: Code = Code::new(304);
/// An annotation names a type that is nowhere declared.
pub(crate) const UNKNOWN_TYPE_NAME: Code = Code::new(305);
/// A read of a provably absent field (luals `undefined-field`).
pub(crate) const FIELD_NOT_FOUND: Code = Code::new(306);
/// One `---@class` name is declared by more than one definition package.
pub(crate) const CLASS_COLLISION: Code = Code::new(307);
/// Use of a `---@deprecated` symbol (luals `deprecated`, #111).
pub(crate) const DEPRECATED: Code = Code::new(308);
/// Discarded return of a `---@nodiscard` call (luals `discard-returns`, #112).
pub(crate) const DISCARD_RETURNS: Code = Code::new(309);
/// One `---@alias` name is declared more than once (luals
/// `duplicate-doc-alias`).
pub(crate) const ALIAS_COLLISION: Code = Code::new(310);
/// Duplicate `---@field` on one class (luals `duplicate-doc-field`).
pub(crate) const DUPLICATE_DOC_FIELD: Code = Code::new(311);
/// Access of a `---@private`/`---@protected`/`---@package` member from outside
/// its visibility scope (luals `invisible`, #115).
pub(crate) const INVISIBLE: Code = Code::new(312);
/// Wrong number of generic type arguments (`Name<A, B>` vs its params, #117).
pub(crate) const GENERIC_ARITY: Code = Code::new(313);
/// A self- or mutually-referential `---@alias` cycle (luals parity, #123).
pub(crate) const CYCLIC_ALIAS: Code = Code::new(314);
/// A non-exhaustive `if`/`elseif` chain dispatching on a finite (literal-union
/// or `---@enum`) discriminant with no `else` branch (#121).
pub(crate) const NON_EXHAUSTIVE_IF: Code = Code::new(315);
/// Call to a `---@async` function from a non-async enclosing function (luals
/// `await-in-sync`).
pub(crate) const AWAIT_IN_SYNC: Code = Code::new(316);
/// A `---@class` ancestry deep enough to trip the class-shape/operator
/// walk's depth cap before it can overflow the stack (round 5 review N2's
/// durable fix, `env::MAX_ANCESTRY_DEPTH`). luabox-only — no luals
/// equivalent, since luals has no such recursive merge to protect.
///
/// **Not restricted to a single-parent chain**: `DiamondGuard::should_apply`
/// trips on `on_path.len()`, the depth of the *current recursion path*, so
/// `---@class A : B, C` with one branch past the cap reports this exactly as
/// a strict chain does. The CLI's separate syntactic pre-check
/// (`check_cmd::class_ancestry_precheck`) follows every named parent
/// too (round 6 review M4(c) — it used to treat any multi-parent class as a
/// root and not follow it at all, so the identical depth flipped from
/// `error`/exit 1 to `warning`/exit 0 the moment an unrelated second parent
/// was added), so the two guards agree on reach; what the pre-check still
/// does not follow is a parent expressed as anything other than a bare
/// name (a union, a table literal, a generic argument, ...) — it
/// under-counts there rather than duplicating this resolver's own walk.
pub const CLASS_DEPTH_LIMIT: Code = Code::new(317);
/// A `---@class` that is its own ancestor — `---@class A : A`, or a mutual
/// `A : B` / `B : A` (round 6 review M67). [`CYCLIC_ALIAS`]'s counterpart on
/// the class axis: `DiamondGuard`'s `on_path` guard already stops the walk,
/// so this never crashed, but the cycle resolved *silently* and a user whose
/// intent was to inherit from something else got no signal at all.
///
/// **Parity, not luabox being stricter** — unlike [`CLASS_DEPTH_LIMIT`].
/// lua-language-server 3.13.5 reports `circle-doc-class` ("Circularly
/// inherited classes") on *both* shapes at `--checklevel=Warning`, measured
/// against the pinned binary (corpus rows `cyclic_class_self` /
/// `cyclic_class_mutual`). This doc previously claimed luals "reports nothing
/// for either shape"; that was asserted, never measured, and round 8 measured
/// it false.
///
/// Parity **on the declaration**, not merely on a resolved class (round 11
/// review R11-1). This ledger is filled by [`crate::env::TypeEnv::note_cyclic`]
/// from a resolution walk, so on its own it says nothing about a class no
/// file references — and luals' check fires from the `---@class` node alone
/// (measured against the pinned binary on a fixture with zero uses; corpus
/// row `cyclic_class_unreferenced`). That half is closed **syntactically**,
/// by [`crate::ClassGraph`]: strongly connected components of the declared
/// `---@class` graph, found before anything is resolved, reported at every
/// declaration that carries a cycle edge, and deduped against this ledger's
/// findings by declaration site.
///
/// That parity is claimed on the shapes measured, up to luals's own ceiling:
/// its walk stops after 999 ancestors
/// (`script/core/diagnostics/circle-doc-class.lua:27`), while
/// [`crate::ClassGraph`]'s Tarjan walk is unbounded, so a ring of ~1,000+
/// declarations is reported here and silent there.
///
/// **Both** surfaces run it (round 12 review R12-1) — `luabox check`, in
/// `check_cmd::cyclic_class_diagnostics`, over the project's files; and the
/// LSP, in `luabox_lsp::diagnostics::class_cycle_diagnostics`, over the
/// workspace's, publishing each member under its own declaring document.
/// One seam, one algorithm, one message. It shipped CLI-only for one round,
/// and this comment claimed instead that "the LSP resolves what it displays,
/// so the same cycle surfaces there when the file is opened": it did not —
/// resolution is reference-driven, which is the whole reason the syntactic
/// half exists — and the editor was green on the exact fixture the feature
/// targets while CI was red. A direct `check_file*` caller that is neither
/// (this crate's own tests, an embedder calling the checker) still sees only
/// the resolution-driven half; running the declaration-driven one is one
/// [`crate::ClassGraph`] away.
///
/// Downgradable the same way every other `LB03xx` is: `[types]
/// strict = false` makes it a warning and `---@diagnostic
/// disable[-line|-next-line]: circle-doc-class` suppresses it —
/// luals' own rule name ([`crate::directive::RULE_CIRCLE_DOC_CLASS`], the
/// single owner of it), so the muscle-memory directive works unchanged.
pub const CYCLIC_CLASS: Code = Code::new(318);
/// A `---@class` ancestry whose *resolution cost* — not its depth — exceeds
/// what the merge walk will spend on it (`env::MAX_ANCESTRY_RESOLUTIONS`).
///
/// Distinct from [`CLASS_DEPTH_LIMIT`] because the cause and the remedy are
/// different, and reporting one as the other actively misleads. The depth cap
/// protects the native stack and is fixed by *flattening*; this bounds the
/// re-resolution a conflicting diamond forces and is fixed by making the
/// conflict go away — binding a shared generic ancestor the same way on every
/// branch, or not reaching it twice.
///
/// It counts **re**-resolutions only. A wide, shallow hierarchy — one
/// `---@class` per generated binding, hundreds of them, each visited once —
/// costs one visit per node, resolves in linear time, and must not trip this:
/// measured, a 601-class / depth-6 hierarchy checks in 0.09 s. Counting first
/// visits rejected exactly that shape (round 6 review, local merge-gate).
pub const CLASS_COST_LIMIT: Code = Code::new(319);
/// A `---@class` header with no usable class name — nothing after the tag
/// (`---@class`), a parent list where the name should be (`---@class : Base`),
/// or a name token that is not an identifier (`---@class 123abc`).
///
/// The header parses, so nothing fails loudly; what it does not do is produce
/// a class. Before this code existed the header was simply *dropped* — every
/// `---@field` under it belonged to nothing, and the only signal a user ever
/// got was an [`UNKNOWN_TYPE_NAME`] at some later `---@type` that referenced
/// the name the class never got, arbitrarily far from the mistake (round 6
/// review M66, #69). A machine-generated header with a typo'd name produced a
/// class that silently did not exist.
///
/// **Parity, not luabox being stricter.** lua-language-server 3.13.5 reports
/// `luadoc-miss-class-name` ("`<class name> expected`") at the declaration for
/// all three shapes — it treats a name token it cannot lex as an identifier
/// the same as a missing one — plus a `doc-field-no-class` on each orphaned
/// `---@field` beneath it. luabox reports the header itself once and leaves
/// the fields alone: one mistake, one diagnostic.
///
/// Downgradable like every other `LB03xx`: `[types] strict = false` makes it a
/// warning, and `---@diagnostic disable[-line|-next-line]:
/// luadoc-miss-class-name` suppresses it — luals' own rule name
/// ([`crate::directive::RULE_LUADOC_MISS_CLASS_NAME`]).
pub(crate) const MALFORMED_CLASS_NAME: Code = Code::new(320);
/// A `---@class` extends list entry that is not a class name — a trailing
/// comma (`---@class A : P,`), a doubled one, a bare `:` with nothing after
/// it, or a token the type parser cannot read as a name at all
/// (`---@class A : ?`).
///
/// All of those arrive here as one thing: `luacats::TypeExprKind::Error`, the
/// type parser's universal recovery node. It does not distinguish "there was
/// nothing to read" from "there was something and it was not a type", so this
/// diagnostic does not claim to either — it says the entry is not a class
/// name, which is true of every shape that reaches it. Claiming a name is
/// *missing* would be wrong on `: ?`, where a token is present and merely
/// unreadable, and this fires at `Severity::Error` under `strict = true`.
///
/// Distinct from [`MALFORMED_CLASS_NAME`] because the cause and the remedy are
/// different: the class exists and is usable, one *edge* of its ancestry does
/// not. Distinct from [`UNKNOWN_TYPE_NAME`] for the same reason — `---@class
/// A : P,` with `P` undeclared already reported LB0305 on `P`, and that
/// finding is about a name that is spelled fine and declared nowhere, not
/// about a list entry that has no name at all. Both fire; luals reports both
/// too (round 6 review M66, #69).
///
/// **Parity.** lua-language-server 3.13.5 reports
/// `luadoc-miss-class-extends-name` ("`<class extends name> expected`") at the
/// comma, alongside its own `undefined-doc-class` on `P`. Suppressed by that
/// same rule name ([`crate::directive::RULE_LUADOC_MISS_CLASS_EXTENDS_NAME`]).
pub(crate) const MALFORMED_CLASS_EXTENDS_ENTRY: Code = Code::new(321);
