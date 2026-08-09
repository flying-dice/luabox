//! The per-file type environment: every declaration the annotations make.
//!
//! Built in two passes over [`luacats::harvest`] output: first collect the
//! *names* of classes/aliases/enums (so forward references resolve), then
//! lower every annotation body against them. The environment is strictly
//! per file (cross-file *class/alias/enum* sharing is the ambient
//! definition layer's job — #108); cross-file `require` *values* are typed
//! separately, by threading a module-export registry into inference rather
//! than into this environment (see [`crate::check_file_with_requires`], #85).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use luabox_syntax::lua::ast::{AstNode, Expr, LocalStmt, Stmt};
use luabox_syntax::lua::{self, SyntaxKind, SyntaxNode};
use luabox_syntax::luacats::{
    self, AliasTag, CastKind, FieldKey, FieldScope, ParamTag, ReturnTag, Tag, TypeExprKind,
    VarargTag,
};

use crate::lower::{Declared, GenericClass, Lowerer};
use crate::ty::{FieldTy, FunctionTy, OperatorSig, ParamTy, TableTy, Ty, TypeParam};

/// A statement's byte range, used to key annotations to their target.
pub(crate) type Target = (usize, usize);

/// A `---@class Sub : Base<number>` parent reference: the parent's name plus
/// the type arguments this declaration binds to its parameters, lowered.
/// Empty `args` is a parent written bare (`: Base`) or a plain parent with no
/// parameters — in both, the parent's parameters stay free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParentRef {
    pub name: String,
    pub args: Vec<Ty>,
}

/// A declared `---@class`: parents plus *own* members (inherited members
/// are merged on demand by [`TypeEnv::class_shape`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ClassDef {
    pub parents: Vec<ParentRef>,
    pub fields: BTreeMap<String, FieldTy>,
    pub indexers: Vec<(Ty, Ty)>,
    /// Generic type-parameter names from `---@class Name<T>` — empty for a
    /// plain class. A reference `Name<arg>` monomorphises the class shape by
    /// substituting these (#84).
    pub params: Vec<String>,
    /// Members attached to the class *carrier* by statements —
    /// `function Class:method()`, `function Class.fn()`, `Class.const = v` —
    /// as collected from the declaring project file (luals parity:
    /// `---@class` declarations and their member attachments are
    /// workspace-global). They resolve on reads and method calls exactly
    /// like `---@field` members ([`TypeEnv::class_shape`] folds them in) but
    /// carry **no table-literal obligation**: luals's `missing-fields` only
    /// requires `---@field`-declared members, so the literal classifiers
    /// skip these (see [`TypeEnv::class_method_names`]).
    pub methods: BTreeMap<String, FieldTy>,
    /// `---@operator <op>[(<input>)]: <result>` overloads declared on the
    /// class, keyed by operator name (`add`, `sub`, `unm`, `len`, ...). A key
    /// maps to every overload of that operator in declaration order; inference
    /// picks the first whose `input` accepts the other operand (#114). Like
    /// `---@field`s these ride the workspace-global class surface, so an
    /// operator declared on a class in one file applies wherever the class is
    /// used (cross-file / defs-package).
    pub operators: BTreeMap<String, Vec<OperatorSig>>,
    /// Non-public member visibility (luals `invisible`, #115): member name →
    /// its declared scope, recorded only for `private` / `protected` /
    /// `package` members (a public member has no entry). Populated from
    /// `---@field <scope> name` modifiers and from standalone `---@private` /
    /// `---@protected` / `---@package` doc blocks on `function Class:method`
    /// declarations. Like `---@field`s these ride the workspace-global class
    /// surface, so a member's visibility declared on a class in one file is
    /// enforced wherever the class is used (cross-file / defs-package). The
    /// *owner* of a restricted member is the class this map lives on; a
    /// subclass re-declaring the member as a plain `---@field` (no scope)
    /// overrides it back to public (see [`TypeEnv::member_visibility`]).
    pub visibility: BTreeMap<String, FieldScope>,
}

/// A declared `---@enum`: member name → value type, plus the union of all
/// member values (what the enum *type* accepts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnumDef {
    pub members: BTreeMap<String, Ty>,
    pub value_union: Ty,
}

/// A lowered `---@cast var [+|-]T[, ...]` bound to the statement it
/// precedes: applied as a flow-state override when inference reaches that
/// statement.
#[derive(Debug, Clone)]
pub(crate) struct CastEntry {
    pub var: String,
    pub ops: Vec<(CastKind, Ty)>,
}

/// The workspace-global type surface one checked project source file
/// contributes (luals parity: `---@class` declarations are
/// workspace-global): its `---@class` definitions — parents, `---@field`
/// members, and carrier member attachments (`function Class:method` etc.)
/// — plus its `---@enum` definitions and its `---@alias` declarations.
/// Collected per file by [`crate::module_surface`] and merged beneath every
/// other file's declarations via [`crate::Ambient::with_project_types`].
///
/// Aliases are carried **raw** (the harvested [`AliasTag`], not a lowered
/// [`Ty`]) exactly as the ambient definition layer carries them: an alias
/// body is only expanded at *lowering* time, inside the consuming file where
/// every workspace-global class/enum/alias it might reference is already in
/// scope. This defers cross-file alias expansion to the one place all the
/// names resolve, and reuses the lowerer's existing cycle guard so a
/// self-referential or mutually-referential alias across files terminates
/// (collapsing to `unknown`) rather than looping (#110).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileTypes {
    pub(crate) classes: BTreeMap<String, ClassDef>,
    pub(crate) enums: BTreeMap<String, EnumDef>,
    pub(crate) aliases: BTreeMap<String, AliasTag>,
    /// The `---@class` tag span of each class this file declares, paired with
    /// this file's own name — the "declared here" secondary a cross-file
    /// consumer's undefined-field diagnostic points at (#90, round 3 review
    /// F71). `TypeEnv::class_decl_span` only ever answers for the *current*
    /// file's own declarations (populated by `absorb_block` during that
    /// file's own build), so a class merged in from elsewhere needs its span
    /// carried here instead, alongside the file it actually lives in — a
    /// span with no file would silently mislabel whichever file happens to be
    /// checking at the time.
    pub(crate) class_decl_spans: BTreeMap<String, (String, std::ops::Range<usize>)>,
}

impl FileTypes {
    /// Whether this file contributes nothing to the workspace surface.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.enums.is_empty() && self.aliases.is_empty()
    }

    /// Collect the file's own `---@class`/`---@enum`/`---@alias` declarations
    /// out of its built environment, folding each class carrier's reified
    /// member attachments (from inference, `carriers`: class name → reified
    /// carrier shape) into the class's `methods` map. Only names the file
    /// itself declares are collected — ambient (defs/stdlib) declarations
    /// seeded into `env` are not re-exported. Aliases are collected raw from
    /// the harvested tags (expansion is deferred to each consumer's lowerer).
    ///
    /// `file` names the declaring file — carried alongside each class's
    /// `---@class` tag span (round 3 review F71) so a cross-file consumer's
    /// "declared here" secondary label points at the right file, not
    /// whichever one happens to be checking.
    pub(crate) fn collect(
        items: &[luacats::AnnotatedItem],
        env: &TypeEnv,
        carriers: &HashMap<String, Ty>,
        file: &str,
    ) -> FileTypes {
        let mut out = FileTypes::default();
        for item in items {
            for tag in &item.block.tags {
                match tag {
                    Tag::Class(c) if !c.name.is_empty() => {
                        let mut def = env.classes.get(&c.name).cloned().unwrap_or_default();
                        if let Some(Ty::Table(carrier)) = carriers.get(&c.name) {
                            for (member, field) in &carrier.fields {
                                // A same-name `---@field` shadows the attachment
                                // on type, but keeps its use-site tags (#33).
                                if let Some(declared) = def.fields.get(member) {
                                    let merged = with_carrier_tags(declared, field);
                                    def.fields.insert(member.clone(), merged);
                                } else {
                                    def.methods
                                        .insert(member.clone(), publish_carrier_method_ty(field));
                                }
                            }
                        }
                        out.classes.insert(c.name.clone(), def);
                        if let Some(range) = env.class_decl_span(&c.name) {
                            out.class_decl_spans
                                .entry(c.name.clone())
                                .or_insert((file.to_string(), range));
                        }
                    }
                    Tag::Enum(e) if !e.name.is_empty() => {
                        if let Some(def) = env.enums.get(&e.name) {
                            out.enums.insert(e.name.clone(), def.clone());
                        }
                    }
                    Tag::Alias(a) if !a.name.is_empty() => {
                        out.aliases.insert(a.name.clone(), a.clone());
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// The `---@alias` declarations this file contributes to the workspace
    /// (name → raw [`AliasTag`]) — folded into the ambient alias map by
    /// [`crate::Ambient::with_project_types`] so a consumer file can name
    /// them (#110).
    pub(crate) fn aliases(&self) -> &BTreeMap<String, AliasTag> {
        &self.aliases
    }
}

/// Everything the annotations of one file declare.
#[derive(Debug, Default)]
pub struct TypeEnv {
    classes: BTreeMap<String, ClassDef>,
    enums: BTreeMap<String, EnumDef>,
    /// Annotated functions by (dotted) name — `f`, `M.helper`.
    functions: BTreeMap<String, FunctionTy>,
    /// `---@type` annotations keyed by their target `local` statement.
    typed_locals: HashMap<Target, Vec<Ty>>,
    /// The source span of each `---@type` annotation, keyed by its target
    /// `local` statement — the anchor a deferred whole-carrier conformance
    /// error points at.
    typed_local_spans: HashMap<Target, std::ops::Range<usize>>,
    /// Function signatures keyed by their target statement (for return
    /// checking inside the body).
    fn_sigs: HashMap<Target, FunctionTy>,
    /// `---@class` / `---@struct` names keyed by the statement they
    /// annotate (the carrier `local`). Inference uses this to associate a
    /// locally-constructed table with its declaration.
    declared_targets: HashMap<Target, String>,
    /// The `---@class` tag span keyed by the carrier statement it annotates —
    /// the anchor a `: Interface` conformance error (LB0300) points at
    /// (mirrors [`Self::typed_local_spans`] for the `---@type` path). #107.
    class_tag_spans: HashMap<Target, std::ops::Range<usize>>,
    /// The `---@class` tag span keyed by the class *name*, for classes
    /// declared in *this* file — the secondary "declared here" label a
    /// conformance error attaches to the parent it names (#107). Only
    /// same-file classes are recorded; ambient/defs parents have no in-file
    /// span to point at.
    class_decl_spans: BTreeMap<String, std::ops::Range<usize>>,
    /// The `---@class` tag span of a class declared in ANOTHER project file,
    /// paired with that file's name — merged in from [`FileTypes`] by
    /// [`Self::merge_file_types`] (round 3 review F71). Distinct from
    /// [`Self::class_decl_spans`] (same-file only, keyed by name with no file
    /// component, since it is only ever read against *this* file): a
    /// cross-file consumer's undefined-field diagnostic needs to know
    /// *which* file the class it names lives in before it can point a
    /// secondary label at it at all.
    cross_file_class_decl_spans: BTreeMap<String, (String, std::ops::Range<usize>)>,
    /// Ambient / global values by name: stdlib module tables (`string`,
    /// `math`, ...) and scalar globals (`_VERSION`) declared by definition
    /// packages (`---@meta` `.d.lua`). Populated only for the ambient
    /// layer; per-file annotations shadow these.
    global_types: BTreeMap<String, Ty>,
    /// `---@cast` overrides keyed by the statement they precede.
    casts: HashMap<Target, Vec<CastEntry>>,
    /// Inline `--[[@as T]]` casts keyed by the byte offset of the end of
    /// the expression they follow (the anchor).
    as_casts: HashMap<usize, Ty>,
    /// References to undeclared type names (LB0305): `(name, span)`.
    pub(crate) unknown_names: Vec<(String, luacats::Span)>,
    /// Generic references whose `<...>` argument count is wrong (LB0313, #117).
    pub(crate) arity_errors: Vec<crate::lower::ArityError>,
    /// Cyclic aliases caught by the lowerer's cycle guard (LB0314, #123).
    pub(crate) cyclic_aliases: Vec<crate::lower::CyclicAlias>,
    /// The `---@class` names declared by *this* file's own annotations (not
    /// the ambient/defs layer seeded beneath). The `package`-visibility test
    /// (luals `invisible`, #115): a `package` member is accessible only in the
    /// file that declares its class, so an access is in-package iff the owner
    /// class is one this file declares.
    local_classes: HashSet<String>,
    /// The `---@enum` names declared by *this* file's own annotations — the
    /// enum counterpart of [`Self::local_classes`], and for the same reason
    /// [`TypeEnv::absorb_block`]'s `---@class` arm needs that one: `enums`
    /// is seeded from the ambient layer before any `absorb_block` runs, so
    /// without a record of which entries came from *this* file the
    /// `Tag::Enum` arm cannot tell "the ambient declares this name too"
    /// (shadow it whole) from "this file already declared it" (first-wins,
    /// round 6 review M53). PR #61 finding 1: it could not, and answered the
    /// first question with the second's rule, silently discarding the file's
    /// own declaration in favour of the definition package's.
    ///
    /// Not merged by [`Self::merge_file_types`] and not read by any
    /// diagnostic — purely `absorb_block`'s own bookkeeping for the single
    /// pass that fills it, unlike `local_classes`, which `#115`'s
    /// package-visibility test also reads back.
    local_enums: HashSet<String>,
    /// Root class names whose ancestor-chain resolution tripped
    /// [`DiamondGuard`]'s depth budget specifically — [`LimitTrip::Depth`]
    /// (`LB0317`, round 5 review N2's durable fix; split from the cost
    /// budget's own queue, [`Self::cost_limit_hits`], by production
    /// readiness review F1) — recorded, not merely detected, because
    /// [`Self::class_shape`]/[`Self::class_shape_bound`]/
    /// [`Self::class_shape_bound_export`]/[`Self::class_operators`] all take
    /// `&self` and run repeatedly during inference and checking (unlike
    /// `unknown_names`/`arity_errors`/`cyclic_aliases` above, which the
    /// `Lowerer` — a `&mut self` pass — collects once while
    /// `build_from_items` runs): there is no `&mut self` moment after
    /// construction for a plain field to be written into, so this needs
    /// interior mutability. A `Mutex` rather than a `RefCell`: `TypeEnv`
    /// lives inside `Ambient`, which `defs.rs`'s per-dialect `OnceLock`
    /// cache holds in a `static`, so it must stay `Sync`.
    ///
    /// This is a **one-shot report queue**, not a durable record: drained,
    /// not merely read, by `crate::check::run` — see
    /// [`Self::take_depth_limit_hits`]. A caller that instead needs to know
    /// whether a class's shape is *durably* incomplete — true for the rest
    /// of this env's life, not just until the next drain — reads
    /// [`Self::truncated_classes`] via [`Self::class_ancestry_truncated`]
    /// instead (round 6 review M13: before the two were split, both
    /// consumers shared this one `Mutex`, so a peek that ran after
    /// `take_depth_limit_hits` had already drained it for an earlier report
    /// wrongly read a genuinely still-truncated class as fine).
    depth_limit_hits: Mutex<HashSet<String>>,
    /// Root class names whose ancestor-chain resolution tripped
    /// [`DiamondGuard`]'s cost budget specifically — [`LimitTrip::Cost`]
    /// (`LB0319`, [`crate::codes::CLASS_COST_LIMIT`], production readiness
    /// review F1). The cost-side sibling of [`Self::depth_limit_hits`]:
    /// same one-shot-report-queue shape, same `Mutex` reason, drained by
    /// [`Self::take_cost_limit_hits`] rather than conflated with the
    /// depth-side queue — a walk that tripped the *cost* budget without ever
    /// coming close to [`MAX_ANCESTRY_DEPTH`] needs its own diagnostic
    /// (`LB0319`, "resolution too costly", fixed by removing a conflicting
    /// diamond) rather than depth's ("too deep", fixed by flattening the
    /// hierarchy) — reporting one as the other actively misleads about what
    /// to fix.
    cost_limit_hits: Mutex<HashSet<String>>,
    /// Root class names whose ancestor-chain resolution has **ever** tripped
    /// either of [`DiamondGuard`]'s two budgets on this env — written
    /// alongside [`Self::depth_limit_hits`] or [`Self::cost_limit_hits`] (
    /// whichever [`LimitTrip`] variant a walk actually tripped) by the same
    /// [`Self::note_limit_trip`] call, but never drained, because a
    /// truncated class's shape stays incomplete for the rest of this env's
    /// lifetime and every later peek needs to see that, not just the first
    /// one to ask before anything else drained a report queue (round 6
    /// review M13). Read by [`Self::class_ancestry_truncated`], whose two
    /// callers both need exactly this durable answer: a same-file inference
    /// query deciding whether an absent field is provably undefined (see
    /// that method's own doc comment), and an [`crate::defs::Ambient`]'s
    /// long-lived env, which never runs through `crate::check::run` and so
    /// never drains either report queue at all — this field would grow
    /// monotonically for that caller too if it were one of those `Mutex`es,
    /// but since nothing ever drains it in the first place, splitting it out
    /// changes nothing for that lifecycle beyond no longer being at the
    /// mercy of an unrelated drain. One shared durable record for both trip
    /// kinds, unlike the two separate report queues: a caller asking "is
    /// this class's shape incomplete" does not need to know *why*, only that
    /// it is.
    truncated_classes: Mutex<HashSet<String>>,
    /// Every class name a true `---@class` cycle (`A : A`, or `A : B` / `B :
    /// A`) was found to reach — not merely the query root, but each distinct
    /// name [`DiamondGuard::should_apply`]'s cycle guard ever refused to
    /// recurse into, so a mutual cycle spanning several classes is reported
    /// against all of them, not just whichever one happened to be queried
    /// first (round 6 review M67). Termination alone (what the cycle guard
    /// already gave, before this field existed) leaves the user no
    /// diagnostic that their `: Parent` list does not mean what they wrote.
    /// A one-shot report queue, the same shape and same reason as
    /// [`Self::depth_limit_hits`] (see its doc for why this needs a
    /// `Mutex`): drained, not merely read, via
    /// [`Self::take_cyclic_class_hits`].
    ///
    /// `crate::codes::CYCLIC_CLASS` (`LB0318`) is the code this drains into,
    /// and `crate::check::report_cyclic_class_hits` is the call site that
    /// drains it — both landed alongside this field, at the same point in
    /// `crate::check::run` as [`Self::depth_limit_hits`]'s own
    /// `report_depth_limit_hits` (round 6 review M67). This doc comment
    /// used to claim otherwise — "no consumer drains this yet", "no `LB0xxx`
    /// code exists" — which every finder in a later merge-gate pass flagged
    /// independently (production readiness review F2): true when first
    /// written, false by the time `CYCLIC_CLASS`/`report_cyclic_class_hits`
    /// shipped, and never updated.
    cyclic_class_hits: Mutex<HashSet<String>>,
}

/// The cycle-guard + memo bookkeeping shared by [`TypeEnv::collect_class`]
/// and [`TypeEnv::collect_operators`]'s depth-first ancestor walks (round 4
/// review finding 7 — before this, each function hand-rolled its own
/// `on_path: HashSet<String>` plus `memo: Vec<(String, Vec<Ty>)>` pair with
/// the identical enter/exit logic). One owner now; both walks call
/// [`DiamondGuard::visit`] and neither touches `on_path`/`resolved`/
/// `generation` directly — there is no separate "did you check `Skip`?"
/// step to forget (round 5 review N15): [`DiamondGuard::visit`] itself
/// decides whether the body runs, and records the visit on the way out
/// regardless of whether it did.
///
/// Round 5 shipped a version of this guard that skipped a repeat visit to
/// `(name, args)` unless `name` had *ever*, anywhere in the whole call, been
/// bound to some other `args` (`bindings: HashMap<String, HashSet<Vec<Ty>>>`,
/// checked as "does any entry disagree"). Two bugs followed from that being
/// **unscoped** — a property of the whole call rather than of what actually
/// happened *since* a key was last resolved:
///
/// - **Exponential (round 5 review N1).** Once any name anywhere in the walk
///   was bound two ways, *every* later repeat of it — even one that agrees
///   with every other visit — re-expanded its full subtree, which is
///   exactly the O(2^k) behaviour the memo exists to prevent.
/// - **Wrong winner, both directions (N3/N4/N5).** The check asked "has
///   `name` ever disagreed", not "has anything conflicting happened since
///   *this* key was last resolved" — so a genuinely stale repeat (something
///   else overwrote what it contributed) could be skipped, and an
///   up-to-date one could be needlessly re-expanded, depending on which
///   *other*, unrelated bindings happened to exist elsewhere in the call.
///
/// This version replaces the per-name binding *set* with a single
/// monotonic `generation` counter plus, per key, the generation as of which
/// it was last resolved (`resolved`). `generation` advances exactly once
/// per *genuinely new* distinct binding of an already-seen name (a real
/// diamond disagreement) — never for a name's first-ever binding, and never
/// for a repeat of a binding already on record. A repeat visit is safe to
/// skip exactly when its own recorded generation matches the current one:
/// nothing has disagreed with it *since it was last written*, wherever in
/// the tree that disagreement would have happened. A stale one (recorded
/// generation behind current) re-runs the same body a fresh visit would —
/// which restores its contribution at its own listed position (fixing
/// N3/N4/N5) without re-expanding on every subsequent visit the way the
/// round 5 guard did (fixing N1's exponential).
///
/// This is **not** claimed to be O(k): a re-walk can itself re-descend into
/// further stale repeats, so the cost of a genuinely adversarial shape —
/// every level disagrees with itself, `conflicting_diamond_source` in the
/// tests below — is polynomial, not linear, and measurably worse than the
/// no-conflict case (`diamond_source`) which this guard *does* keep at
/// O(k). Measured, release build, `diamond_perf_sweep`:
///
/// ```text
/// k     conflicting_diamond_source   diamond_source (no conflict)
/// 20    1.41ms                       0.21ms
/// 40    8.89ms                       0.42ms
/// 60    28.05ms                      0.60ms
/// 80    60.96ms                      0.83ms
/// 100   116.31ms                     1.06ms
/// ```
///
/// The conflicting column's growth over the two doublings this table
/// covers — k=20→40 (8.89/1.41 ≈ 6.3x) and k=40→80 (60.96/8.89 ≈ 6.9x) — is
/// bounded and nowhere near the 2.0-2.4x **per level** (not per doubling)
/// round 5's unscoped guard measured on the same adversarial shape — the fix
/// this replaces took 32s at k=20; this one takes 1.4ms — but it is not the
/// flat O(k) the no-conflict shape gets: a ~6.3-6.9x growth per doubling
/// works out to roughly O(k^2.7) over this range (round 6 review M15: an
/// earlier revision of this comment wrote "≈2.8x per doubling", which is the
/// *exponent*, not the ratio a doubling of k actually produces — the true
/// per-doubling ratio these same four measurements imply is the ~6.3-6.9x
/// above). No comment in this file claims otherwise beyond what these
/// numbers show (round 5 review N13/N14: every complexity claim here is one
/// this test measured, not one reasoned about and left unverified).
///
/// `should_apply` is [`DiamondGuard::visit`]'s decision half, called from inside it
/// (round 6 review M16: an earlier revision of this comment had the
/// direction backwards, describing `should_apply` as "returned by `visit`"
/// — `visit` itself returns `()`; the `bool` `should_apply` produces is
/// passed *into* `body`, the closure `visit`'s caller supplies, not handed
/// back out to that caller). It answers a second, narrower question some
/// callers need: is this particular visit a name's first-ever binding, or
/// does it follow an earlier, different one? [`TypeEnv::collect_class`]'s
/// indexer merge is the one consumer — see its doc comment for why fields
/// and indexers need different answers to "which sibling wins" even though
/// both are merged by the same walk (round 5 review N6/N7).
/// Ancestor-chain depth [`DiamondGuard`] refuses to recurse past (round 5
/// review N2's durable fix — `LB0317`, [`crate::codes::CLASS_DEPTH_LIMIT`]).
///
/// Every live `on_path` entry costs one native `TypeEnv::collect_class`/
/// `collect_operators` stack frame; a single-parent `Cn : C(n-1)` chain
/// walks one frame per ancestor with nothing to stop it short of the calling
/// thread's own stack, and past whatever that thread survives the whole
/// PROCESS aborts — `SIGABRT`, uncatchable, no diagnostic, no file name
/// (exactly the failure round 5 review N2 reported: `class_shape`/
/// `class_shape_bound`/`class_operators` all take `&self`, called from
/// every entry point that resolves a class — the CLI, the LSP request path,
/// and an embedder calling `luabox_lsp::run_stdio` directly, whose thread
/// carries no explicit stack pin at all).
///
/// Measured fresh for this constant (debug build, this walk's *current*
/// frame size — `env::tests::temp_probe_unpinned_debug_floor`, bisected by
/// hand and not left in the tree): a chain forcing resolution of its
/// deepest class, `cargo test`'s own default thread (no `.stack_size()` —
/// Rust's un-pinned default, ~2 MiB, exactly the budget an embedder calling
/// `luabox_lsp::run_stdio` directly gets, since that thread carries no
/// explicit stack pin at all) survives to **~990-999** before it aborts;
/// pinned to 16 MiB (`luabox-cli::main::PINNED_STACK_BYTES`, mirrored by
/// `luabox-lsp::server::PINNED_STACK_BYTES` — what the CLI/LSP's own
/// dispatcher threads actually run with) it survives to **~7,900-7,999** —
/// an ~8x ratio matching the ~8x more stack, confirming this walk's frame
/// size scales as expected between the two.
///
/// The un-pinned figure, not the pinned one, is the binding constraint: the
/// embedder thread the finding names as most exposed gets no pin at all, and
/// `cargo test` itself exercises that same un-pinned default on every test
/// that does not explicitly spawn a bigger stack (most of this module's
/// deep-chain tests do spawn one; this guard must hold even for the ones
/// that do not, and for every caller outside this crate's own test suite).
/// 200 sits about 5x below the ~990 un-pinned floor while staying trivially
/// clear of the pinned/release floors above it. No dialect or SPEC.md
/// convention describes a legitimate hierarchy within an order of
/// magnitude of this depth, so the only inputs
/// this can refuse are already pathological — see
/// `env::tests::a_class_chain_at_the_ancestry_limit_resolves_fully_and_correctly`
/// for the floor side of that claim, and
/// `env::tests::a_class_chain_past_the_ancestry_limit_does_not_crash` for the
/// ceiling side.
///
/// `luabox-cli::check_cmd`'s syntactic pre-check
/// (`class_ancestry_precheck`) imports this exact constant rather than
/// deriving its own ceiling: it used to guard 2,000 links deep, a number
/// picked for its *own* (pinned-thread) crash floor with room to spare — but
/// that number was never a promise this walk could keep. A chain past 200
/// still hits this cap and truncates inside `class_shape`/`class_shape_bound`/
/// `class_operators` regardless of what the pre-check said was safe, so a
/// pre-check ceiling looser than this one only misinforms the user about
/// where resolution actually gives out (production readiness review,
/// finding 1: a 400-class chain the pre-check certified "fine" produced a
/// false `LB0306` on a field the truncated walk simply never reached). One
/// number, enforced twice — once early and cheaply as `LB0317` on the raw
/// annotations (catching a declared-but-never-resolved chain the walk below
/// would otherwise never visit — round 6 review M4(b): this pre-check used
/// to hard-code `LB0001`, "syntax error", for this condition; it now emits
/// the same `LB0317` this durable check below does), once durably here for
/// every caller that reaches a `TypeEnv` directly, CLI pre-check or not.
pub const MAX_ANCESTRY_DEPTH: usize = 200;

/// A ceiling on the total number of *re*-resolutions one [`DiamondGuard`]-
/// supervised walk may run `body` for — the companion budget to
/// [`MAX_ANCESTRY_DEPTH`] (round 6 review M3; re-scoped to re-resolutions
/// only by production readiness review F1 — see [`DiamondGuard::resolutions`]'s
/// own doc comment for exactly which arm of `should_apply` counts and why).
/// `MAX_ANCESTRY_DEPTH` bounds how deep any ONE recursion path can go; it
/// says nothing about how many times a walk re-resolves a *stale repeat*,
/// which is exactly what a genuinely conflicting diamond forces (round 5's
/// fix note above measures this as polynomial, not O(k)). And
/// `class_shape`/`class_operators`/etc. run once **per class** a project
/// file declares — `luabox check` on one file with G independently-
/// conflicting diamond groups pays this polynomial cost G times over, once
/// per group's own root query, with nothing capping the sum.
///
/// **F1: counting every surviving `should_apply` arm — first bindings
/// included — was the wrong quantity, not merely the wrong number.** A
/// first-ever binding of a name, and a genuinely new distinct binding of an
/// already-seen name, are each visited at most once per distinct `(name,
/// args)` pair the ancestor graph actually contains — bounded by what the
/// source declares, not by how many times a conflict forces a re-walk. A
/// wide, shallow, **non-conflicting** hierarchy (120 independent 5-deep
/// mixin chains, one class inheriting all 120 — 601 classes, max recursion
/// depth 6, zero diamond conflicts anywhere) visits every one of those 601
/// names exactly once: 601 first-bindings, zero re-resolutions. Counting
/// first-bindings put that single, entirely legitimate query over any
/// budget in the few-hundred range — measured this session, this exact
/// fixture through `luabox check`, release build:
///
/// ```text
/// binary                          verdict    time
/// merge base (`develop`)          clean      0.017-0.053s
/// this constant counting all arms error[LB0317], exit 1   —
/// this constant, re-resolutions only (F1 fix) clean      0.023-0.034s
/// ```
///
/// Re-scoping the counter to only the stale-generation re-resolution arm —
/// the walk a conflicting diamond actually forces, not the linear discovery
/// pass every hierarchy pays once regardless of conflicts — fixes the false
/// rejection by construction: the wide fixture's `DiamondGuard::resolutions`
/// is 0 under the new counting, for any budget above 0.
///
/// The constant itself is re-derived from a direct measurement of
/// `DiamondGuard::resolutions` under the new counting (not chosen by
/// symmetry with `MAX_ANCESTRY_DEPTH`, and not carried over unexamined from
/// the old counting's value): a release-build sweep of `collect_class`
/// alone (no CLI, no parse/harvest overhead — gathered with a throwaway
/// instrument, not left in the tree), uncapped, over
/// `conflicting_diamond_source(k)` — every level genuinely disagreeing —
/// gives
///
/// ```text
/// k     resolutions   collect_class alone
/// 20    190           ~150µs
/// 40    780           ~552µs
/// 60    1,770         ~1.37ms
/// 80    3,160         ~2.12ms
/// 100   4,950         ~3.18ms
/// ```
///
/// Exactly `k(k-1)/2` at every sampled point — the re-resolution count for
/// this fixture is a clean triangular number, not the ~O(k^2.7) the OLD
/// counting measured on the same fixture (that earlier growth rate mixed in
/// the O(k) linear first-visit pass at every level, which is why it did not
/// reduce to a closed form). A project file resolves one `DiamondGuard`
/// walk **per declared class**, so `luabox check`'s real per-group cost is
/// the *sum* of k walks of depth 1, 2, ..., k, each individually capped by
/// this budget once its own re-resolution count crosses it.
///
/// Every figure in this doc comment was measured on the fixture shape
/// `conflicting_diamond_source` had when they were taken —
/// `A{i}<T> : A{i-1}<T>, A{i-1}<number>`, both edges naming one ancestor.
/// PR #61 finding 6 made a single declaration's parent list dedupe by name
/// like every other duplicate-declaration axis, which collapses those two
/// edges into one, so the fixture now writes the disagreeing edge to the
/// *grandparent* instead. Re-measured on the new shape, `resolutions` is
/// exactly `(k-1)(k-2)/2` (6 at k=5, 36 at k=10, 171 at k=20) — the same
/// closed form one level lower, so every row below still describes the same
/// work, reached at `k+1` rather than `k`. Nothing here was re-derived by
/// assumption; the new closed form is a direct measurement.
///
/// 200 was picked by working back from CLI wall-clock time on the single
/// most adversarial shape this budget exists for —
/// `conflicting_diamond_group` at k=190 (every level conflicting, the
/// deepest a group can go before nearing [`MAX_ANCESTRY_DEPTH`] itself) —
/// measured this session, release `luabox check`:
///
/// ```text
/// fixture                              develop   uncapped (new counting)   budget=200 (new counting)
/// 1 group  (197 lines,  191 classes)   0.011-0.059s   >90s (times out)         0.14s
/// 10 groups (1970 lines, 1910 classes) 0.060-0.197s   not attempted             1.44-1.48s
/// ```
///
/// The uncapped row confirms the pathology this budget exists for is still
/// very real under the new counting (a single k=190 conflicting group alone
/// does not finish in 90s) — re-scoping the counter to re-resolutions only
/// fixes F1's false positive, it does not remove the need for a cap. At
/// budget=200, both rows land under 1.5s, comfortably faster on both
/// fixtures than the *previous* (F1-buggy) `budget=500` figures this doc
/// used to cite (0.21s / 2.13s) despite the new counting letting every
/// individual class's walk run substantially more real, legitimate first-
/// visit work before capping. `k(k-1)/2 ≤ 200` holds through k=20 (`20·19/2
/// = 190`; on the current fixture shape the same bound holds through k=21,
/// `20·19/2` again), so any hierarchy with up to 20 full levels of genuine,
/// cascading generic-diamond disagreement resolves completely, uncapped —
/// [`MAX_ANCESTRY_DEPTH`]'s own doc argues no real hierarchy comes within an
/// order of magnitude of 200 *levels*; the same argument applies more
/// strongly to 20 full levels of actual disagreement at every one of them,
/// which is a far rarer shape than mere depth. See
/// `env::tests::a_k_level_diamond_with_conflicting_bindings_grows_no_worse_than_linearly_with_group_count`
/// for the regression test this measurement backs, and
/// `env::tests::a_wide_shallow_non_conflicting_hierarchy_resolves_cleanly_and_cheaply`
/// for F1's own regression test.
pub const MAX_ANCESTRY_RESOLUTIONS: u64 = 200;

/// Which of [`DiamondGuard`]'s two budgets a walk tripped first —
/// production-readiness review F1. Before this split, both budgets set the same
/// `depth_exceeded: Option<String>` field, so a walk that tripped only the
/// *cost* budget ([`MAX_ANCESTRY_RESOLUTIONS`]) was reported identically to
/// one that tripped the *depth* budget ([`MAX_ANCESTRY_DEPTH`]) — `LB0317`,
/// "too deep", with a "flatten the hierarchy" remedy that does not fit a
/// wide-but-shallow hierarchy whose actual problem is a conflicting diamond.
/// [`TypeEnv::note_limit_trip`] is the sole production reader: it matches on
/// the variant to decide which report queue to file `root` under
/// (`LB0317`/[`crate::codes::CLASS_DEPTH_LIMIT`] or
/// `LB0319`/[`crate::codes::CLASS_COST_LIMIT`]).
///
/// **Fieldless.** Each variant used to carry the ancestor name whose visit
/// was refused, justified as "for tests that need to assert exactly who a
/// trip is attributed to" — but production never read either payload
/// ([`TypeEnv::note_limit_trip`], the sole production reader, discards it
/// via `_` and files the top-level *query root* instead, which is a
/// different name), so the payload was state that could only ever go stale
/// or disagree with what is reported. PR #61 finding 7 removed it; the two
/// tests that read it now assert attribution the way production does, off
/// `depth_limit_hits`/`cost_limit_hits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LimitTrip {
    /// [`MAX_ANCESTRY_DEPTH`] tripped: recursing further would add a native
    /// stack frame past the crash floor that constant is set to stay clear
    /// of.
    Depth,
    /// [`MAX_ANCESTRY_RESOLUTIONS`] tripped: total re-resolution work for
    /// this walk exceeded the budget, regardless of how deep any one path
    /// went.
    Cost,
}

#[derive(Default)]
struct DiamondGuard {
    on_path: HashSet<String>,
    /// Every `(name, args)` this call has resolved, and the `generation` as
    /// of which its contribution is known current. Doubles as the old
    /// `memo` (a key present at all `HashSet<(String, Vec<Ty>)>::contains`)
    /// and the old `bindings` (`resolved.get(name)` non-empty, with more
    /// than one key, tells you `name` has disagreeing bindings) — round 5
    /// review N11: those were two collections holding one derivable
    /// relationship (`memo.contains((n, a)) <=> bindings[n].contains(a)`
    /// at every program point, since both were only ever written together
    /// in `exit`), and nothing enforced that they stayed in sync.
    resolved: HashMap<String, HashMap<Vec<Ty>, u64>>,
    /// Bumped once per genuinely new distinct binding of an already-seen
    /// name — see the type doc comment.
    generation: u64,
    /// Total number of times `body` has run to **re**-resolve a key already
    /// on record (`should_apply`'s `Some(_) => ...` stale-generation arm),
    /// incremented at the exact point that arm decides to return
    /// `Some(..)` — production-readiness review F1. Deliberately **not**
    /// incremented for a name's first-ever binding, nor for a genuinely new
    /// distinct binding of an already-seen name (`should_apply`'s
    /// `None => ...` arm): both of those are bounded by what the source
    /// itself declares — one `should_apply` call per distinct `(name,
    /// args)` pair the ancestor graph actually contains — and are not the
    /// pathology this budget exists to bound. The pathology is *re*-visiting
    /// a key that was already resolved once, because some *other* binding
    /// disagreed with it since — the generation-stamped re-walk a
    /// conflicting diamond forces, which can revisit the same key
    /// arbitrarily many times as further disagreements keep invalidating it.
    /// A version of this counter that incremented on every surviving arm
    /// (first bindings included) rejected legitimate wide-but-shallow
    /// hierarchies with no conflict anywhere in them — see
    /// [`MAX_ANCESTRY_RESOLUTIONS`]'s own doc comment for the measurement
    /// that caught it.
    resolutions: u64,
    /// Set the first time a visit would exceed [`MAX_ANCESTRY_DEPTH`] or
    /// [`MAX_ANCESTRY_RESOLUTIONS`] — which budget, and the ancestor name
    /// whose visit was refused. Sticky: `should_apply` refuses every further
    /// attempt on this guard too, once set, so a pathological chain does not
    /// resolve partway down some branches and stop short on others depending
    /// on visit order. Read back by [`TypeEnv::class_shape_bound`] and its
    /// siblings, right after their own top-level `collect_class`/
    /// `collect_operators` call returns, via [`TypeEnv::note_limit_trip`],
    /// to record `LB0317` or `LB0319` (round 5 review N2's durable fix;
    /// split into the two variants by production-readiness review F1 — see
    /// [`LimitTrip`]'s own doc comment for why one shared `Option<String>`
    /// was wrong).
    limit_exceeded: Option<LimitTrip>,
    /// Ancestor names `should_apply` refused because they were already on
    /// the current recursion path — a true `---@class` cycle (`A : A`, or
    /// `A : B` / `B : A`), not a diamond (round 6 review M67). The guard
    /// against this back-edge keeps the walk from looping forever, but
    /// terminating silently gives the user no signal that their `: Parent`
    /// list does not mean what they wrote — a cyclic class's own field
    /// reads back as reachable, with nothing to say the chain never
    /// actually reached anywhere else. Not sticky like `limit_exceeded`
    /// (a cycle on one branch does not invalidate an unrelated sibling
    /// branch the same visit resolves cleanly): every name this walk ever
    /// refused this way is collected, not just the first, so
    /// [`TypeEnv::note_cyclic`] can report every class the cycle actually
    /// reaches, not merely that one was found somewhere. `should_apply`
    /// only allocates the `String` this inserts the first time a given name
    /// is refused this way — a borrowed `contains` check first (production
    /// readiness review F3) keeps every later repeat of the same back-edge,
    /// which a per-keystroke walk over an unresolved cycle hits often,
    /// allocation-free.
    cyclic: HashSet<String>,
}

impl DiamondGuard {
    /// Visit `(name, args)`, running `body` only if this key needs
    /// (re-)processing right now, and recording the visit on the way out
    /// regardless. This is the only way into a walk this guard oversees:
    /// there is no `Skip` a caller must remember to check and no `exit` a
    /// caller must remember to pair with an `enter` — `body` either runs
    /// exactly once, with its result folded in and the visit recorded, or
    /// it does not run at all and nothing is recorded (round 5 review N15).
    ///
    /// `body`'s second argument is whether this is `name`'s first-ever
    /// binding in this call (`true`) or a binding that follows a *different*
    /// one (`false`) — see the type doc comment and
    /// [`TypeEnv::collect_class`]'s indexer merge, the one place that
    /// distinction changes behaviour.
    fn visit(&mut self, name: &str, args: &[Ty], body: impl FnOnce(&mut Self, bool)) {
        let Some(is_first_binding) = self.should_apply(name, args) else {
            return;
        };
        body(self, is_first_binding);
        // Reuse the existing `resolved` entry for `name` (and, inside it,
        // for `args`) whenever either is already on record, rather than
        // reallocating an owned `String`/`Vec<Ty>` key only to immediately
        // discard it in favour of the one already there (round 6 review
        // M14): a repeat visit — the common case on any diamond shape,
        // since every ancestor reached through more than one path is
        // visited again on every subsequent reference to it — already has
        // both as existing map keys; only a name's or a binding's genuine
        // first appearance needs a fresh allocation here.
        let bindings = match self.resolved.get_mut(name) {
            Some(bindings) => bindings,
            None => self.resolved.entry(name.to_string()).or_default(),
        };
        match bindings.get_mut(args) {
            Some(slot) => *slot = self.generation,
            None => {
                bindings.insert(args.to_vec(), self.generation);
            }
        }
        self.on_path.remove(name);
    }

    /// The decision half of [`Self::visit`]: `None` means a true cycle or an
    /// up-to-date repeat — the caller does nothing further, `on_path` is
    /// already rolled back, and nothing is recorded. `Some(is_first_binding)`
    /// means the caller must run its body and this guard is now waiting for
    /// the bookkeeping `visit` performs on return.
    fn should_apply(&mut self, name: &str, args: &[Ty]) -> Option<bool> {
        // Ordered ahead of the sticky `limit_exceeded` return below, not
        // after it (PR #61 finding 5). Both refuse the visit, so the
        // walk-stopping semantics are identical either way — but only this
        // one *records* anything, and a back-edge onto a name still on the
        // current recursion path is a true `---@class` cycle no matter what
        // some already-exhausted sibling branch did. Behind the limit check
        // the record was simply lost, with nothing to recover it: resolution
        // here is reference-driven, so there is no per-class query loop that
        // would revisit the cycle later. Measured at d20cd47 on
        // `---@class Cyc : <over-budget group>, Cyc` — `LB0319` recorded,
        // `LB0318` silently absent.
        if self.on_path.contains(name) {
            // True cycle (round 6 review M67). Borrowed lookup first
            // (production readiness review F3): a per-keystroke walk over an
            // unresolved cycle hits this same back-edge repeatedly, and only
            // the first refusal of a given `name` is new information —
            // `cyclic.insert` allocates a fresh `String` only then, not on
            // every later repeat of a name already on record.
            if !self.cyclic.contains(name) {
                self.cyclic.insert(name.to_string());
            }
            return None;
        }
        if self.limit_exceeded.is_some() {
            return None; // already tripped — refuse everything else too
        }
        if self.on_path.len() >= MAX_ANCESTRY_DEPTH {
            // A genuinely new level, not a cycle back-edge (checked above):
            // recursing into `collect_class`/`collect_operators` for `name`
            // would add another native stack frame past the crash floor
            // `MAX_ANCESTRY_DEPTH` is set to stay clear of (see its doc
            // comment). Refuse here, before that frame exists, rather than
            // let the process find its own limit.
            self.limit_exceeded = Some(LimitTrip::Depth);
            return None;
        }
        // Borrowed lookups only, so the common up-to-date-repeat case (any
        // diamond shape re-checks most ancestors far more often than it
        // re-resolves them) costs nothing beyond the lookup itself: `on_path`
        // is touched only once we already know `body` will run (round 6
        // review M14 — this used to insert `name.to_string()` into `on_path`
        // unconditionally, before it could even ask this question, then
        // immediately remove it again on exactly this bail path).
        //
        // `is_stale_reresolution` is `true` only for the middle arm below —
        // a key already resolved once, now invalidated by some *other*
        // binding disagreeing with it since. That, and only that, is the
        // total-work budget's business (production readiness review F1):
        // a name's first-ever binding and a genuinely new distinct binding
        // of an already-seen name (the other two arms) are each visited at
        // most once per distinct `(name, args)` pair the ancestor graph
        // actually contains, bounded by the source itself, not by how many
        // times a diamond conflict forces a re-walk. See `resolutions`'s own
        // doc comment for why counting either of those as "resolutions"
        // rejected legitimate wide-but-shallow hierarchies with no conflict
        // anywhere in them.
        let (is_first_binding, is_stale_reresolution) = match self.resolved.get(name) {
            None => (true, false),
            Some(bindings) => match bindings.get(args) {
                Some(&resolved_at) if resolved_at == self.generation => {
                    return None; // up to date; nothing has disagreed since
                }
                // Stale: something else disagreed with `name`'s binding
                // since this exact key was last resolved. Fall through and
                // re-resolve it now, restoring it to the current generation —
                // this is THE re-resolution the total-work budget counts.
                Some(_) => (false, true),
                // A genuinely new distinct binding of an already-seen name:
                // a real diamond disagreement, discovered for the first
                // time. Advance the generation so every other key resolved
                // before this point is now stale and will re-assert itself
                // the next (and only the next) time it is visited — but this
                // visit itself is not a re-resolution of anything; nothing
                // was invalidated to produce it.
                None => {
                    self.generation += 1;
                    (false, false)
                }
            },
        };
        // The total-work budget (round 6 review M3, re-scoped to
        // re-resolutions only by production readiness review F1): only a
        // stale re-resolution counts as the genuine, potentially-unbounded
        // work this budget exists to cap. Checked, not merely counted: once
        // a walk has re-resolved `MAX_ANCESTRY_RESOLUTIONS` stale keys, this
        // attempt — the one that would push it over — is refused via the
        // same sticky `limit_exceeded` flag and truncation path
        // `MAX_ANCESTRY_DEPTH` uses, but filed as `LimitTrip::Cost` rather
        // than `LimitTrip::Depth` so the two are reported as the distinct
        // diagnostics they are (`LB0319` vs `LB0317` — see [`LimitTrip`]'s
        // doc comment).
        if is_stale_reresolution {
            self.resolutions += 1;
            if self.resolutions > MAX_ANCESTRY_RESOLUTIONS {
                self.limit_exceeded = Some(LimitTrip::Cost);
                return None;
            }
        }
        self.on_path.insert(name.to_string());
        Some(is_first_binding)
    }
}

// --- member merge precedence ------------------------------------------
//
// Two `---@class` declarations for one name are merged by three separate
// seams — `TypeEnv::absorb_block` (same file), `TypeEnv::merge_file_types`
// (cross file), `TypeEnv::collect_class` (the consume-site ancestor fold) —
// and each decides, independently, per member kind, who wins when two
// declarations disagree. The functions below are the single owner of that
// decision for each kind: every seam that needs to decide calls one of
// these rather than re-deriving the rule inline. See
// `docs/03-reference/03-class-merge-precedence.md` for the full measured
// matrix these functions encode.

/// The two contexts a member-precedence decision gets made in.
///
/// - `Duplicate` — a second `---@class` declaration for a name already
///   declared: `absorb_block`'s same-file case or `merge_file_types`'s
///   cross-file case. Both read a *duplicate* of the user's own intent, so
///   both resolve the identical way: the first declaration wins.
/// - `AncestorFold` — `collect_class`'s consume-site walk, folding two
///   *different* classes (unrelated parents, or the same generic ancestor
///   reached twice with different bindings) into one shape. `is_first_binding`
///   is [`DiamondGuard::visit`]'s distinction between a name's first-ever
///   binding in the walk (`true`, a genuinely unrelated sibling parent) and a
///   competing repeat of it (`false`, the same ancestor reached again with a
///   different binding — a diamond conflict) — the distinction both
///   `member_wins` and [`indexer_resolution`] read to give the two shapes
///   their own (opposite) winners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberArrival {
    Duplicate,
    AncestorFold { is_first_binding: bool },
}

/// The single "does an incoming value replace what's already on record"
/// rule for every member kind whose precedence is a plain overwrite-or-keep
/// decision — `---@field`s, carrier methods, and (for `Duplicate` only)
/// visibility scopes; visibility's own `AncestorFold` shape is decided
/// separately, by `walk_ancestor_names` (see its doc comment). Value-agnostic
/// (`V` is never inspected, only whether it is present), which is what lets
/// one function serve all these: matrix rows `field-B`/`field-C` (duplicate:
/// first wins), `field-D` (ancestor fold, first binding: first-**listed**
/// parent wins — luals parity, production-readiness-assessment-9natxz A1),
/// `field-F` (ancestor fold, repeat binding: last-**visited** edge wins), and
/// their method/visibility counterparts.
fn member_wins<V>(existing: Option<&V>, arrival: MemberArrival) -> bool {
    match (existing, arrival) {
        (None, _) => true,
        (Some(_), MemberArrival::Duplicate) => false,
        // Two genuinely *different* classes each declaring their own value
        // for this key (unrelated parents, `is_first_binding: true`) is
        // first-listed-wins — luals parity (compiler.lua:369-375/424,
        // production-readiness-assessment-9natxz A1): the first sibling
        // visited claims the key and every later one is blocked, exactly
        // like `indexer_resolution`'s `Keep` arm for the identical case. The
        // *same* ancestor reached again with a genuinely different binding
        // (a diamond conflict, `is_first_binding: false`) is unaffected by
        // this fix and stays last-visited-edge-wins, matching indexer's
        // `Overwrite` arm — that shape has no luals analogue to disagree
        // with (luals 3.13.5 has no generic `---@class` support at all) and
        // is not part of this correction.
        (Some(_), MemberArrival::AncestorFold { is_first_binding }) => !is_first_binding,
    }
}

/// What a seam merging a `---@field [K] V` indexer for `key` should do with
/// the slot: append a fresh one, replace what is there, or leave it alone.
enum IndexerResolution {
    Insert,
    Overwrite,
    Keep,
}

/// `---@field [K] V` indexer precedence — the single rule for every seam
/// that must decide two indexer values for the same key
/// (`docs/03-reference/03-class-merge-precedence.md`'s indexer table).
/// [`member_wins`] answers the identical `is_first_binding` question now
/// (production-readiness-assessment-9natxz A1 brought fields onto the same
/// rule this function already had), but a map's `insert` can't express
/// "leave the existing slot alone" the way a `Vec` of `(key, value)` pairs
/// needs to — hence the three-way `IndexerResolution` here instead of
/// `member_wins`'s bool.
///
/// - `Duplicate` (same-file `absorb_block` / cross-file `merge_file_types`):
///   the **first** declaration wins — an existing slot is always kept
///   (`indexer-B`/`indexer-C`).
/// - `AncestorFold` with `is_first_binding: true` — two genuinely
///   *different* classes each declaring their own indexer for the same key
///   (unrelated parents): the **first**-listed one wins, so a later
///   sibling's value is dropped (`indexer-D`) — matching fields.
/// - `AncestorFold` with `is_first_binding: false` — the *same* ancestor
///   reached twice with different bindings (a diamond): the **last**-visited
///   edge wins, exactly like fields (`indexer-F`).
fn indexer_resolution(exists: bool, arrival: MemberArrival) -> IndexerResolution {
    // Derived from `member_wins` rather than hand-maintained separately
    // (round 6 review M10): a map's `insert` can express "overwrite" or
    // "insert fresh" but not "leave the existing slot alone", which is why
    // this needs its own three-way `IndexerResolution` — but *which* of
    // those three applies is exactly the same "does an incoming value
    // replace what's on record" question `member_wins` already answers, so
    // this asks it that question rather than re-deriving the rule inline.
    // The `V` parameter is never inspected (see `member_wins`'s own doc
    // comment) so a unit reference stands in for the indexer slot's actual
    // value.
    if !exists {
        IndexerResolution::Insert
    } else if member_wins(Some(&()), arrival) {
        IndexerResolution::Overwrite
    } else {
        IndexerResolution::Keep
    }
}

/// The `HashMap<Ty, usize>` side of the indexer-dedupe pair: which key sits
/// at which position in a `Vec<(Ty, Ty)>` of `---@field [K] V` entries.
///
/// The single owner of that pairing (PR #61 round-8 F20). Three seams merge
/// indexers — the same-file/cross-file duplicate fold
/// ([`TypeEnv::merge_file_types`]), the per-tag fold
/// ([`TypeEnv::absorb_block`]) and the ancestor fold
/// ([`TypeEnv::collect_class`]) — and each carried its own hand-copied
/// `.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect()` plus its
/// own `contains_key` / `insert(len())` / `push` sequence. Three copies of one
/// invariant is three chances for the index and the vector to disagree about
/// which `---@field [K] V` counts as a duplicate, and any change to indexer-key
/// canonicalisation had to be made three times. The invariant now lives here:
/// nothing outside this type decides what an entry's position is, and
/// `absorb` is the only way an entry reaches the vector at all.
///
/// The entry list is passed in per call rather than owned, because the seams
/// hold it differently — one has it behind a `&mut` field re-borrowed per tag
/// and keeps this index alive across those borrows, one owns it outright.
/// `index_of` is what makes that safe to state: an [`IndexerIndex`] is only
/// ever valid for the entry list it was built from.
#[derive(Default)]
struct IndexerIndex {
    index_of: HashMap<Ty, usize>,
}

impl IndexerIndex {
    /// The index for an entry list that already has entries in it — the
    /// O(K) build the O(K²) `.iter().any()` re-scan replaced (round 6 review
    /// M55).
    fn of(entries: &[(Ty, Ty)]) -> Self {
        IndexerIndex {
            index_of: entries
                .iter()
                .enumerate()
                .map(|(i, (key, _))| (key.clone(), i))
                .collect(),
        }
    }

    /// Merge one arriving `[key] value` into `entries` under
    /// [`indexer_resolution`]'s precedence for `arrival`, keeping the index in
    /// step whichever way it resolves.
    fn absorb(&mut self, entries: &mut Vec<(Ty, Ty)>, key: Ty, value: Ty, arrival: MemberArrival) {
        match indexer_resolution(self.index_of.contains_key(&key), arrival) {
            IndexerResolution::Insert => {
                self.index_of.insert(key.clone(), entries.len());
                entries.push((key, value));
            }
            IndexerResolution::Overwrite => {
                if let Some(&position) = self.index_of.get(&key) {
                    entries[position].1 = value;
                }
            }
            IndexerResolution::Keep => {} // first-listed for this key already stands
        }
    }
}

/// `---@operator` accumulation — the one kind where nothing picks a winner
/// between two conflicting values at merge time: every declared overload
/// joins one scan list, and *resolution* (inference, #114) tries each in
/// declaration order until one accepts the operand, so first-listed-wins
/// (`operator-D`) falls out of **accumulation order** rather than an
/// overwrite decision here. `operator-F`'s diamond-conflicting shape is the
/// one exception with an overwrite-shaped decision — see
/// [`TypeEnv::collect_operators`]'s doc comment for why *that* seam does
/// need one, unlike the two duplicate-declaration seams below.
///
/// Both duplicate-declaration seams calling this function dedupe a
/// value-identical repeat: `merge_file_types` folds an already-complete
/// class def and must not grow the list from folding the same source twice
/// (`operator-C`, pinned by
/// `merging_repeated_declarations_never_duplicates_parents_or_operators`);
/// `absorb_block` reads this file's own doc blocks one at a time and used to
/// skip the dedupe check entirely, on the reasoning that a value-identical
/// `---@operator` repeat is not a state two tags on one class in one file
/// ever produce in the corpus this was measured against (`operator-B`'s two
/// overloads always differ on `result`). That reasoning covered the
/// *measured* corpus, not the general case — a literal copy-paste of one
/// `---@operator` tag onto a second `---@class` block for the same name is
/// exactly as constructible in one file as across two, and there is no
/// reason a same-file duplicate should keep a byte-identical entry twice
/// when a cross-file one collapses it to one (the same "the file boundary
/// does not change the merge" principle
/// `docs/03-reference/02-limitations.md`'s "Duplicate `---@class`
/// declarations union" section already states for every other member kind).
/// Both seams now dedupe identically — see
/// `absorb_block_dedupes_a_byte_identical_operator_repeat_the_same_as_merge_file_types`
/// for the pin. A repeat that differs on `input`/`result` (`operator-B`'s
/// actual fixture) is never value-identical, so this change does not touch
/// it: `sigs.contains` compares the whole signature, and two overloads that
/// disagree on anything still both survive, exactly as before.
///
/// `collect_operators` (the ancestor-fold seam for this kind) does not call
/// this function: unconditional list-building plus a narrower,
/// owner-tracked supersede rule for diamond conflicts is a different shape
/// than "keep or drop one incoming value" — see its own doc comment.
fn push_operator_overload(sigs: &mut Vec<OperatorSig>, sig: OperatorSig) {
    if sigs.contains(&sig) {
        return;
    }
    sigs.push(sig);
}

/// A class's own `<T, U, ...>` type-parameter list: a re-declaration that
/// names none has none to contribute (first non-empty list is canonical,
/// `typeparam-B-absorb-block-second-empty-same-file`); shared by
/// `absorb_block`'s same-file case and `merge_file_types`'s cross-file case.
/// No `AncestorFold` counterpart exists for this kind — a class's own
/// parameter list has no analogue across parents for `collect_class` to
/// arbitrate (matrix: type-parameter table, D/E/F columns N/A).
fn adopt_params_if_unset(existing: &mut Vec<String>, incoming: &[String]) {
    if existing.is_empty() {
        existing.clear();
        existing.extend_from_slice(incoming);
    }
}

/// Append `parent` to a class's own `parents` list unless an ancestor of the
/// same name is already on it — parents dedupe by NAME, so two declarations
/// naming one parent (with possibly different type arguments) are the same
/// edge in the chain and the **first** wins, matching every other
/// duplicate-declaration axis in this file (round 6 review M9: this body used
/// to be hand-copied, identically, into `absorb_block`'s same-file
/// re-declaration case and `merge_file_types`'s cross-file case — the one
/// member kind with no single owner in the table the rest of this section is
/// named after).
///
/// The rule is *per class*, not per declaration (PR #61 finding 6): a single
/// `---@class C : Base<number>, Base<string>` names one edge twice exactly as
/// two declarations do, so `absorb_block` routes an individual declaration's
/// own parent list through here too. It used to push those raw, which kept
/// both edges and let the last-visited one win — the opposite answer this
/// function documents, reached silently, from the same two edges written on
/// one line instead of two.
fn push_parent_ref(parents: &mut Vec<ParentRef>, parent: ParentRef) {
    if !parents.iter().any(|p| p.name == parent.name) {
        parents.push(parent);
    }
}

impl TypeEnv {
    /// Build the environment for one parsed file.
    #[must_use]
    pub fn build(parse: &lua::Parse) -> TypeEnv {
        let items = luacats::harvest(parse);
        Self::build_from_items(parse, &items, None)
    }

    /// Build the environment from pre-harvested annotations.
    ///
    /// `ambient` is the definition-package layer (stdlib + project `defs`,
    /// [`crate::defs::Ambient`]) merged *beneath* the file's own
    /// declarations: its classes/enums/functions/globals seed the
    /// environment first, so a same-named file declaration shadows them.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn build_from_items(
        parse: &lua::Parse,
        items: &[luacats::AnnotatedItem],
        ambient: Option<&crate::defs::Ambient>,
    ) -> TypeEnv {
        let mut decl = Declared::default();
        // Ambient type names must be visible to the file's lowerer so
        // annotations that reference stdlib classes/aliases don't trip
        // LB0305. File-declared names (inserted below) win on collision.
        if let Some(ambient) = ambient {
            for name in ambient.env.classes.keys() {
                decl.classes.insert(name.clone());
            }
            for name in ambient.env.enums.keys() {
                decl.enums.insert(name.clone());
            }
            for (name, alias) in &ambient.aliases {
                decl.aliases.insert(name.clone(), alias.clone());
            }
        }
        decl.absorb_tags(items);

        let mut env = TypeEnv::default();
        // Seed the ambient declarations beneath the file's own.
        if let Some(ambient) = ambient {
            env.classes = ambient.env.classes.clone();
            env.enums = ambient.env.enums.clone();
            env.functions = ambient.env.functions.clone();
            env.global_types = ambient.env.global_types.clone();
            env.cross_file_class_decl_spans = ambient.env.cross_file_class_decl_spans.clone();
        }
        let mut lowerer = Lowerer::new(&decl);
        let root = parse.syntax();
        // Harvested here rather than at its own use site further down (PR
        // #61 finding 2): inline `--[[@as T]]` casts are lowered by the very
        // `lowerer` whose `generic_classes` the pre-scan below gates, so they
        // are part of "does this file reference a generic class" and must be
        // available before that question is asked, not after.
        let inline_as = luacats::harvest_inline_as(parse);
        // Build generic `---@class Name<T>` templates before the main pass so
        // references (`Name<number>`) resolve regardless of declaration order,
        // and ambient generic classes are reachable too (#84).
        lowerer.generic_classes =
            collect_generic_classes(items, &inline_as, &root, ambient, &decl, &env);
        for item in items {
            lowerer.generics = block_generics(item);
            env.absorb_block(item, &mut lowerer, &root);
        }
        // The standalone `---@private`/`---@protected`/`---@package` visibility
        // on `function Class:method` doc blocks, resolved once every class in
        // the file is known. (`local_classes` — the classes this file itself
        // declares, the `package`-visibility test of #115 — is filled by
        // `absorb_block` as each `---@class` is seen, because the duplicate
        // merge of #49 needs it during the walk, not after it.)
        env.absorb_standalone_visibility(items, &root);
        // Inline `--[[@as T]]` casts: anchor each to the end offset of the
        // expression it directly follows (skipping back over whitespace).
        if !inline_as.is_empty() {
            lowerer.generics.clear();
            let text = root.text().to_string();
            let bytes = text.as_bytes();
            for cast in &inline_as {
                let ty = lowerer.lower(&cast.ty);
                let mut anchor = cast.span.start.min(bytes.len());
                while anchor > 0 && bytes[anchor - 1].is_ascii_whitespace() {
                    anchor -= 1;
                }
                if anchor > 0 {
                    env.as_casts.insert(anchor, ty);
                }
            }
        }
        env.unknown_names = std::mem::take(&mut lowerer.unknown_names);
        env.arity_errors = std::mem::take(&mut lowerer.arity_errors);
        env.cyclic_aliases = std::mem::take(&mut lowerer.cyclic_aliases);
        env
    }

    /// Build the ambient environment for a definition package: a set of
    /// `---@meta` `.d.lua` sources, each already parsed and harvested.
    ///
    /// All files share one declared-name universe (so a class declared in
    /// `io.d.lua` is referenceable from `os.d.lua`), then each is lowered
    /// independently and its name-keyed declarations merged — module tables
    /// and scalar globals surface as [`TypeEnv::global_type`] entries. The
    /// returned alias map lets a consuming file's lowerer expand ambient
    /// `---@alias`es without re-parsing the packages.
    pub(crate) fn build_ambient(
        files: &[(lua::Parse, Vec<luacats::AnnotatedItem>)],
    ) -> (TypeEnv, BTreeMap<String, AliasTag>) {
        let mut decl = Declared::default();
        for (_, items) in files {
            decl.absorb_tags(items);
        }

        let mut env = TypeEnv::default();
        for (parse, items) in files {
            let root = parse.syntax();
            let mut file_env = TypeEnv::default();
            let mut lowerer = Lowerer::new(&decl);
            for item in items {
                lowerer.generics = block_generics(item);
                file_env.absorb_block(item, &mut lowerer, &root);
            }
            // Definition files are never inferred, so their carrier-style
            // member definitions are folded into the class surface here (#39).
            file_env.absorb_carrier_members(items, &root);
            file_env.collect_global_types(&root);
            // Name-keyed maps merge *first-wins*: definition files are supplied
            // in winner-first order (stdlib base, then project-local defs, then
            // dependency defs alphabetically — #108's `workspace.library`
            // precedence), so an earlier source's declaration of a name is kept
            // and a later duplicate is dropped. The collision itself is
            // reported separately (`LB0307`); here we only pick the winner. The
            // stdlib layer never collides across its own files, so this is a
            // no-op for the cached stdlib path.
            merge_keep_first(&mut env.classes, file_env.classes);
            merge_keep_first(&mut env.enums, file_env.enums);
            merge_keep_first(&mut env.functions, file_env.functions);
            merge_keep_first(&mut env.global_types, file_env.global_types);
            env.unknown_names.append(&mut lowerer.unknown_names.clone());
            env.arity_errors.append(&mut lowerer.arity_errors.clone());
            env.cyclic_aliases
                .append(&mut lowerer.cyclic_aliases.clone());
        }
        // The declared-name universe already holds every ambient alias raw;
        // hand that map back rather than maintaining a second copy.
        (env, decl.aliases)
    }

    /// Clone the name-keyed ambient surface of this environment — the four
    /// maps [`Self::build_from_items`] seeds a file's environment from
    /// (classes, enums, functions, global types). Range-keyed per-file maps
    /// are irrelevant to an ambient layer and stay empty. Used by
    /// [`crate::Ambient::with_project_types`] to derive a merged layer
    /// without mutating the shared base.
    pub(crate) fn clone_surface(&self) -> TypeEnv {
        TypeEnv {
            classes: self.classes.clone(),
            enums: self.enums.clone(),
            functions: self.functions.clone(),
            global_types: self.global_types.clone(),
            cross_file_class_decl_spans: self.cross_file_class_decl_spans.clone(),
            ..TypeEnv::default()
        }
    }

    /// Merge one project file's workspace-global declarations beneath this
    /// (ambient) environment. See [`crate::Ambient::with_project_types`] for
    /// the semantics: absent classes insert whole; present classes merge
    /// member-wise with the existing (defs) declaration winning same-name
    /// collisions, and a carrier attachment never shadowing a `---@field`;
    /// enums merge first-wins.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn merge_file_types(&mut self, file: &FileTypes) {
        // First-wins, matching every other axis of this merge: the first
        // file to declare a name is the one a "declared here" secondary
        // points at (round 3 review F71).
        for (name, location) in &file.class_decl_spans {
            self.cross_file_class_decl_spans
                .entry(name.clone())
                .or_insert_with(|| location.clone());
        }
        for (name, def) in &file.classes {
            match self.classes.get_mut(name) {
                None => {
                    self.classes.insert(name.clone(), def.clone());
                }
                Some(existing) => {
                    // Type parameters follow the same first-wins rule as
                    // members, with the same allowance the in-file merge makes:
                    // a declaration that named none has none to keep. Read
                    // before the parents loop below: the rename it drives
                    // (#59, F38) must be computed against the canonical
                    // params already on record, and applied to `parent.args`
                    // *before* a parent is pushed — otherwise a pushed
                    // argument keeps this declaration's own (non-canonical)
                    // spelling.
                    adopt_params_if_unset(&mut existing.params, &def.params);
                    // …and one that spells them differently is unified
                    // **positionally**, exactly as two declarations in one file
                    // are, so its field bodies — and its parent references'
                    // type arguments — still substitute at an instantiation
                    // site instead of leaking the other declaration's
                    // parameter name through the merged class.
                    let rename = class_param_unification(&def.params, &existing.params);
                    // Parents dedup by NAME: two declarations naming one
                    // parent with different arguments are the same edge in the
                    // chain, and the first wins as every other member does.
                    for parent in &def.parents {
                        let parent = match &rename {
                            Some(map) => ParentRef {
                                name: parent.name.clone(),
                                args: parent
                                    .args
                                    .iter()
                                    .map(|arg| crate::generics::subst_ty(arg, map))
                                    .collect(),
                            },
                            None => parent.clone(),
                        };
                        push_parent_ref(&mut existing.parents, parent);
                    }
                    let subst = |field: &FieldTy| match &rename {
                        Some(map) => FieldTy {
                            ty: crate::generics::subst_ty(&field.ty, map),
                            optional: field.optional,
                        },
                        None => field.clone(),
                    };
                    for (field, ty) in &def.fields {
                        if member_wins(existing.fields.get(field), MemberArrival::Duplicate) {
                            existing.fields.insert(field.clone(), subst(ty));
                        }
                    }
                    for (member, ty) in &def.methods {
                        // As in [`FileTypes::collect`]: the existing `---@field`
                        // declaration wins on type, but inherits the incoming
                        // attachment's use-site tags (#33).
                        if let Some(declared) = existing.fields.get(member) {
                            let merged = with_carrier_tags(declared, &subst(ty));
                            existing.fields.insert(member.clone(), merged);
                        } else if member_wins(
                            existing.methods.get(member),
                            MemberArrival::Duplicate,
                        ) {
                            existing.methods.insert(member.clone(), subst(ty));
                        }
                    }
                    // First declaration wins, matching the fields loop above
                    // (round 5 review N7): deduping on the full `(key,
                    // value)` pair alone let two declarations that conflict
                    // on *value* for the same key both survive as separate
                    // entries, which `TypeEnv::collect_class`'s consumer then
                    // resolved by walk order — a second, silent precedence
                    // rule disagreeing with this file's own first-wins one.
                    // Built once for this class, not re-scanned per incoming
                    // entry (round 6 review M55): `.iter().any()` here cost
                    // O(existing.indexers.len()) on every incoming indexer,
                    // making a class with K `---@field [K] V` entries cost
                    // O(K²) to merge.
                    let mut indexers = IndexerIndex::of(&existing.indexers);
                    for (key, value) in &def.indexers {
                        let (key, value) = match &rename {
                            Some(map) => (
                                crate::generics::subst_ty(key, map),
                                crate::generics::subst_ty(value, map),
                            ),
                            None => (key.clone(), value.clone()),
                        };
                        indexers.absorb(
                            &mut existing.indexers,
                            key,
                            value,
                            MemberArrival::Duplicate,
                        );
                    }
                    // Operator signatures follow the same positional rename
                    // as fields/methods/indexers above (round 4 review R5):
                    // without it, a re-declaration that spells the class's
                    // type parameters differently left an inherited
                    // `---@operator add(U): U` naming `U` on a merged class
                    // whose canonical parameter list reads `T`, unresolvable
                    // at every use site and, when it reaches a diagnostic, a
                    // raw type-parameter name in user-facing text — the same
                    // class of defect F42/F43 exist to prevent.
                    for (op, sigs) in &def.operators {
                        let slot = existing.operators.entry(op.clone()).or_default();
                        for sig in sigs {
                            let sig = match &rename {
                                Some(map) => OperatorSig {
                                    input: sig
                                        .input
                                        .as_ref()
                                        .map(|ty| crate::generics::subst_ty(ty, map)),
                                    result: crate::generics::subst_ty(&sig.result, map),
                                },
                                None => sig.clone(),
                            };
                            push_operator_overload(slot, sig);
                        }
                    }
                    for (member, scope) in &def.visibility {
                        if member_wins(existing.visibility.get(member), MemberArrival::Duplicate) {
                            existing.visibility.insert(member.clone(), *scope);
                        }
                    }
                }
            }
        }
        for (name, def) in &file.enums {
            self.enums
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
    }

    /// Merge one *implicit* surface — a class/enum set harvested from a
    /// vendored rock source (#30) — beneath this environment, keeping any name
    /// already claimed **whole**.
    ///
    /// The difference from [`Self::merge_file_types`] is deliberate. Two
    /// declarations of a class in code the user *wrote* are two halves of one
    /// intent, so luals (and luabox) union their members. A rock's declaration
    /// is not the user's: when a project declares `mylib.Point` itself — in
    /// `[types] defs` or in its own source — it is correcting or replacing what
    /// the rock says, and unioning the rock's fields back in would defeat the
    /// escape hatch. So an already-claimed name is left exactly as it is, and
    /// only unclaimed names are inserted.
    ///
    /// Deliberately does NOT propagate `class_decl_spans` (round 4 review
    /// R33, corrected after an earlier pass in this same round got it
    /// backwards and broke the acceptance suite): unlike a project file, a
    /// rock's declaring source is vendored code the user did not write and
    /// cannot edit. `luarocks-tree.feature`'s "misusing a rock type is
    /// reported in the consumer, not the rock" scenario is the encoded
    /// contract — it asserts the undefined-field diagnostic names the
    /// consumer (`src/main.lua`) and explicitly asserts the secondary does
    /// NOT mention `lua_modules` at all. A "declared here" pointing into
    /// vendored code adds a location the reader cannot act on and re-blames
    /// the rock for the consumer's own mistake — the opposite of what F71's
    /// remedy exists to do. So `insert_unclaimed_types` leaving
    /// `cross_file_class_decl_spans` untouched is not the third seam F71
    /// missed; it is the one seam that must stay closed.
    pub(crate) fn insert_unclaimed_types(&mut self, file: &FileTypes) {
        for (name, def) in &file.classes {
            self.classes
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
        for (name, def) in &file.enums {
            self.enums
                .entry(name.clone())
                .or_insert_with(|| def.clone());
        }
    }

    /// Bind module tables (`math = {}` under `---@class mathlib`) and scalar
    /// globals (`_VERSION` under `---@type string`) to their declared types,
    /// so field reads like `math.pi` and `_VERSION` resolve. Called once per
    /// definition file, before its range-keyed maps are merged away.
    ///
    /// Also folds def-declared *value* fields on global tables
    /// (`zlib.version = "1.3.1"` with a `---@type string`, or a bare typed
    /// literal) into the base table's shape — the class shape when the base is
    /// `---@class`-declared, or the structural table registered for a plain
    /// `zlib = {}` (the dominant def style) — so `zlib.version` reads back as
    /// its declared/literal type. Function fields already register by dotted
    /// name, but scalar fields had no home before (#105). Statements are
    /// visited in document order, so an intermediate `mylib.sub = {}` folds
    /// before the nested `mylib.sub.const = ...` that resolves through it.
    fn collect_global_types(&mut self, root: &SyntaxNode) {
        for node in root.descendants() {
            let Some(Stmt::Assign(assign)) = Stmt::cast(node.clone()) else {
                continue;
            };
            let targets: Vec<Expr> = assign
                .targets()
                .map(|t| t.exprs().collect())
                .unwrap_or_default();
            let [target] = &targets[..] else {
                continue; // only single-target assignments carry a type here
            };
            let range = node.text_range();
            let key = (usize::from(range.start()), usize::from(range.end()));
            match target {
                Expr::Name(name) => {
                    let Some(name) = name.name() else { continue };
                    if let Some(class) = self.declared_targets.get(&key) {
                        self.global_types
                            .insert(name.text().to_string(), Ty::Named(class.clone()));
                    } else if let Some(types) = self.typed_locals.get(&key)
                        && let Some(ty) = types.first()
                    {
                        self.global_types
                            .insert(name.text().to_string(), ty.clone());
                    } else if is_table_constructor(&assign) {
                        // A *plain* global table (`love = {}`, no `---@class`
                        // — the dominant def style): register an empty
                        // structural table so later dotted value-field
                        // assignments fold into it (#105). `or_insert` keeps
                        // already-folded fields if the name is (unusually)
                        // assigned twice.
                        self.global_types
                            .entry(name.text().to_string())
                            .or_insert_with(|| Ty::Table(Box::default()));
                    }
                }
                Expr::Field(field) => {
                    let (Some(field_name), Some(base)) = (field.field_name(), field.base()) else {
                        continue;
                    };
                    let Some(ty) = self.field_value_ty(&assign, key) else {
                        continue;
                    };
                    // A `---@class`-declared base folds into the class shape; a
                    // plain-table base folds into the structural table stored in
                    // `global_types` (nested paths walk intermediate table
                    // fields either way).
                    if let Some(class) = self.resolve_path_class(&base) {
                        if let Some(def) = self.classes.get_mut(&class) {
                            def.fields
                                .entry(field_name.text().to_string())
                                .or_insert(FieldTy {
                                    ty,
                                    optional: false,
                                });
                        }
                    } else if let Some(table) = self.resolve_path_table_mut(&base) {
                        table
                            .fields
                            .entry(field_name.text().to_string())
                            .or_insert(FieldTy {
                                ty,
                                optional: false,
                            });
                    }
                }
                _ => {}
            }
        }
    }

    /// The declared or literal-widened type written to a global-table value
    /// field: a `---@class`/`---@type` on the assignment first, else the
    /// widened type of a bare literal right-hand side (`= "1.3.1"` → `string`,
    /// matching luals), else an empty structural table for a bare `= {}`
    /// sub-table (so deeper `mylib.sub.const = ...` assignments fold through
    /// it). `None` when nothing types the field.
    fn field_value_ty(&self, assign: &lua::ast::AssignStmt, key: Target) -> Option<Ty> {
        if let Some(class) = self.declared_targets.get(&key) {
            return Some(Ty::Named(class.clone()));
        }
        if let Some(ty) = self.typed_locals.get(&key).and_then(|t| t.first()) {
            return Some(ty.clone());
        }
        let value = assign.values().and_then(|v| v.exprs().next())?;
        if matches!(value, Expr::Table(_)) {
            return Some(Ty::Table(Box::default()));
        }
        literal_ty(&value).map(|t| t.widened())
    }

    /// Resolve a dotted global-table path to the mutable structural table it
    /// names: `zlib` → the plain (`---@class`-less) `zlib = {}` table stored
    /// in `global_types`; `zlib.sub` → its nested table field. `None` when the
    /// path is not a plain-table chain — class-declared bases fold through
    /// [`Self::resolve_path_class`] instead.
    fn resolve_path_table_mut(&mut self, expr: &Expr) -> Option<&mut TableTy> {
        let mut segments: Vec<String> = Vec::new();
        let mut cur = expr.clone();
        let base = loop {
            match cur {
                Expr::Name(name) => break name.name()?.text().to_string(),
                Expr::Field(field) => {
                    segments.push(field.field_name()?.text().to_string());
                    cur = field.base()?;
                }
                _ => return None,
            }
        };
        segments.reverse();
        let mut table = match self.global_types.get_mut(&base)? {
            Ty::Table(table) => table.as_mut(),
            _ => return None,
        };
        for segment in &segments {
            table = match &mut table.fields.get_mut(segment)?.ty {
                Ty::Table(next) => next.as_mut(),
                _ => return None,
            };
        }
        Some(table)
    }

    /// Resolve a dotted global-table path to the `---@class` name it is
    /// declared as: `zlib` via its global binding, `mylib.sub` by walking the
    /// intermediate class's field type. `None` for paths that do not bottom
    /// out at a class-typed global table.
    fn resolve_path_class(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Name(name) => match self.global_types.get(name.name()?.text()) {
                Some(Ty::Named(class)) => Some(class.clone()),
                _ => None,
            },
            Expr::Field(field) => {
                let base_class = self.resolve_path_class(&field.base()?)?;
                let member = field.field_name()?;
                match self.classes.get(&base_class)?.fields.get(member.text()) {
                    Some(FieldTy {
                        ty: Ty::Named(class),
                        ..
                    }) => Some(class.clone()),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Process one annotation block: class/field members, function
    /// signatures, `---@type` locals, and enums.
    #[allow(clippy::too_many_lines)]
    fn absorb_block(
        &mut self,
        item: &luacats::AnnotatedItem,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let mut current_class: Option<String> = None;
        // O(1) dedup lookup for the current class's indexer entries (round 6
        // review M55): the tags loop below can add thousands of `---@field
        // [K] V` entries to one class one tag at a time, and re-scanning
        // `class.indexers` with `.iter().any()` on every single tag made a
        // class with K indexer entries cost O(K²). This mirrors
        // `class.indexers`' key set — invalidated (rebuilt lazily, on next
        // use) whenever `current_class` changes rather than on every tag, so
        // it costs O(K) total to build across the whole loop, not O(K) per
        // tag.
        let mut indexers = IndexerIndex::default();
        let mut indexers_for: Option<String> = None;
        // The positional unification a re-declaration's field bodies need when
        // it spells the class's type parameters differently from the canonical
        // (first) list — `None` while the two agree, which is the common case.
        let mut class_rename: Option<BTreeMap<String, Ty>> = None;
        let mut params: Vec<&ParamTag> = Vec::new();
        let mut returns: Vec<&ReturnTag> = Vec::new();
        // Legacy `---@vararg Type`: the deprecated EmmyLua spelling of
        // `---@param ... Type`. luals binds both a `doc.vararg` tag and a
        // `doc.param` `...` tag onto the same AST vararg source and merges
        // (unions) every bound doc's type via `vm.setNode` — see
        // `script/vm/compiler.lua`'s `'...'` case, which loops over
        // `source.bindDocs` and calls `vm.setNode` (additive, not `cover`)
        // for each `doc.vararg`/`doc.param` match. So when both a `---@vararg`
        // and a `---@param ...` tag appear on the same block, the resulting
        // vararg element type is their union, not "last tag wins".
        let mut varargs: Vec<&VarargTag> = Vec::new();
        let mut types: Option<Vec<Ty>> = None;
        let mut type_span: Option<std::ops::Range<usize>> = None;
        let mut overloads: Vec<FunctionTy> = Vec::new();
        let mut casts: Vec<CastEntry> = Vec::new();
        let mut fn_generics: Vec<TypeParam> = Vec::new();
        // `---@deprecated` / `---@nodiscard` / `---@async` attach to the whole
        // doc block, so a block that annotates a function carries them onto
        // that function's signature (#111, #112). Scanned alongside the other
        // tags. `---@async` drives the `await-in-sync` call-site check
        // (LB0316): luals decides a `function` is async iff any of its
        // `bindDocs` is a `doc.async` tag (`script/vm/doc.lua`'s local
        // `isAsync`: `for _, doc in ipairs(value.bindDocs) do if doc.type ==
        // 'doc.async' then value._async = true`).
        let mut deprecated = false;
        let mut nodiscard = false;
        let mut is_async = false;
        // `---@version` rides the same block→signature path as `---@deprecated`
        // (luals has no dedicated version diagnostic — see [`crate::version`]).
        // Parsed out of the raw `SimpleTag` body; an empty/unrecognised body
        // yields `None` and gates nothing.
        let mut version: Option<crate::version::VersionReq> = None;

        for tag in &item.block.tags {
            match tag {
                Tag::Deprecated(_) => deprecated = true,
                Tag::Nodiscard(_) => nodiscard = true,
                Tag::Async(_) => is_async = true,
                Tag::Version(v) => {
                    version = v
                        .text
                        .as_deref()
                        .and_then(crate::version::VersionReq::parse);
                }
                Tag::Class(c) if !c.name.is_empty() => {
                    // Validate each parent reference: lowering a `: Base` that
                    // no class/alias/enum declares records an unknown
                    // name at the parent's own span → LB0305 (#107). Forward
                    // references and defs/ambient parents already sit in the
                    // lowerer's declared-name universe, so they do not fire.
                    //
                    // That lowering answers "is this real?" and then throws the
                    // reference away — it monomorphises to a structural table,
                    // while a parent must stay a NAME for the merge to walk.
                    // So the arguments are lowered a second time and kept:
                    // `: Base<number>` binds `Base`'s parameters in
                    // `class_shape_bound` instead of leaving them free. The
                    // second pass is construction, not diagnosis — the first
                    // one is the sole reporter, so a `: Base<Bogus>` is one
                    // LB0305, not two.
                    //
                    // Routed through `push_parent_ref` rather than a raw
                    // `push` (PR #61 finding 6): parents dedupe by NAME, and
                    // that rule cannot depend on whether the user wrote the
                    // two edges on one `---@class` line or two. It did.
                    // Measured at d20cd47 on `---@class C : Base<number>,
                    // Base<string>`: one declaration kept both edges and the
                    // last-visited one won (`item` = String) while the same
                    // two edges split across two declarations — which already
                    // went through `push_parent_ref`, at the re-declaration
                    // branch below — deduped to the first (`item` = Number).
                    // Opposite answers, both silent, and both contradicting
                    // `push_parent_ref`'s own documented "the **first** wins,
                    // matching every other duplicate-declaration axis".
                    let mut parents: Vec<ParentRef> = Vec::new();
                    for parent in &c.parents {
                        lowerer.lower(parent);
                        if let TypeExprKind::Named { name, args } = &parent.kind {
                            let quiet = QuietMark::of(lowerer);
                            let args = args.iter().map(|a| lowerer.lower(a)).collect();
                            quiet.rollback(lowerer);
                            push_parent_ref(
                                &mut parents,
                                ParentRef {
                                    name: name.clone(),
                                    args,
                                },
                            );
                        }
                    }
                    // Two declarations of one class in the same file are two
                    // halves of one intent, exactly as two declarations across
                    // files are: they **union** (#49). Before this, the second
                    // declaration replaced the first and every member it had
                    // collected vanished — a merge rule that depended on the
                    // file boundary, which luals has no notion of.
                    //
                    // The *first* declaration in this file still replaces any
                    // ambient (stdlib / `[types] defs`) class of the same name
                    // whole: that is the escape hatch (see
                    // `Ambient::with_rock_types`), a different axis from
                    // duplicate declarations in code the user wrote.
                    // `local_classes` — the set of names *this file* declares —
                    // is what tells the two apart, so it is filled here rather
                    // than in a later sweep.
                    //
                    // `methods` is the one axis carried over rather than reset:
                    // when this file checks *itself*, `self.classes` is seeded
                    // from the project-wide ambient (`build_from_items`, which
                    // clones `ambient.env.classes` before any `absorb_block`
                    // call runs) — and that ambient is self-inclusive
                    // (`check_cmd.rs`'s `run_passes` folds every file's own
                    // `FileTypes` in, itself included), so a class this file
                    // declares already carries *this file's own* carrier
                    // attachments (`function Class:method()`), reified by
                    // `FileTypes::collect`'s post-inference carrier fold on an
                    // earlier pass over this same file. Resetting the whole
                    // `ClassDef` here — as every other field does, correctly,
                    // since `absorb_block`'s own `Tag::Field`/`Tag::Operator`
                    // arms re-derive them from scratch in this very pass —
                    // silently discarded that seeded `methods` map, which
                    // nothing else in `absorb_block` (or anywhere else before
                    // this file's own obligations are checked) ever
                    // repopulates: carrier methods are folded in only by
                    // `FileTypes::collect`, run *after* this file's own
                    // `TypeEnv` is built and checked, and its output feeds
                    // *other* files via `merge_file_types`, never this one.
                    // The result was a same-file `: Parent` inheriting a
                    // carrier-attached method reading as a false `LB0306`
                    // (`collect_class`'s ancestry walk finds an empty
                    // `methods` map on the parent) even though the identical
                    // shape resolves cleanly the moment the parent moves to
                    // its own file — cross-file consumption never hits this
                    // reset because a non-locally-declared class is left
                    // exactly as the ambient seeded it.
                    if self.local_classes.insert(c.name.clone()) {
                        let methods = self
                            .classes
                            .get(&c.name)
                            .map(|def| def.methods.clone())
                            .unwrap_or_default();
                        self.classes.insert(
                            c.name.clone(),
                            ClassDef {
                                parents,
                                params: c.params.clone(),
                                methods,
                                ..ClassDef::default()
                            },
                        );
                        class_rename = None;
                    } else {
                        // A re-declaration may name the type parameters the
                        // first one omitted; it never renames them
                        // (first-wins, as for every other member) — done
                        // BEFORE the rename below is computed, not after: a
                        // bare first declaration has an EMPTY canonical list
                        // until this runs, and computing the rename against
                        // that stale empty list mapped this very
                        // declaration's own, now-canonical parameter to
                        // `unknown` instead of leaving it alone (own ==
                        // canonical needs the update to have already
                        // happened to see that they agree).
                        if let Some(existing) = self.classes.get_mut(&c.name) {
                            adopt_params_if_unset(&mut existing.params, &c.params);
                        }
                        // A re-declaration's `---@field` bodies — and, per
                        // #59/F38, its *parent references' type arguments* —
                        // are written against the type parameters *it* names,
                        // so when those differ from the canonical list they
                        // are unified **positionally** here, before either is
                        // absorbed: slot `i` is one type variable however the
                        // two declarations spell it. The rename must exist
                        // *before* `parents` is pushed, or the pushed
                        // argument keeps the re-declaration's own
                        // (non-canonical) spelling. Without this the merged
                        // class carries a parent argument (or a field type)
                        // naming a parameter its own parameter list never
                        // mentions, unresolvable at every instantiation site —
                        // visible across the `require` boundary, where the
                        // merged class is what the consumer sees.
                        class_rename = self
                            .classes
                            .get(&c.name)
                            .and_then(|def| class_param_unification(&c.params, &def.params));
                        if let Some(existing) = self.classes.get_mut(&c.name) {
                            for parent in parents {
                                let parent = match &class_rename {
                                    Some(map) => ParentRef {
                                        name: parent.name,
                                        args: parent
                                            .args
                                            .iter()
                                            .map(|arg| crate::generics::subst_ty(arg, map))
                                            .collect(),
                                    },
                                    None => parent,
                                };
                                push_parent_ref(&mut existing.parents, parent);
                            }
                        }
                    }
                    current_class = Some(c.name.clone());
                    // First-wins, matching the fields: the "declared here"
                    // label points at the declaration that introduced the name.
                    self.class_decl_spans
                        .entry(c.name.clone())
                        .or_insert(c.span.start..c.span.end);
                    if let Some(span) = item.target {
                        self.declared_targets
                            .insert((span.start, span.end), c.name.clone());
                        self.class_tag_spans
                            .insert((span.start, span.end), c.span.start..c.span.end);
                    }
                }
                Tag::Field(f) => {
                    let Some(class) = current_class
                        .as_ref()
                        .and_then(|name| self.classes.get_mut(name))
                    else {
                        continue; // a stray @field outside a @class block
                    };
                    let ty = lowerer.lower(&f.ty);
                    let ty = match &class_rename {
                        Some(map) => crate::generics::subst_ty(&ty, map),
                        None => ty,
                    };
                    match &f.key {
                        FieldKey::Name(name) => {
                            // **First declaration wins**, whether the duplicate
                            // sits in this `---@class` block, in a second block
                            // for the same class in this file (#49), or in
                            // another file (`TypeEnv::merge_file_types`, which
                            // has always been first-wins). `LB0311`
                            // (`duplicate-doc-field`) already promised exactly
                            // this in its note — "the first declaration wins;
                            // remove or rename this one" — and now the stored
                            // type agrees with the message. luals unions the
                            // two declared types instead; luabox keeps the
                            // first deterministically and warns (#110) — a
                            // duplicate `---@enum` keeps the first too
                            // (`Tag::Enum` below), though nothing currently
                            // warns on it (round 6 review M53).
                            if !member_wins(class.fields.get(name), MemberArrival::Duplicate) {
                                continue;
                            }
                            class.fields.insert(
                                name.clone(),
                                FieldTy {
                                    ty,
                                    optional: f.optional,
                                },
                            );
                            // Record a non-public `---@field <scope>` modifier
                            // for the visibility check (#115). A plain (public)
                            // field is left out of the map — and clears any
                            // inherited restriction — so a same-name public
                            // re-declaration on a *subclass* reads as public.
                            match f.scope {
                                Some(FieldScope::Public) | None => {
                                    class.visibility.remove(name);
                                }
                                Some(scope) => {
                                    class.visibility.insert(name.clone(), scope);
                                }
                            }
                        }
                        FieldKey::Indexer(key) => {
                            let key = lowerer.lower(key);
                            let key = match &class_rename {
                                Some(map) => crate::generics::subst_ty(&key, map),
                                None => key,
                            };
                            // First declaration wins, matching `FieldKey::Name`
                            // just above (round 5 review N7): before this, a
                            // re-declaration's conflicting `---@field [K] V`
                            // was simply appended, so a duplicate class ended
                            // up with two indexer entries for one key and
                            // `TypeEnv::collect_class`'s consumer picked
                            // whichever it walked last — the opposite winner
                            // from every other duplicate-member axis in this
                            // block, none of which warned either.
                            if indexers_for.as_deref() != current_class.as_deref() {
                                indexers = IndexerIndex::of(&class.indexers);
                                indexers_for.clone_from(&current_class);
                            }
                            indexers.absorb(&mut class.indexers, key, ty, MemberArrival::Duplicate);
                        }
                    }
                }
                Tag::Operator(o) if !o.op.is_empty() => {
                    let Some(class) = current_class
                        .as_ref()
                        .and_then(|name| self.classes.get_mut(name))
                    else {
                        continue; // a stray @operator outside a @class block
                    };
                    let input = o.input.as_ref().map(|t| lowerer.lower(t));
                    let result = lowerer.lower(&o.result);
                    // Same-file positional rename, matching `Tag::Field`
                    // above (round 4 review R5): a re-declaration's operator
                    // bodies are written against the type parameters *it*
                    // names, exactly like its field bodies.
                    let input = match &class_rename {
                        Some(map) => input.map(|ty| crate::generics::subst_ty(&ty, map)),
                        None => input,
                    };
                    let result = match &class_rename {
                        Some(map) => crate::generics::subst_ty(&result, map),
                        None => result,
                    };
                    push_operator_overload(
                        class.operators.entry(o.op.clone()).or_default(),
                        OperatorSig { input, result },
                    );
                }
                Tag::Param(p) => params.push(p),
                Tag::Return(r) => returns.push(r),
                Tag::Type(t) => {
                    types = Some(t.types.iter().map(|ty| lowerer.lower(ty)).collect());
                    type_span = Some(t.span.start..t.span.end);
                }
                Tag::Enum(e) if !e.name.is_empty() => {
                    // First declaration wins, matching every other same-file
                    // duplicate-declaration axis in this block (round 6
                    // review M53: this used to `insert` unconditionally —
                    // last-wins, silently, the one axis in this file that
                    // disagreed with "the file boundary does not change the
                    // merge" and with its own neighbouring `Tag::Field` arm's
                    // comment, which already claimed enums made this same
                    // trade). No diagnostic fires on the dropped duplicate
                    // yet — unlike `LB0311` for fields, nothing scans for a
                    // repeated `---@enum` name — so this only fixes which
                    // declaration's members are kept, not silence about it.
                    //
                    // …but *only* against another declaration in this file.
                    // `self.enums` is seeded from the ambient (stdlib /
                    // `[types] defs`) layer before this pass ever runs
                    // (`build_from_items`, whose own doc says the file's
                    // declarations go on top: "a same-named file declaration
                    // shadows them"), so a bare `member_wins` here read the
                    // definition package's entry as if it were the user's own
                    // first declaration and dropped the file's — PR #61
                    // finding 1. `local_enums` is the enum counterpart of the
                    // `local_classes` escape hatch the `---@class` arm above
                    // already uses for exactly this: the FIRST local
                    // declaration replaces the ambient entry whole, and only
                    // a later one in the same file is a duplicate that loses.
                    // Measured at d20cd47 on a file re-declaring an ambient
                    // enum: `enum_members` returned the ambient's members, so
                    // the file got a false `LB0300` on each of its own and a
                    // false ACCEPT of the ambient-only value.
                    let shadows_ambient = self.local_enums.insert(e.name.clone());
                    if shadows_ambient
                        || member_wins(self.enums.get(&e.name), MemberArrival::Duplicate)
                    {
                        let def = enum_def(e, item.target, root);
                        self.enums.insert(e.name.clone(), def);
                    }
                }
                Tag::Overload(o) => {
                    if let Ty::Function(func) = lowerer.lower(&o.ty) {
                        overloads.push(*func);
                    }
                }
                Tag::Cast(c) if !c.var.is_empty() => casts.push(lower_cast(c, lowerer)),
                Tag::Generic(g) => {
                    for p in &g.params {
                        fn_generics.push(TypeParam {
                            name: p.name.clone(),
                            constraint: p.constraint.as_ref().map(|c| lowerer.lower(c)),
                        });
                    }
                }
                Tag::Vararg(v) => varargs.push(v),
                _ => {}
            }
        }

        let target = item.target.map(|span| (span.start, span.end));
        if let Some(target) = target
            && !casts.is_empty()
        {
            self.casts.entry(target).or_default().append(&mut casts);
        }
        // A bare `---@deprecated`/`---@nodiscard` block (no `---@param`/
        // `---@return`) must still attach a signature so the flag reaches the
        // function's use sites; `attach_function` no-ops on non-function
        // targets, so deprecating a `---@class` carrier is harmless here.
        let has_sig_tags = !params.is_empty()
            || !returns.is_empty()
            || !overloads.is_empty()
            || !varargs.is_empty();
        // The block's use-site tags on their own — `fun(…)` syntax has nowhere
        // to write them, so a `---@type fun(…)` declaration folds them in
        // ([`merge_block_tags`], #38).
        let block_tags = FunctionTy {
            deprecated,
            nodiscard,
            is_async,
            version: version.clone(),
            ..FunctionTy::default()
        };
        if (has_sig_tags || deprecated || nodiscard || is_async || version.is_some())
            && let Some(target) = target
        {
            let mut func = FunctionTy {
                overloads,
                generics: fn_generics,
                deprecated,
                nodiscard,
                is_async,
                version,
                // A signature was *written* here, so calls may be checked
                // against it — including from another module, where this flag
                // is the only surviving evidence that a human wrote one (#46).
                // A block carrying only use-site flags declares no signature.
                declared: has_sig_tags,
                ..FunctionTy::default()
            };
            // With no signature tags the block contributes only the flag:
            // stay fully arity/type-permissive (an unannotated function is
            // never arity-checked), so `---@deprecated`/`---@async` alone never
            // manufactures an `LB0301`/`LB0300`.
            if !has_sig_tags {
                func.varargs = Some(Ty::Any);
            }
            self.attach_function(&params, &returns, &varargs, func, target, lowerer, root);
        }
        if let (Some(types), Some(target)) = (types, target) {
            self.attach_typed_functions(&types, target, root, &block_tags);
            self.typed_locals.insert(target, types);
            if let Some(span) = type_span {
                self.typed_local_spans.insert(target, span);
            }
        }
    }

    /// Register the callables a `---@type fun(…)` declares on an *assignment*
    /// under their dotted names (#38).
    ///
    /// A dotted call (`M.a(…)`) is argument-checked through the by-name
    /// callable map — the same map `function M.a() end` and
    /// `---@param`-annotated `M.a = function() end` register into — so an
    /// explicit `---@type fun(…)` has to land there too, or the declared
    /// signature reaches the value but never the call site. `---@type A, B` is
    /// positional, so each target takes the type in its own slot and a lone
    /// annotation over `a, b = f, g` declares `a` only.
    ///
    /// Restricted to a function-*literal* right-hand side: `---@type fun(…)`
    /// over a call result or a name declares that variable's type without
    /// defining a callable here, and inference already carries it.
    fn attach_typed_functions(
        &mut self,
        types: &[Ty],
        target: Target,
        root: &SyntaxNode,
        block_tags: &FunctionTy,
    ) {
        let Some(Stmt::Assign(assign)) = stmt_at(root, target) else {
            return;
        };
        let Some(target_list) = assign.targets() else {
            return;
        };
        let values: Vec<Expr> = assign
            .values()
            .map(|v| v.exprs().collect())
            .unwrap_or_default();
        for (i, assigned) in target_list.exprs().enumerate() {
            let Some(Ty::Function(sig)) = types.get(i) else {
                continue;
            };
            if !matches!(values.get(i), Some(Expr::Function(_))) {
                continue;
            }
            let Some(name) = assign_target_name(&assigned) else {
                continue;
            };
            self.functions
                .insert(name, merge_block_tags((**sig).clone(), Some(block_tags)));
        }
    }

    /// Record the standalone `---@private` / `---@protected` / `---@package`
    /// visibility declared on `function Class:method` (or `function
    /// Class.member`) doc blocks (#115). The `---@field <scope>` modifier form
    /// is handled inline in [`Self::absorb_block`]; this covers the tag-above-a-
    /// method form luals also accepts.
    ///
    /// The carrier *variable* a method attaches to (`function M:m()`) need not
    /// share the `---@class` name (`---@class Animal local M = {}`), so a
    /// variable→class map is built first from every class carrier's declaring
    /// statement, then each method block's first path segment is resolved
    /// through it. A block that also carries a `---@class` is skipped (the tag
    /// then documents the class, not a member). Unresolvable carriers are left
    /// alone — the conservative direction (no false `invisible`).
    fn absorb_standalone_visibility(
        &mut self,
        items: &[luacats::AnnotatedItem],
        root: &SyntaxNode,
    ) {
        let var_to_class = carrier_var_classes(items, root);

        for item in items {
            // A block documenting a class is not a member visibility block.
            if item.block.tags.iter().any(|t| matches!(t, Tag::Class(_))) {
                continue;
            }
            let Some(scope) = item.block.tags.iter().find_map(standalone_scope) else {
                continue;
            };
            let Some(span) = item.target else { continue };
            let Some(stmt) = stmt_at(root, (span.start, span.end)) else {
                continue;
            };
            let Some((carrier, member)) = visibility_carrier_member(&stmt) else {
                continue;
            };
            let Some(class) = var_to_class.get(&carrier) else {
                continue;
            };
            if let Some(def) = self.classes.get_mut(class) {
                // First declaration wins (round 6 review M6): this used to
                // write unconditionally, so two standalone visibility blocks
                // for one member in one file left file order deciding the
                // winner (last-wins), while `merge_file_types`'s identical
                // cross-file case already routed through `member_wins` and
                // got first-wins — the same member kind deciding its own
                // precedence two different ways depending on which side of a
                // file boundary the duplicate landed on, against
                // `push_operator_overload`'s own stated invariant that "the
                // file boundary does not change the merge".
                if member_wins(def.visibility.get(&member), MemberArrival::Duplicate) {
                    def.visibility.insert(member, scope);
                }
            }
        }
    }

    /// Fold carrier-style member definitions — `function Class:method(…)` and
    /// `function Class.fn(…)` — into their class's surface (#39).
    ///
    /// A checked *project* file gets this for free: inference walks the carrier
    /// table and [`FileTypes::collect`] folds the reified shape into the class.
    /// A `---@meta` definition file is never inferred (it declares, it does not
    /// compute), so a carrier attachment there previously reached nothing and
    /// every use site reported `LB0306`. luals makes no such distinction — a
    /// `function Class:method()` in a library file is a member of `Class`
    /// wherever it is written — so the same attachments are harvested
    /// syntactically here, tags and all.
    ///
    /// The `---@field` declaration stays authoritative on collision, with the
    /// attachment's use-site tags folded in exactly as [`with_carrier_tags`]
    /// does elsewhere (#33). An attachment with no doc block at all still joins
    /// the surface, at a fully permissive signature — its member *exists*, and
    /// an unannotated function is never arity-checked.
    ///
    /// This is the one merge seam that does **not** thread a `class_rename`
    /// (round 5 review N10): fields, operators and parent argument lists all
    /// unify a re-declaration's differently-spelled type parameters onto the
    /// class's canonical ones (`class_param_unification`, `absorb_block`'s
    /// `Tag::Field`/`Tag::Operator` arms), but a carrier-attached method's
    /// signature is harvested from the *function's own* `---@param`/
    /// `---@return` tags, lowered against the ordinary declared-name
    /// universe — a class's own type-parameter placeholders are never in
    /// that scope at all, so a signature here cannot name one in the first
    /// place (referencing it lowers to an unresolved name, `LB0305`). There
    /// is currently no fixture where a rename would have anything to
    /// substitute: this is a documented, currently-unreachable gap, not the
    /// uniform "one owner" guarantee the parent-argument/field/operator
    /// seams give — tracked here rather than silently assumed, so a future
    /// `---@generic`-scoped carrier attachment does not inherit an
    /// unexamined asymmetry.
    fn absorb_carrier_members(&mut self, items: &[luacats::AnnotatedItem], root: &SyntaxNode) {
        let var_to_class = carrier_var_classes(items, root);
        if var_to_class.is_empty() {
            return;
        }
        for node in root.descendants() {
            let Some(Stmt::FunctionDecl(decl)) = Stmt::cast(node.clone()) else {
                continue;
            };
            let Some(name) = decl.name() else { continue };
            let segments: Vec<String> = name.segments().map(|s| s.text().to_string()).collect();
            // One hop only: `function C.a.b()` attaches to the member table
            // `C.a`, not to `C` — this harvest models class members, not
            // nested tables. It is not dropped, though: a dotted declaration
            // is also registered by its full name (`resolve_callable_target`),
            // which is how the stdlib's `function io.open()` is reached.
            let [carrier, member] = segments.as_slice() else {
                continue;
            };
            let Some(class) = var_to_class.get(carrier).cloned() else {
                continue;
            };
            let r = node.text_range();
            let sig = self
                .fn_sigs
                .get(&(usize::from(r.start()), usize::from(r.end())))
                .cloned()
                .unwrap_or_else(FunctionTy::opaque);
            let attached = FieldTy {
                ty: Ty::Function(Box::new(sig)),
                optional: false,
            };
            let Some(def) = self.classes.get_mut(&class) else {
                continue;
            };
            match def.fields.get(member) {
                Some(declared) => {
                    let merged = with_carrier_tags(declared, &attached);
                    def.fields.insert(member.clone(), merged);
                }
                None => {
                    // First declaration wins (round 6 review M7 — the
                    // identical defect as M6, one member kind over): this
                    // used to write unconditionally, so a defs package
                    // declaring one carrier member twice published the
                    // **last** signature in one file and the **first** split
                    // across two, disagreeing with `merge_file_types`'s
                    // cross-file carrier/method handling above.
                    if member_wins(def.methods.get(member), MemberArrival::Duplicate) {
                        def.methods.insert(member.clone(), attached);
                    }
                }
            }
        }
    }

    /// Build a [`FunctionTy`] from `@param`/`@return` tags, reconcile it
    /// with the target function's AST parameter list, and register it under
    /// the function's name. Orchestration only: [`lower_signature_tags`]
    /// lowers the tags, [`resolve_callable_target`] finds the name and AST
    /// parameter list, [`reconcile_params`] binds the two together.
    #[allow(clippy::too_many_arguments)]
    fn attach_function(
        &mut self,
        params: &[&ParamTag],
        returns: &[&ReturnTag],
        varargs_tags: &[&VarargTag],
        mut func: FunctionTy,
        target: Target,
        lowerer: &mut Lowerer<'_>,
        root: &SyntaxNode,
    ) {
        let tag_params = lower_signature_tags(&mut func, params, returns, varargs_tags, lowerer);

        let Some(stmt) = stmt_at(root, target) else {
            return;
        };
        let Some((name, param_list)) = resolve_callable_target(&stmt) else {
            return;
        };
        // A `:` method's implicit `self` is absent from the AST parameter
        // list; [`reconcile_params`] restores it from an explicit
        // `---@param self T` tag, so the callable kind is decided here.
        let is_method =
            matches!(&stmt, Stmt::FunctionDecl(f) if f.name().is_some_and(|n| n.is_method()));
        reconcile_params(&mut func, tag_params, param_list, is_method);

        if let Some(name) = name {
            self.functions.insert(name, func.clone());
        }
        self.fn_sigs.insert(target, func);
    }

    // --- lookups -----------------------------------------------------

    /// The merged structural shape of a class: parents first (depth-first),
    /// own members overriding, with a cycle guard. The class's *own* type
    /// parameters stay free — `Box<T>`'s `item` is `T` here, as a bare `Box`
    /// reference means.
    pub(crate) fn class_shape(&self, name: &str) -> Option<TableTy> {
        self.class_shape_bound(name, &[])
    }

    /// [`Self::class_shape`] with `name`'s own type parameters bound to
    /// `args`, positionally — what a reference that wrote them means.
    ///
    /// This is also how a parent's parameters are bound while merging:
    /// `---@class Sub : Base<number>` gives `Base`'s `U` as `number`, so
    /// `Sub` inherits `item: number` rather than a parameter no one can name.
    /// An argument the reference omits leaves its parameter free, spelled out
    /// as the parameter's own literal name here — correct for the ambient/LSP
    /// callers that want to show a generic template as written (`class_shape`
    /// backs [`Self::generic_classes`] and `crate::defs::Ambient`'s hover
    /// surface). A reference-consuming site that could put that name in front
    /// of a user as if it meant something wants [`Self::class_shape_bound_export`]
    /// instead (finding 5 of the production readiness review).
    pub(crate) fn class_shape_bound(&self, name: &str, args: &[Ty]) -> Option<TableTy> {
        if !self.classes.contains_key(name) {
            return None;
        }
        let mut shape = TableTy::default();
        let mut guard = DiamondGuard::default();
        self.collect_class(name, args, &mut shape, &mut guard, false);
        self.note_limit_trip(&guard, name);
        self.note_cyclic(&guard);
        Some(shape)
    }

    /// [`Self::class_shape_bound`] for the **module-export** seam (#56): the
    /// identical walk, except a class's own unbound trailing parameters
    /// substitute to [`Ty::Unknown`] *at the exact point [`Self::collect_class`]
    /// already decides they are free*, instead of being left as
    /// [`Ty::Named`] and erased afterwards by matching on the parameter's
    /// *spelling* against the fully-resolved shape.
    ///
    /// Despite the name, `require` is no longer this method's only caller
    /// (finding 5 of the production readiness review): every same-file
    /// reference-consuming site that can put a member's type in front of a
    /// user — [`crate::infer::Infer::lookup_shape_field`]/`lookup_ty_field`'s
    /// field reads, [`Self::resolve_named_bound`]'s `ipairs`/`pairs` element
    /// types, `Checker::table_shape`'s table-literal obligation, and
    /// `Checker::check_class_conformance`'s `: Parent` obligation — reuses
    /// this same erasure rather than leaving its own bare-reference case to
    /// leak `T` a second, differently-shaped way. Only [`Self::class_shape`]/
    /// [`Self::class_shape_bound`] (ambient/LSP template display,
    /// [`Self::generic_classes`]) and [`Self::resolve_named`]'s raw baseline
    /// (this method's own `erased != resolved` comparison in
    /// [`crate::infer::Infer::reify_export`]) still want the parameter left
    /// as itself.
    ///
    /// That was the previous shape of this fix
    /// (`class_params_in_scope` — deleted, round 5 review N8/N9 — plus a
    /// blanket `subst_ty` over the flattened result in
    /// [`crate::infer::Infer::reify_export`]): it could not tell a
    /// genuinely-unbound parameter from a bound reference to a real class
    /// that merely happens to share the parameter's spelling, and erased
    /// both alike, everywhere in the merged shape the spelling occurred —
    /// not just the occurrences the unbound parameter actually produced.
    /// `---@class Holder` / `---@field evt Event` / `---@class Emitter<Event>`
    /// / `---@class Sub : Holder, Emitter` is the control: `Emitter`'s own,
    /// entirely unrelated parameter happens to be spelled `Event`, and the
    /// old pass erased `Holder`'s legitimate `evt: Event` reference right
    /// alongside it, everywhere `Sub` is used through `require` — while the
    /// same file, read without crossing `require`, resolved `evt` correctly
    /// (round 5 review N9: the two seams disagreed about what the same
    /// declarations mean). Baking the substitution into each visited
    /// class's own `bound` map — this method, via [`Self::collect_class`]'s
    /// `erase_free` — means `Emitter`'s substitution can only ever touch
    /// occurrences `Emitter`'s own fields produce; `Holder`'s `Event`
    /// reference is substituted through `Holder`'s own `bound` map, which
    /// has no entry for it, so it is untouched, on both sides of `require`
    /// alike.
    pub(crate) fn class_shape_bound_export(&self, name: &str, args: &[Ty]) -> Option<TableTy> {
        if !self.classes.contains_key(name) {
            return None;
        }
        let mut shape = TableTy::default();
        let mut guard = DiamondGuard::default();
        self.collect_class(name, args, &mut shape, &mut guard, true);
        self.note_limit_trip(&guard, name);
        self.note_cyclic(&guard);
        Some(shape)
    }

    /// Record that resolving `root`'s shape/operators tripped one of
    /// [`DiamondGuard`]'s two budgets, if it did, filing it under the right
    /// report queue for which one — the shared tail
    /// [`Self::class_shape_bound`], [`Self::class_shape_bound_export`] and
    /// [`Self::class_operators`] each run once their own top-level
    /// `collect_class`/`collect_operators` call returns. Renamed from
    /// `note_depth_limit` (production readiness review F1): before
    /// [`LimitTrip`] existed there was only one budget and one queue to file
    /// a trip under; now there are two, and this is the single place that
    /// decides which.
    fn note_limit_trip(&self, guard: &DiamondGuard, root: &str) {
        let queue = match &guard.limit_exceeded {
            Some(LimitTrip::Depth) => &self.depth_limit_hits,
            Some(LimitTrip::Cost) => &self.cost_limit_hits,
            None => return,
        };
        queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(root.to_string());
        // Durable record (round 6 review M13): written every time alongside
        // whichever drainable report queue above just got `root`, but never
        // cleared by this env, so `class_ancestry_truncated` keeps answering
        // truthfully for `root` after `take_depth_limit_hits`/
        // `take_cost_limit_hits` empties that queue for an unrelated report.
        // One shared durable set for both trip kinds (production readiness
        // review F1): a truncated shape is equally incomplete either way, so
        // a caller asking only "is this incomplete" does not need to know
        // which budget did it.
        self.truncated_classes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(root.to_string());
    }

    /// Record every class name `guard`'s walk found a true `---@class`
    /// cycle at (round 6 review M67) — the cyclic-class counterpart of
    /// [`Self::note_limit_trip`], with the same call sites and the same
    /// one-shot-report-queue shape, but unlike that one this reports every
    /// name [`DiamondGuard::cyclic`] collected, not just the query root: a
    /// mutual `A : B` / `B : A` cycle should be attributed to whichever
    /// class the walk actually looped back onto, which is not always the
    /// class a caller happened to query first.
    fn note_cyclic(&self, guard: &DiamondGuard) {
        if !guard.cyclic.is_empty() {
            self.cyclic_class_hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(guard.cyclic.iter().cloned());
        }
    }

    /// Fold every [`DiamondGuard`] limit trip `other` recorded into this
    /// env's own ledgers — the fix for the silent-bypass hole production
    /// readiness measurement R1 found: [`collect_generic_classes`] resolves
    /// each `---@class Name<T>` template on a throwaway `discovery: TypeEnv`
    /// (its own `class_shape` call, same as any other), and that env used to
    /// be dropped the moment the template map was built. `note_limit_trip`/
    /// `note_cyclic` had already filed the trip faithfully — just into
    /// `discovery`'s `depth_limit_hits`/`cost_limit_hits`/`truncated_classes`/
    /// `cyclic_class_hits`, never `self`'s — so a `Name<Args>` reference
    /// resolved entirely through the discovery-built template (the common
    /// case: `lower_named` substitutes straight from the template, never
    /// re-walking `self`'s own ancestry for that name) left `self`'s ledgers
    /// with no record the walk ever ran long, and
    /// `crate::check::report_depth_limit_hits`/`report_cost_limit_hits` had
    /// nothing to drain. Reached only through a non-generic leaf class did
    /// the checker fall back to walking `self` directly, tripping `self`'s
    /// own ledgers the ordinary way — which is why that path always reported
    /// correctly while the direct-name path stayed silent. `other` is
    /// discarded by its caller right after this call, so a plain read +
    /// extend (not a drain) is enough; `self`'s own drain-on-report contract
    /// ([`Self::take_depth_limit_hits`] etc.) is untouched.
    ///
    /// `roots` is the set of generic class names *this file's own*
    /// annotations reference — the pre-scan's own output — and it scopes the
    /// two **reportable** ledgers (PR #61 finding 3). The discovery walk
    /// resolves a template for every generic class it can reach, not only
    /// the ones this file names, so forwarding its trips wholesale spilled
    /// one `LB0319` per over-budget class in an ambient hierarchy into every
    /// file that named anything generic at all: measured at d20cd47, one
    /// 50-level over-budget defs hierarchy produced **30** distinct-message
    /// `LB0319`s per consuming file, all at `Span(file, 0..0)`, which the
    /// CLI's dedupe cannot collapse because its key is message + file. The
    /// bound: a file reports a trip only for a root it actually references;
    /// a deeper ancestor's own trip belongs to whichever file references
    /// THAT ancestor.
    ///
    /// The other two ledgers are forwarded unscoped, deliberately:
    ///
    /// - `truncated_classes` is not reported, it is *read* — by
    ///   [`Self::class_ancestry_truncated`], to suppress a false
    ///   `LB0306`/`LB0303` on a shape the walk admits is incomplete. Scoping
    ///   it would re-introduce exactly the false positives finding 1 of the
    ///   earlier production-readiness review removed, for an ancestor this
    ///   file reaches through inference rather than by name.
    /// - `cyclic_class_hits` is attributed to the class the walk actually
    ///   looped back onto, which by construction is rarely the root a file
    ///   names (a `A : B` / `B : A` pair reached from `Root` records `A`/`B`,
    ///   never `Root`) — see [`Self::note_cyclic`]. Scoping it to `roots`
    ///   would discard essentially every `LB0318` rather than narrow it.
    fn absorb_limit_trips(&self, other: &TypeEnv, roots: &HashSet<String>) {
        fn extend(dst: &Mutex<HashSet<String>>, src: &Mutex<HashSet<String>>) {
            let src = src
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            dst.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(src.iter().cloned());
        }
        fn extend_within(
            dst: &Mutex<HashSet<String>>,
            src: &Mutex<HashSet<String>>,
            roots: &HashSet<String>,
        ) {
            let src = src
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            dst.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend(src.iter().filter(|name| roots.contains(*name)).cloned());
        }
        extend_within(&self.depth_limit_hits, &other.depth_limit_hits, roots);
        extend_within(&self.cost_limit_hits, &other.cost_limit_hits, roots);
        extend(&self.truncated_classes, &other.truncated_classes);
        extend(&self.cyclic_class_hits, &other.cyclic_class_hits);
    }

    /// Drain every depth-limit hit (`LB0317`) recorded since the env was
    /// built — root class names only, deduplicated (one pathological class
    /// can trip the guard on every reference within a file's check, not
    /// just once). `crate::check::run` calls this once per file, after both
    /// inference and the checker have finished querying `self`, and turns
    /// each name into one diagnostic. [`Self::take_cost_limit_hits`] is the
    /// cost-budget counterpart — a walk trips at most one of the two
    /// (`limit_exceeded` is a single `Option`), so a root class never
    /// appears in both.
    pub(crate) fn take_depth_limit_hits(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .depth_limit_hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_iter()
        .collect()
    }

    /// Drain every cost-limit hit (`LB0319`, [`crate::codes::CLASS_COST_LIMIT`])
    /// recorded since the env was built — root class names only,
    /// deduplicated, the cost-budget counterpart of
    /// [`Self::take_depth_limit_hits`] with the identical drain-once
    /// contract and call-site shape (production readiness review F1). A
    /// walk that ran past [`MAX_ANCESTRY_RESOLUTIONS`] re-resolutions
    /// without ever recursing anywhere near [`MAX_ANCESTRY_DEPTH`] drains
    /// here, not into `take_depth_limit_hits` — see [`LimitTrip`]'s doc
    /// comment for why conflating the two misleads about the fix.
    pub(crate) fn take_cost_limit_hits(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .cost_limit_hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_iter()
        .collect()
    }

    /// Drain every true-cycle hit recorded since the env was built —
    /// deduplicated, every class the cycle reaches rather than only the name
    /// that happened to be queried (round 6 review M67, the cyclic-class
    /// counterpart of [`Self::take_depth_limit_hits`], with the identical
    /// drain-once contract). `crate::check::report_cyclic_class_hits` turns
    /// each name into one `LB0318` exactly as `report_depth_limit_hits` does
    /// for `LB0317`, drained at the same point in `crate::check::run` and
    /// with the same three attribution tiers.
    pub(crate) fn take_cyclic_class_hits(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .cyclic_class_hits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_iter()
        .collect()
    }

    /// Whether resolving `name`'s ancestor chain has ever tripped either of
    /// [`DiamondGuard`]'s two budgets on this env (`LB0317` or `LB0319`) — a
    /// *durable peek* against [`Self::truncated_classes`], which nothing
    /// ever drains, unlike [`Self::take_depth_limit_hits`]/
    /// [`Self::take_cost_limit_hits`]'s one-shot report queues (round 6
    /// review M13: this used to peek the *drainable* `depth_limit_hits`
    /// ledger directly, so a class truncated while checking one file no
    /// longer read as truncated once `crate::check::run` drained that ledger
    /// into that file's own `LB0317` diagnostics — correct only for the
    /// caller that runs strictly before the drain, wrong for every caller
    /// that might run after). Two callers need this durable answer:
    ///
    /// - A field lookup that just called `class_shape`/`class_shape_bound`
    ///   for `name` and came up empty (`crate::infer`'s `lookup_shape_field`/
    ///   `lookup_ty_field`, `crate::check`'s `check_table_literal`): a class
    ///   whose ancestry was truncated has an admittedly incomplete shape, so
    ///   an absent member there is not provably undefined — `LB0317` alone
    ///   is the honest diagnostic, not `LB0317` *and* a false `LB0306`/
    ///   `LB0303` for a field that exists past the cutoff (production
    ///   readiness review, finding 1). This query can run *after*
    ///   `report_depth_limit_hits` has already drained the report queue for
    ///   an earlier class in the same file, so it cannot share that queue.
    /// - An LSP surface reading through [`crate::defs::Ambient`]'s own
    ///   long-lived env (`Ambient::class_members`/`class_members_bound`),
    ///   which never runs through `check::run` and so never drains
    ///   `depth_limit_hits` or `cost_limit_hits` at all — [`crate::defs::Ambient::class_ancestry_truncated`]
    ///   exposes this same peek so hover/completion/goto-definition/
    ///   signature-help can say so too, instead of silently showing fewer
    ///   members than the class actually declares with the Problems panel
    ///   none the wiser (finding 2). This caller's set would only ever grow
    ///   regardless of which field backed it, since nothing drains an
    ///   `Ambient`'s env either way — splitting the field changes nothing for
    ///   this lifecycle, only for the check-path one above.
    pub(crate) fn class_ancestry_truncated(&self, name: &str) -> bool {
        self.truncated_classes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(name)
    }

    /// Every generic class this env declares, as the [`GenericClass`]
    /// templates a [`Lowerer`] instantiates a reference against — the same
    /// derivation [`collect_generic_classes`]'s discovery pass ends with,
    /// exposed for a caller that already has a *complete* env (this one) and
    /// needs a lowerer that can resolve a **nested** generic reference inside
    /// an argument list (round 3 review F48's [`Self::lower_bound_args`]):
    /// without it, `Box<Pair<number>>` would lower `Pair<number>` as the bare
    /// `Ty::Named("Pair")`, since a fresh [`Lowerer`] otherwise starts with no
    /// generic classes in scope at all.
    fn generic_classes(&self) -> BTreeMap<String, GenericClass> {
        let mut out = BTreeMap::new();
        for (name, def) in &self.classes {
            if def.params.is_empty() {
                continue;
            }
            if let Some(shape) = self.class_shape(name) {
                out.insert(
                    name.clone(),
                    GenericClass {
                        params: def.params.clone(),
                        template: shape,
                    },
                );
            }
        }
        out
    }

    /// Lower a list of LuaCATS type expressions — a generic reference's
    /// `<...>` arguments — against this env's own declared names (classes,
    /// enums, and `aliases`, supplied by the caller since this env does not
    /// itself retain alias bodies). No `---@generic`/class-parameter scope:
    /// an argument names a concrete type, never a template placeholder, so
    /// nothing needs to be in `Lowerer::generics` for this call. Reports no
    /// diagnostics — this is a resolution query over an already-checked
    /// annotation, not a declaration being checked for the first time; an
    /// undeclared name lowers leniently to `Ty::Unknown`, matching how a bare
    /// unresolvable reference already behaves everywhere else in the checker.
    ///
    /// The reference-site half of round 3 review F48: `Ambient::class_members`
    /// resolves a class's shape with every parameter free, which is right for
    /// a bare `Box` but silently wrong for `Box<number>` — this is what lets a
    /// caller (LSP hover/completion) bind the arguments the reference actually
    /// wrote before asking for the shape, instead of the editor answering `T`
    /// where the checker answers `number`.
    pub(crate) fn lower_bound_args(
        &self,
        args: &[luacats::TypeExpr],
        aliases: &BTreeMap<String, AliasTag>,
    ) -> Vec<Ty> {
        let decl = Declared {
            classes: self.classes.keys().cloned().collect(),
            enums: self.enums.keys().cloned().collect(),
            aliases: aliases.clone(),
        };
        let mut lowerer = Lowerer::new(&decl);
        lowerer.generic_classes = self.generic_classes();
        args.iter().map(|arg| lowerer.lower(arg)).collect()
    }

    /// Expand `name` (bound to `args`) into `shape`, folding parents
    /// depth-first, own members overriding.
    ///
    /// `erase_free` decides what a class's own type parameters left
    /// **unbound** by a visit become in the result: `false` (every ordinary
    /// caller — [`Self::class_shape_bound`]) leaves an unbound parameter as
    /// itself (`Box<T>`'s `item` reads `T`, as a bare `Box` reference
    /// means); `true` ([`Self::class_shape_bound_export`] only) substitutes
    /// [`Ty::Unknown`] for it instead, in `bound` itself, at the exact place
    /// this method already computes which parameters a visit leaves free —
    /// see that method's doc comment for why, and for round 5 review N8/N9,
    /// the bug this replaces.
    ///
    /// Two guards, doing two different jobs (round 4 review R1), bundled
    /// into one [`DiamondGuard`] (round 4 review finding 7 — this walk and
    /// [`Self::collect_operators`]'s had been two hand-copied pairs of the
    /// same fields and enter/exit logic; this is their single owner, via
    /// [`DiamondGuard::visit`]):
    ///
    /// - `on_path` guards the *currently-expanding path*: inserted on entry,
    ///   removed on exit, so a true cycle (a class reachable from itself,
    ///   e.g. `---@class A : B` / `---@class B : A`) still terminates, while
    ///   a diamond — the same ancestor reached through two different
    ///   parents — is not dropped just because some *other* branch has it
    ///   on its own path.
    /// - `resolved`/`generation` decide whether a repeat `(name, args)`
    ///   visit — one already expanded to completion earlier in this same
    ///   call, regardless of path — should be skipped outright or
    ///   re-walked. **It is not simply "every repeat is skipped"**: see
    ///   [`DiamondGuard`]'s doc comment for why that collapses a k-level
    ///   diamond back to O(2^k) (round 5 review N1) the moment *any* name in
    ///   the walk has two bindings, and gets the *wrong* winner besides
    ///   (N3/N4/N5) — and for why the fix is a generation counter, not a
    ///   cache-and-always-reapply (measured wrong on
    ///   `a_later_empty_sibling_does_not_clobber_an_earlier_siblings_field_override`,
    ///   round 4 R1's own pinned regression: `Base.x: number`; `A : Base`
    ///   overrides `x: string`; `B : Base` (bare) declares nothing;
    ///   `C : A, B` — an unconditional reapply lets `B`'s bare, uninforming
    ///   repeat of `(Base, [])` stomp `A`'s override *after* it ran).
    ///
    ///   A repeat that does need to re-run reasserts its binding exactly at
    ///   its own listed position — the same code path a fresh visit takes,
    ///   writing directly into the shared `shape` accumulator below — which
    ///   is what lets a later, competing (`is_first_binding: false`) edge
    ///   still overwrite an earlier one's contribution the way its later
    ///   *visit* position demands (last-visited-edge-wins for a diamond
    ///   conflict on fields; see the indexer loop below, and `member_wins`,
    ///   for why a genuinely unrelated sibling parent — `is_first_binding:
    ///   true` — is the opposite rule, first-**listed**-wins, matching
    ///   luals).
    fn collect_class(
        &self,
        name: &str,
        args: &[Ty],
        shape: &mut TableTy,
        guard: &mut DiamondGuard,
        erase_free: bool,
    ) {
        guard.visit(name, args, |guard, is_first_binding| {
            let Some(def) = self.classes.get(name) else {
                return;
            };
            let mut bound: BTreeMap<String, Ty> = def
                .params
                .iter()
                .zip(args)
                .map(|(param, arg)| (param.clone(), arg.clone()))
                .collect();
            // Every declared parameter past `args`' length has nothing to
            // bind it — it survives substitution as itself
            // (`generics::subst_ty` leaves an unmapped name alone) unless
            // `erase_free` asks for it to become `unknown` instead, decided
            // right here, at the one place substitution actually knows which
            // parameters this visit leaves free — not by a later pass that
            // matches on a parameter's *spelling* against the resolved
            // shape, which cannot tell a genuinely-unbound parameter from a
            // bound reference to a real class that happens to share its
            // name (round 4 review R6, and round 5 review N8: a later
            // "collect every free name, then erase every occurrence of it
            // anywhere in the merged shape" pass reintroduced exactly that
            // confusion one level up — an unrelated ancestor's own unbound
            // parameter erased a same-spelled real class reference living
            // in a completely different branch of the same shape).
            if erase_free {
                for param in def.params.iter().skip(args.len()) {
                    bound.insert(param.clone(), Ty::Unknown);
                }
            }
            // This class's OWN contribution goes first — carrier-attached
            // members, then `---@field` declarations, with a declaration
            // *unconditionally* overriding a same-name attachment on THIS
            // SAME class (annotations are authoritative, matrix row
            // method-G) — resolved into a local map, then merged into
            // `shape` **before** any parent is even recursed into. This
            // ordering, not just the merge rule, is what makes "own
            // overrides inherited" hold unconditionally: luals's own
            // mechanism runs a class's own phase 1-3 (fields, then carrier
            // attachments) and locks each via `copyToSearched` *before*
            // phase 4 (extends) ever runs (compiler.lua:342-509), so an
            // ancestor's contribution for a key this class already declared
            // is never even considered. Mirroring that order here means the
            // `is_first_binding`-gated merge below — the same one two
            // *different* classes' contributions go through — already
            // blocks a parent from overwriting this class's own value with
            // no separate carve-out needed: by the time a parent is
            // recursed into, this class's own declarations have already
            // claimed their keys in `shape`.
            //
            // Doing this the other way around — parents first, own second,
            // both still merged through the identical `is_first_binding`
            // gate — was tried and is wrong: a root (or any first-visited)
            // class's own `is_first_binding` is `true` the same as an
            // unrelated sibling's, so its own fields loop would refuse to
            // overwrite what its *own* parent recursion, moments earlier in
            // the very same visit, had just written — breaking the most
            // basic single-inheritance override
            // (`env::tests::temp_probe_single_parent_override`) and the
            // pinned `a_later_empty_sibling_does_not_clobber_an_earlier_
            // siblings_field_override` regression alike. Own-first avoids
            // that: this class's own value is already sitting in `shape`
            // *before* its own parent's `is_first_binding` is even
            // computed, so the parent's later, `is_first_binding`-gated
            // attempt to write the same key is correctly refused as "someone
            // already claimed this" — precedence, not conflation.
            let mut own: BTreeMap<String, FieldTy> = BTreeMap::new();
            for (member, ty) in &def.methods {
                own.insert(member.clone(), crate::generics::subst_field(ty, &bound));
            }
            for (field, ty) in &def.fields {
                own.insert(field.clone(), crate::generics::subst_field(ty, &bound));
            }
            // The cross-ancestor question this same gate also answers: does
            // this class's (already internally resolved) own contribution
            // override what an earlier-processed *different* class already
            // put in `shape`? Two genuinely *unrelated* parents each
            // declaring their own value for this key keep the
            // **first**-listed one — luals parity (production-readiness-
            // assessment-9natxz A1: this codebase used to overwrite
            // unconditionally here, i.e. last-listed-wins, reasoned from its
            // own code comments rather than from luals's actual
            // `compiler.lua:369-375`/`424` behaviour, and was backwards) —
            // while the *same* ancestor reached again with a genuinely
            // different binding (a diamond conflict) still overwrites, i.e.
            // last-**visited**-edge-wins, unaffected by that fix since luals
            // has no generic-class support to disagree with there. Matches
            // `indexer_resolution` below.
            for (member, value) in own {
                if member_wins(
                    shape.fields.get(&member),
                    MemberArrival::AncestorFold { is_first_binding },
                ) {
                    shape.fields.insert(member, value);
                }
            }
            for parent in &def.parents {
                // A parent's arguments are written in *this* class's parameter
                // vocabulary — `---@class Cell<T> : Slot<T>` passes its own `T`
                // through — so they are substituted before they bind the parent's.
                let parent_args: Vec<Ty> = parent
                    .args
                    .iter()
                    .map(|arg| crate::generics::subst_ty(arg, &bound))
                    .collect();
                self.collect_class(&parent.name, &parent_args, shape, guard, erase_free);
            }
            // Both halves of an indexer are substituted, matching
            // `generics::subst_table` — the reference implementation every
            // *direct* generic instantiation goes through (#59, F36).
            //
            // Indexers do **not** follow the field rule above: measured
            // against `develop` (round 5 review N6), two *unrelated* classes
            // each declaring their own same-keyed indexer keep the
            // **first**-listed one (`C2 : P1, P2` with `P1`'s own `[string]:
            // number` and `P2`'s own, unrelated `[string]: string` reads
            // `c["k"]` as `number` — P1's), while a shared generic ancestor
            // reached twice with *different* bindings still keeps the
            // **last** one, exactly like fields
            // (`conflicting_generic_diamond_bindings_agree_between_fields_indexers_and_assignability`,
            // pinned) — because that second case is the *same name* bound
            // two different ways, not two different names. `is_first_binding`
            // is exactly that distinction: `true` only for a class name's
            // first-ever binding in this call, `false` for a binding that
            // follows a *different* one — a fresh fully-unrelated visit
            // (`P1`, `P2`, or `Base`'s own first binding) is always the
            // former; only a genuine repeat-with-conflict (this is the
            // second, third, ... distinct binding of one name) is the
            // latter. So: append (first-listed for this key wins) when this
            // is a name's first binding; overwrite (last processed wins,
            // matching fields) when it is not.
            //
            // Round 5 shipped a plain overwrite-by-key here unconditionally
            // — correct for the shared-ancestor case, but it silently
            // flipped the unrelated-classes case from first- to last-wins,
            // a verdict change against `develop` with no changelog entry
            // (round 5 review N6). The duplicate-`---@class`-declaration
            // seams (`absorb_block`, `merge_file_types`) keep their own,
            // separate first-wins rule for indexers unchanged (round 5
            // review N7) — this loop only decides between *already-merged*
            // per-class indexer lists.
            // Built once for this class node, not re-scanned per indexer
            // (round 6 review M55): `.iter().any()`/`.iter_mut().find()`
            // here cost O(shape.indexers.len()) on every one of this
            // class's own indexers, making a class with K `---@field [K]
            // V` entries cost O(K²) to resolve.
            let mut indexers = IndexerIndex::of(&shape.indexers);
            for (key, value) in &def.indexers {
                let key = crate::generics::subst_ty(key, &bound);
                let value = crate::generics::subst_ty(value, &bound);
                indexers.absorb(
                    &mut shape.indexers,
                    key,
                    value,
                    MemberArrival::AncestorFold { is_first_binding },
                );
            }
        });
    }

    /// The shared shape of [`Self::class_method_names`], [`Self::member_visibility`]
    /// and [`Self::is_subclass`] (round 5 review N12): a guarded walk
    /// of `name` and its ancestor chain, visiting each distinct class name at
    /// most once (a name may be *expanded* more than once — see the
    /// `expanded_at` comment on the depth cut — but `visit` sees it once).
    /// Deliberately **not** [`DiamondGuard`] — none of the three
    /// cares which *type arguments* an edge binds, only whether a name has
    /// been reached before, so the extra machinery `DiamondGuard` needs to
    /// tell "same ancestor, different binding" apart from a true repeat
    /// would answer a question none of these three ask. `visit` sees every
    /// popped name exactly once (even one no `ClassDef` exists for — an
    /// undeclared parent reference is a legitimate value to compare against,
    /// as [`Self::is_subclass`] does) and returns [`std::ops::ControlFlow::Break`]
    /// to stop the walk early with a result, or [`std::ops::ControlFlow::Continue`]
    /// to keep going; a name with no [`ClassDef`] contributes no parents to
    /// walk further, regardless of which `visit` returns for it.
    ///
    /// **This walk and [`Self::collect_class`]' agree on depth, not on
    /// reach** (PR #61 finding 8, correcting an earlier claim here that they
    /// simply agree). Both refuse to go past [`MAX_ANCESTRY_DEPTH`], but the
    /// two refusals have different blast radii: this one skips the over-deep
    /// *branch* and carries on with every sibling still on the stack, while
    /// [`DiamondGuard`]'s `limit_exceeded` is **sticky** and refuses every
    /// remaining sibling of the whole walk too. So `C : DeepChain, Shallow`
    /// gives an empty merged shape (`Shallow` is refused after the chain
    /// trips) while `is_subclass(C, Shallow)` still answers `true`.
    /// `env::tests::is_subclass_does_not_reach_further_than_the_shape_walk_does`
    /// pins the direction the two DO agree on — a single-parent chain, where
    /// there is no sibling for stickiness to reach — and
    /// `env::tests::a_truncated_class_stays_lenient_rather_than_agreeing_with_is_subclass`
    /// pins the other: the disagreement is safe only because every
    /// diagnostic path that could act on it is gated behind
    /// [`Self::class_ancestry_truncated`], which is `true` for exactly these
    /// classes.
    fn walk_ancestor_names<T>(
        &self,
        start: &str,
        mut visit: impl FnMut(&str, Option<&ClassDef>) -> std::ops::ControlFlow<T>,
    ) -> Option<T> {
        let mut stack = vec![(start.to_string(), 0usize)];
        // Name -> the SHALLOWEST depth this walk has expanded that name at.
        // Not a bare set (PR #61 round-8 F1): the depth cut below applies to
        // the node being popped, but what it silently throws away is that
        // node's PARENTS, pushed at `depth + 1`. A node first expanded at
        // `MAX - 1` is itself under the cap, records itself, and pushes
        // parents that are then dropped at `MAX` — so a bare set made the
        // later shallow re-reach of that node a no-op and deleted its whole
        // subtree from the walk. Recording the depth lets a strictly
        // shallower re-reach re-expand: same node, parents now pushed at a
        // depth the cap accepts. Terminates because a name's recorded depth
        // strictly decreases on each re-expansion and is floored at 0, so a
        // name expands at most `MAX_ANCESTRY_DEPTH` times however tangled the
        // graph. Not free on ordinary shapes either — any diamond whose
        // SHORTER path to a shared ancestor is listed second re-expands that
        // ancestor once (`A : B, D` with `B : D` reaches `D` at 2 then at 1)
        // — but one extra expansion per such node is what buys an answer that
        // does not depend on parent order, and the cap keeps the worst case
        // bounded rather than merely unlikely.
        let mut expanded_at: HashMap<String, usize> = HashMap::new();
        // Separate from `expanded_at` so `visit` keeps its documented
        // once-per-name contract: a re-expansion re-pushes parents, it does
        // not re-announce a name the caller has already been given.
        let mut visited = HashSet::new();
        while let Some((name, depth)) = stack.pop() {
            // The same cap `DiamondGuard` refuses to recurse past in
            // `collect_class`/`collect_operators` (round 6 review M12):
            // without it, this walk reaches ancestors `class_shape` itself
            // truncates away, so `is_subclass`/`class_method_names`/
            // `member_visibility` could disagree with the merged shape about
            // how far the chain actually reaches — an upcast `is_subclass`
            // approves that the truncated shape then fails with a spurious
            // "missing" diagnostic, because the field it promised was never
            // actually merged in.
            //
            // Checked BEFORE `expanded_at` (PR #61 finding 4): that map
            // records what this walk has *expanded*, not what it has merely
            // popped, so a name first reached past the cap must leave no
            // trace at any depth — the same
            // name may sit at a perfectly reachable depth on another branch
            // still to be popped, and marking it here poisoned it
            // permanently. Measured at d20cd47: `---@class A : C1, X` with a
            // 199-long C-chain ending at X (X at depth 200 through the
            // chain, depth 1 as A's own second parent) made
            // `is_subclass("A", "X")` `false` for a parent A lists on its own
            // declaration line — and `true` again on swapping the two
            // parents, since the shallow X then popped first.
            if depth >= MAX_ANCESTRY_DEPTH {
                continue;
            }
            match expanded_at.get(&name) {
                // Already expanded from here or nearer the root: everything
                // this pop could push is already on (or through) the stack.
                Some(&previous) if previous <= depth => continue,
                _ => expanded_at.insert(name.clone(), depth),
            };
            let def = self.classes.get(&name);
            if visited.insert(name.clone())
                && let std::ops::ControlFlow::Break(result) = visit(&name, def)
            {
                return Some(result);
            }
            if let Some(def) = def {
                // Reversed: `stack` is a LIFO, so pushing in declared order
                // would pop the *last*-listed parent first —
                // `member_visibility`'s only order-sensitive consumer used to
                // inherit exactly that last-listed-wins bias for two
                // unrelated parents, disagreeing with indexer/operator's
                // first-listed rule for the identical shape (finding 4).
                // Pushing reversed pops the first-listed parent first, giving
                // every order-sensitive caller ordinary left-to-right
                // declaration-order preorder traversal — own name first
                // (already true: `start` is visited before any parent is
                // pushed), then each parent's whole subtree in listed order.
                // `class_method_names` (accumulates a set) and `is_subclass`
                // (`Break` on name match) are unaffected by traversal order
                // either way.
                stack.extend(
                    def.parents
                        .iter()
                        .rev()
                        .map(|p| (p.name.clone(), depth + 1)),
                );
            }
        }
        None
    }

    /// The member names of `name`'s shape that are carrier attachments
    /// (`function Class:method()` et al.) rather than `---@field`
    /// declarations, across the parent chain. These resolve on reads but
    /// carry no table-literal obligation (luals `missing-fields` parity) —
    /// the literal classifiers exclude them from the required set.
    pub(crate) fn class_method_names(&self, name: &str) -> HashSet<String> {
        let mut methods = HashSet::new();
        let mut declared = HashSet::new();
        self.walk_ancestor_names(name, |_, def| {
            if let Some(def) = def {
                methods.extend(def.methods.keys().cloned());
                declared.extend(def.fields.keys().cloned());
            }
            std::ops::ControlFlow::Continue::<()>(())
        });
        &methods - &declared
    }

    /// Every `---@operator <op>` overload in scope for a class, own
    /// declarations first then inherited (depth-first over the parent chain),
    /// in declaration order — the sequence inference scans for the first
    /// overload whose parameter accepts the other operand (#114). Own
    /// operators precede inherited ones so a subclass override wins.
    pub(crate) fn class_operators(&self, name: &str, op: &str) -> Vec<OperatorSig> {
        let mut out: Vec<(String, OperatorSig)> = Vec::new();
        let mut guard = DiamondGuard::default();
        self.collect_operators(name, &[], op, &mut out, &mut guard);
        self.note_limit_trip(&guard, name);
        self.note_cyclic(&guard);
        out.into_iter().map(|(_owner, sig)| sig).collect()
    }

    /// [`Self::collect_class`]'s substitution and [`DiamondGuard`] rules,
    /// for `---@operator` overloads instead of members: `args` binds
    /// `name`'s own type parameters, a parent's arguments are substituted
    /// through that binding before they bind the parent's own, the guard's
    /// `on_path` half admits a shared ancestor reached through two
    /// differently-bound parents on both edges (#59, F37 — before that fix
    /// `collect_operators` performed no substitution at all, unlike its
    /// sibling, so `---@operator add(U): U` on `---@class Base<U>` inherited
    /// by `---@class Sub : Base<number>` left `U` free), and its memo half
    /// skips a redundant `(name, args)` pair already expanded to completion
    /// — the same diamond-collapsing rule `collect_class` applies, since
    /// this walk has the identical shape and the identical exponential
    /// exposure (round 4 review R1; round 5 review N1 for why the guard
    /// itself had to change again).
    ///
    /// `out` carries each entry's owning ancestor name alongside its
    /// substituted signature (stripped by [`Self::class_operators`] before it
    /// returns) so a genuinely competing repeat — [`DiamondGuard::visit`]'s
    /// `is_first_binding: false`, an ancestor reached a second time with a
    /// *different* binding, a diamond conflict — can supersede that
    /// ancestor's earlier contribution rather than merely append beside it.
    /// Before this, a competing repeat's entries only ever joined the list
    /// after the first-visited edge's, so the first-match scan (#114) always
    /// picked the *first*-visited edge's signature — the opposite of
    /// [`Self::collect_class`]'s last-visited-edge-wins for the identical
    /// diamond-conflicting shape on fields/indexers, and undocumented
    /// anywhere (finding 3). Superseding on every non-first-binding visit —
    /// including a *stale* one reasserting a binding identical to what it
    /// already contributed — costs nothing beyond a `retain` a stale replay
    /// would otherwise skip: the entries it removes and re-adds are
    /// value-identical, so the net list is the same either way.
    fn collect_operators(
        &self,
        name: &str,
        args: &[Ty],
        op: &str,
        out: &mut Vec<(String, OperatorSig)>,
        guard: &mut DiamondGuard,
    ) {
        guard.visit(name, args, |guard, is_first_binding| {
            let Some(def) = self.classes.get(name) else {
                return;
            };
            let bound: BTreeMap<String, Ty> = def
                .params
                .iter()
                .zip(args)
                .map(|(param, arg)| (param.clone(), arg.clone()))
                .collect();
            if !is_first_binding {
                // A competing (or stale) repeat of this exact ancestor:
                // whatever it contributed on an earlier visit is no longer
                // current — drop it before this visit's fresh entries go in,
                // so the most-recently-visited binding is the only candidate
                // the first-match scan sees for this ancestor.
                out.retain(|(owner, _)| owner != name);
            }
            if let Some(sigs) = def.operators.get(op) {
                out.extend(sigs.iter().map(|sig| {
                    (
                        name.to_string(),
                        OperatorSig {
                            input: sig
                                .input
                                .as_ref()
                                .map(|ty| crate::generics::subst_ty(ty, &bound)),
                            result: crate::generics::subst_ty(&sig.result, &bound),
                        },
                    )
                }));
            }
            for parent in &def.parents {
                let parent_args: Vec<Ty> = parent
                    .args
                    .iter()
                    .map(|arg| crate::generics::subst_ty(arg, &bound))
                    .collect();
                self.collect_operators(&parent.name, &parent_args, op, out, guard);
            }
        });
    }

    pub(crate) fn enum_member(&self, enum_name: &str, member: &str) -> Option<&Ty> {
        self.enums.get(enum_name)?.members.get(member)
    }

    /// The members of a declared `---@enum`, as `(member name, value type)`
    /// pairs in declaration-stable order — the finite domain the exhaustiveness
    /// check (`LB0315`) enumerates. `None` when `name` is not an enum.
    pub(crate) fn enum_members(&self, name: &str) -> Option<Vec<(String, Ty)>> {
        let def = self.enums.get(name)?;
        Some(
            def.members
                .iter()
                .map(|(member, ty)| (member.clone(), ty.clone()))
                .collect(),
        )
    }

    /// Resolve a [`Ty::Named`] reference to its structural type: a class
    /// becomes its table shape, an enum the union of its member values.
    pub(crate) fn resolve_named(&self, name: &str) -> Option<Ty> {
        if let Some(shape) = self.class_shape(name) {
            return Some(Ty::Table(Box::new(shape)));
        }
        self.enums.get(name).map(|e| e.value_union.clone())
    }

    /// [`Self::resolve_named`], except a class's own type parameters left
    /// unbound by this reference substitute to [`Ty::Unknown`] instead of
    /// surviving as their literal name — the [`Self::class_shape_bound_export`]
    /// rule (#56, finding 5 of the production readiness review), reused here
    /// for every other reference-consuming site that can put a member's type
    /// in front of a user: a field read on an annotated value, an `ipairs`/
    /// `pairs` element type, a table-literal's expected shape. `T` is not a
    /// value a Lua reference can name or produce, so a bare `---@class Sub :
    /// Base` (Base generic, left unbound) should read `Sub`'s inherited
    /// member the same way a bare `Box` reference already does (#84) and the
    /// same way it crosses `require` (#56) — not literally as `T`.
    ///
    /// [`Self::resolve_named`] itself must keep the raw (non-erasing)
    /// resolution: [`crate::infer::Infer::reify_export`] diffs its own
    /// erased result against it to decide whether erasure changed anything,
    /// and that comparison needs an unerased baseline to diff against.
    pub(crate) fn resolve_named_bound(&self, name: &str) -> Option<Ty> {
        if let Some(shape) = self.class_shape_bound_export(name, &[]) {
            return Some(Ty::Table(Box::new(shape)));
        }
        self.enums.get(name).map(|e| e.value_union.clone())
    }

    pub(crate) fn function(&self, name: &str) -> Option<&FunctionTy> {
        self.functions.get(name)
    }

    /// The ambient type of a global value (stdlib module table or scalar
    /// global) declared by a definition package. `None` for names the
    /// active definition packages do not declare.
    pub(crate) fn global_type(&self, name: &str) -> Option<&Ty> {
        self.global_types.get(name)
    }

    pub(crate) fn typed_local(&self, target: Target) -> Option<&[Ty]> {
        self.typed_locals.get(&target).map(Vec::as_slice)
    }

    /// The source span of the `---@type` annotation on a `local` statement.
    pub(crate) fn typed_local_span(&self, target: Target) -> Option<std::ops::Range<usize>> {
        self.typed_local_spans.get(&target).cloned()
    }

    pub(crate) fn fn_sig(&self, target: Target) -> Option<&FunctionTy> {
        self.fn_sigs.get(&target)
    }

    /// The `---@class`/`---@struct` name bound to a statement, if any.
    pub(crate) fn declared_target(&self, target: Target) -> Option<&str> {
        self.declared_targets.get(&target).map(String::as_str)
    }

    /// The `---@class` tag span on the carrier statement `target` — the
    /// anchor a `: Interface` conformance error points at (#107).
    pub(crate) fn class_tag_span(&self, target: Target) -> Option<std::ops::Range<usize>> {
        self.class_tag_spans.get(&target).cloned()
    }

    /// The in-file `---@class` tag span of `name`, when it is declared in the
    /// file under check — the "declared here" secondary label for a parent a
    /// conformance error names (#107). `None` for ambient/defs classes.
    pub(crate) fn class_decl_span(&self, name: &str) -> Option<std::ops::Range<usize>> {
        self.class_decl_spans.get(name).cloned()
    }

    /// [`Self::class_decl_span`]'s cross-file counterpart (round 3 review
    /// F71): the declaring file plus span for a class declared in a
    /// *different* project file than the one currently checking, or `None`
    /// when `name` is not a class this env has cross-file span data for
    /// (either genuinely undeclared, or declared in the current file — use
    /// [`Self::class_decl_span`] for that case).
    pub(crate) fn cross_file_class_decl_span(
        &self,
        name: &str,
    ) -> Option<(String, std::ops::Range<usize>)> {
        self.cross_file_class_decl_spans.get(name).cloned()
    }

    /// The declared parents of a `---@class`, in declaration order — each
    /// with the type arguments this declaration binds to it.
    pub(crate) fn class_parents(&self, name: &str) -> Option<&[ParentRef]> {
        self.classes.get(name).map(|def| def.parents.as_slice())
    }

    /// Whether `name` is a LuaCATS `---@class` (in-file, def-package, or
    /// cross-package). The `undefined-field` read rule (#90) fires only for
    /// real classes.
    pub(crate) fn is_class(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// Whether `name` declares `field` as one of its *own* `---@field`s (as
    /// opposed to inheriting it) — the set a conformance obligation excludes,
    /// since a re-declared member is governed by the class's own declaration
    /// rather than the parent's (#107).
    pub(crate) fn class_declares_own(&self, name: &str, field: &str) -> bool {
        self.classes
            .get(name)
            .is_some_and(|def| def.fields.contains_key(field))
    }

    /// The visibility of `member` as seen through `class`, plus the class that
    /// *owns* (declares) the restriction — the luals `invisible` lookup (#115).
    /// `None` when the member is public (no restriction anywhere the resolution
    /// reaches). Resolution walks own declarations first, then parents: the
    /// nearest declaration wins, so a subclass that re-declares an inherited
    /// restricted member as a plain `---@field` (recorded as *not* in
    /// `visibility`, but present in `fields`) overrides it back to public.
    /// Two *unrelated* parents at the same depth (`---@class C : P1, P2`,
    /// both declaring `member`) resolve **first-listed-wins** —
    /// `walk_ancestor_names`'s traversal order, matching indexer/operator's
    /// first-listed rule for the identical shape (finding 4; before this it
    /// was last-listed, via the same LIFO stack read in the opposite
    /// direction, a third mechanism disagreeing with both).
    pub(crate) fn member_visibility(
        &self,
        class: &str,
        member: &str,
    ) -> Option<(FieldScope, String)> {
        self.walk_ancestor_names(class, |name, def| {
            let Some(def) = def else {
                return std::ops::ControlFlow::Continue(());
            };
            if let Some(scope) = def.visibility.get(member) {
                return std::ops::ControlFlow::Break(Some((*scope, name.to_string())));
            }
            // A plain (public) re-declaration of the member here shadows any
            // parent restriction — resolution stops, member is public.
            if def.fields.contains_key(member) || def.methods.contains_key(member) {
                return std::ops::ControlFlow::Break(None);
            }
            std::ops::ControlFlow::Continue(())
        })
        .flatten()
    }

    /// Whether `class` is `ancestor` or transitively extends it (`---@class
    /// Child : ancestor`) — the `protected` reachability test (#115).
    pub(crate) fn is_subclass(&self, class: &str, ancestor: &str) -> bool {
        self.walk_ancestor_names(class, |name, _def| {
            if name == ancestor {
                std::ops::ControlFlow::Break(())
            } else {
                std::ops::ControlFlow::Continue(())
            }
        })
        .is_some()
    }

    /// Whether `class` is declared by *this* file's own annotations (the
    /// `package`-visibility test — a `package` member is reachable only in the
    /// file that declares its owner class, #115).
    pub(crate) fn declares_class_locally(&self, class: &str) -> bool {
        self.local_classes.contains(class)
    }

    /// The `---@cast` overrides attached to a statement, if any.
    pub(crate) fn casts_at(&self, target: Target) -> Option<&[CastEntry]> {
        self.casts.get(&target).map(Vec::as_slice)
    }

    /// The inline `--[[@as T]]` cast anchored to an expression ending at
    /// `end_offset`, if any.
    pub(crate) fn as_cast_at(&self, end_offset: usize) -> Option<&Ty> {
        self.as_casts.get(&end_offset)
    }
}

/// Lower a function block's `@param`/`@return`/`@vararg` tags into `func`,
/// returning the non-vararg `@param` tags as [`ParamTy`]s for reconciliation
/// against the AST parameter list ([`reconcile_params`]).
fn lower_signature_tags(
    func: &mut FunctionTy,
    params: &[&ParamTag],
    returns: &[&ReturnTag],
    varargs_tags: &[&VarargTag],
    lowerer: &mut Lowerer<'_>,
) -> Vec<ParamTy> {
    // Lower every tag up front (unknown type names must be reported even
    // for tags that end up unbound); non-vararg tags are *reconciled by
    // name* against the AST parameter list below.
    let mut tag_params: Vec<ParamTy> = Vec::new();
    for param in params {
        let ty = lowerer.lower(&param.ty);
        if param.vararg {
            func.varargs = Some(ty);
        } else {
            tag_params.push(ParamTy {
                name: param.name.clone(),
                ty,
                optional: param.optional,
            });
        }
    }
    // Legacy `---@vararg Type`. luals precedent (see the comment
    // in `absorb_block`): a `---@vararg` and a `---@param ... Type` on
    // the same block both bind to the AST `...` node and their types are
    // merged, so union rather than overwrite — this covers both a
    // `---@vararg` next to a `---@param ...` and several `---@vararg`
    // tags on one block (each is one more bound doc in luals).
    for v in varargs_tags {
        let ty = lowerer.lower(&v.ty);
        func.varargs = Some(match func.varargs.take() {
            Some(existing) => Ty::union(vec![existing, ty]),
            None => ty,
        });
    }
    func.has_return_annotation = !returns.is_empty();
    for tag in returns {
        for it in &tag.items {
            if it.vararg {
                func.returns_vararg = true;
            }
            func.returns.push(lowerer.lower(&it.ty));
        }
    }
    tag_params
}

/// Extract the callable's registered name (dotted, `None` for an anonymous or
/// method target) and its AST parameter list from the statement it annotates.
/// `None` when the target is not a function definition at all — there is then
/// nothing to attach a signature to.
fn resolve_callable_target(stmt: &Stmt) -> Option<(Option<String>, Option<lua::ast::ParamList>)> {
    match stmt {
        Stmt::LocalFunction(f) => Some((f.name().map(|t| t.text().to_string()), f.param_list())),
        Stmt::FunctionDecl(f) => {
            let name = f.name().and_then(|n| {
                // Methods (`function M:m()`) have an implicit `self`
                // parameter — TODO(P1): resolve method calls; skipped
                // from the callable map for now.
                if n.is_method() {
                    None
                } else {
                    let joined: Vec<String> = n.segments().map(|s| s.text().to_string()).collect();
                    Some(joined.join("."))
                }
            });
            Some((name, f.param_list()))
        }
        Stmt::Local(l) => {
            let value = l.values().and_then(|v| v.exprs().next());
            let Some(Expr::Function(f)) = value else {
                return None;
            };
            Some((
                l.names()
                    .next()
                    .and_then(|n| n.name())
                    .map(|t| t.text().to_string()),
                f.param_list(),
            ))
        }
        // `return function(…) end` — a module whose entire export *is* a
        // function, the `direct` row of #46's table. The doc block above the
        // `return` binds to the function value exactly as it does above a
        // `local f = function(…) end`, so a single-function module can carry a
        // signature at all; without this its `---@param`s bind to nothing and
        // the module is unchecked in its own file as well as in every
        // consumer. No name to register under — the function is reached only
        // through `require`, never by a dotted name in this file.
        Stmt::Return(ret) => {
            let value = ret.exprs().and_then(|v| v.exprs().next());
            let Some(Expr::Function(f)) = value else {
                return None;
            };
            Some((None, f.param_list()))
        }
        // `Carrier.m = function(…) end` / `g = function(…) end` — the
        // assignment spelling of a function definition. luals binds a doc
        // block's `---@param`/`---@return`/`---@deprecated` to the function
        // value on the right of a `setfield`/`setglobal` exactly as it does
        // for `function Carrier.m()`, so the same signature attaches here
        // (#38). Single-target only, matching [`visibility_carrier_member`]:
        // `a, b = f, g` has no single function to attach to.
        Stmt::Assign(assign) => {
            let target_list = assign.targets()?;
            let mut targets = target_list.exprs();
            let target = targets.next()?;
            if targets.next().is_some() {
                return None;
            }
            let Some(Expr::Function(f)) = assign.values().and_then(|v| v.exprs().next()) else {
                return None;
            };
            Some((assign_target_name(&target), f.param_list()))
        }
        _ => None,
    }
}

/// The dotted name an assignment target registers a function under —
/// `f` for `f = …`, `M.helper` for `M.helper = …`. `None` for a computed
/// (`t[k]`) or otherwise unnameable target, which attaches a signature to the
/// statement without publishing a by-name callable.
fn assign_target_name(target: &Expr) -> Option<String> {
    match target {
        Expr::Name(name) => Some(name.name()?.text().to_string()),
        Expr::Field(field) => {
            let base = assign_target_name(&field.base()?)?;
            Some(format!("{base}.{}", field.field_name()?.text()))
        }
        _ => None,
    }
}

/// Reconcile the lowered `@param` tags against the target's real AST parameter
/// list, writing the final ordered parameters (and any vararg ceiling) onto
/// `func`.
fn reconcile_params(
    func: &mut FunctionTy,
    tag_params: Vec<ParamTy>,
    param_list: Option<lua::ast::ParamList>,
    is_method: bool,
) {
    // Reconcile with the real parameter list: `@param` tags bind to the
    // parameter of the *same name* (names are mandatory in the tag
    // syntax), so a partially-annotated function never misassociates a
    // tag with the wrong position. Unannotated parameters become
    // optional `unknown` (permissive — partial annotation must not
    // manufacture arity errors), and an unannotated `...` still lifts
    // the arity ceiling.
    //
    // TODO(P2): tags naming no parameter are silently unbound today
    // (LuaLS warns); surface a diagnostic for them.
    if let Some(list) = param_list {
        let mut ast_names: Vec<String> = Vec::new();
        let mut ast_vararg = false;
        for p in list.params() {
            if p.is_vararg() {
                ast_vararg = true;
            } else if let Some(name) = p.name() {
                ast_names.push(name.text().to_string());
            }
        }
        let mut used = vec![false; tag_params.len()];
        for name in &ast_names {
            let tag = tag_params
                .iter()
                .enumerate()
                .find(|(i, tag)| !used[*i] && &tag.name == name);
            match tag {
                Some((i, tag)) => {
                    func.params.push(tag.clone());
                    used[i] = true;
                }
                None => func.params.push(ParamTy {
                    name: name.clone(),
                    ty: Ty::Unknown,
                    optional: true,
                }),
            }
        }
        // A `:` method's implicit `self` is absent from the AST parameter
        // list; keep an explicit `---@param self T` tag so inference can
        // honor it (the standard-LuaCATS `self` fallback).
        if is_method
            && let Some(pos) = tag_params.iter().position(|p| p.name == "self")
            && !used[pos]
        {
            func.params.insert(0, tag_params[pos].clone());
        }
        if ast_vararg && func.varargs.is_none() {
            func.varargs = Some(Ty::Unknown);
        }
    } else {
        // No AST parameter list to reconcile against (malformed source):
        // fall back to tag order.
        func.params = tag_params;
    }
}

/// Merge `from` into `into` keeping the value already present on a key
/// collision (first-wins) — the `workspace.library` precedence for merging
/// definition-package layers (#108). Contrast [`BTreeMap::append`], which is
/// last-wins.
fn merge_keep_first<V>(into: &mut BTreeMap<String, V>, from: BTreeMap<String, V>) {
    for (key, value) in from {
        into.entry(key).or_insert(value);
    }
}

/// The generic type-variable names in scope for one annotation block:
/// `---@generic T` parameters plus any `---@class Name<T>`'s own `<T>`
/// params. Both lower to `Ty::Named` placeholders rather than tripping
/// LB0305 (#84).
///
/// `pub(crate)` because `check::duplicate_doc_fields_with_ambient` lowers
/// the same indexer keys and must use the SAME notion of "in-scope generic
/// name" — two copies of this rule made LB0311 fire or miss per seam
/// (round 8 review F21).
pub(crate) fn block_generics(item: &luacats::AnnotatedItem) -> HashSet<String> {
    let mut names = HashSet::new();
    for tag in &item.block.tags {
        match tag {
            Tag::Generic(g) => names.extend(g.params.iter().map(|p| p.name.clone())),
            Tag::Class(c) => names.extend(c.params.iter().cloned()),
            _ => {}
        }
    }
    names
}

/// Build the generic `---@class Name<T>` templates the lowerer instantiates at
/// reference sites: every class with parameters, its template the class's
/// FULLY RESOLVED shape — including inherited members, monomorphised through
/// the parent chain exactly as [`TypeEnv::class_shape`] already does for
/// every other consumer of it (#59).
///
/// Corrected (round 3 review 2(a)): previously this built each class's
/// template from its own `---@field`s alone, one declaration at a time —
/// parent fields were never folded in ("inherited generic fields are
/// deliberately shallow", the doc comment used to say, citing #84 as if that
/// were a settled design rather than a gap). `---@type Sub<string>` where
/// `Sub : Base<T>` resolved `slot` (declared on `Base`, not `Sub`) as
/// `unknown` instead of `string` — a real class's own reference-site
/// behaviour disagreeing with the same class read through a plain
/// `---@type Sub` binding, which correctly inherits via `class_shape`.
/// `docs/03-reference/02-limitations.md` documents the old behaviour as
/// deliberate; it is not — see the fix's report for the corrected wording.
///
/// The fix: build a throwaway, diagnostics-discarded `TypeEnv` via the SAME
/// `absorb_block` machinery [`TypeEnv::build_from_items`] uses for real,
/// seeded with the same ambient. Once every class in the file is known to
/// it — same two-pass shape [`TypeEnv::build_from_items`] already needs so a
/// reference resolves regardless of declaration order — `class_shape` (already
/// inheritance-, substitution- and cycle-aware) answers the "what does this
/// class's shape monomorphise to, own params free" question directly, so a
/// generic reference's ancestors are in scope on the same terms a same-file
/// plain reference's already are. The cost is one extra walk of the file's
/// annotations per build; correctness here was worth more than that walk —
/// **when there is a generic class to build a template for at all.**
///
/// Round 5 review N27: that discovery walk (and the `class_shape` resolution
/// after it) ran unconditionally, before the `def.params.is_empty()` filter
/// that discards everything it built for a plain class — so a project with
/// zero generic classes anywhere still paid to re-absorb every file's
/// annotations a second time, in full, on every `check`. Measured: 110ms at
/// N=50 files, 6159ms at N=500 (`luabox check` CPU, generic-free corpus).
/// The cheap fix: a project or file with no generic class in scope has
/// nothing this function could ever return, so check for one — a single
/// pass over each item's already-harvested tags, no lowering, no absorb —
/// before paying for the discovery env at all.
/// Add to `out` every one of `names` that `expr`, or anything nested inside
/// it, references — bare or instantiated. Recurses through every
/// `TypeExprKind` a `Named` node can be nested inside (optional, array,
/// union, tuple, table, function signature, parenthesised) so a reference
/// buried inside `(Box<T>)[]` or `{ x: Box }` is still found; literal/error
/// leaves carry no nested type expressions and contribute nothing.
///
/// Collects rather than answering a bool (PR #61 finding 3, which needs the
/// *identity* of the referenced roots, not merely that there was one). A
/// matching `Named` node still has its own arguments walked — `Box<Pair<T>>`
/// references both `Box` and `Pair`, and the old short-circuiting `||` never
/// had to say so.
fn collect_type_expr_references(
    expr: &luacats::TypeExpr,
    names: &HashSet<String>,
    out: &mut HashSet<String>,
) {
    match &expr.kind {
        TypeExprKind::Named { name, args } => {
            if names.contains(name) {
                out.insert(name.clone());
            }
            for arg in args {
                collect_type_expr_references(arg, names, out);
            }
        }
        TypeExprKind::Optional(inner) | TypeExprKind::Array(inner) | TypeExprKind::Paren(inner) => {
            collect_type_expr_references(inner, names, out);
        }
        TypeExprKind::Union(items) | TypeExprKind::Tuple(items) => {
            for item in items {
                collect_type_expr_references(item, names, out);
            }
        }
        TypeExprKind::Table(fields) => {
            for field in fields {
                match field {
                    luacats::TableField::Named { ty, .. } => {
                        collect_type_expr_references(ty, names, out);
                    }
                    luacats::TableField::Indexer { key, value } => {
                        collect_type_expr_references(key, names, out);
                        collect_type_expr_references(value, names, out);
                    }
                }
            }
        }
        TypeExprKind::Fun { params, returns } => {
            for param in params {
                if let Some(ty) = param.ty.as_ref() {
                    collect_type_expr_references(ty, names, out);
                }
            }
            for ret in returns {
                collect_type_expr_references(&ret.ty, names, out);
            }
        }
        TypeExprKind::StringLit(_)
        | TypeExprKind::NumberLit(_)
        | TypeExprKind::BoolLit(_)
        | TypeExprKind::Backtick(_)
        | TypeExprKind::Error => {}
    }
}

/// Add to `out` every one of `names` that any type expression `tag` carries
/// references — exhaustive over every [`Tag`] variant so a new tag kind that
/// grows a `TypeExpr` field later must be added here explicitly rather than
/// silently falling through a wildcard arm and under-triggering the scan it
/// backs (round 6 review M56). The collecting counterpart of
/// [`collect_type_expr_references`], for the same reason (PR #61 finding 3).
///
/// Exhaustive over the variants is not the same as exhaustive over the
/// **fields** (PR #61 round-8 F5): the match arms are what the compiler
/// checks, and a payload struct carrying two `TypeExpr`s can lose one
/// silently. There are fourteen such fields across the payloads —
/// `ClassTag::parents`, `FieldTag::key` (when `FieldKey::Indexer`),
/// `FieldTag::ty`, `ParamTag::ty`, `ReturnItem::ty`, `TypeTag::types`,
/// `AliasTag::ty`, `AliasMember::ty`, `GenericParam::constraint`,
/// `OverloadTag::ty`, `CastOp::ty`, `OperatorTag::input`,
/// `OperatorTag::result`, `VarargTag::ty` — and every one is walked below.
/// `EnumTag`/`MetaTag`/`SimpleTag`/`UnknownTag` carry no type expression at
/// all.
fn collect_tag_references(tag: &Tag, names: &HashSet<String>, out: &mut HashSet<String>) {
    let mut collect = |ty: &luacats::TypeExpr| collect_type_expr_references(ty, names, out);
    match tag {
        Tag::Class(c) => c.parents.iter().for_each(&mut collect),
        Tag::Field(f) => {
            // The KEY as well as the value (PR #61 round-8 F5): a
            // `---@field [Box<number>] string` carries a whole `TypeExpr` in
            // `FieldKey::Indexer` that is NOT reachable from `f.ty`, so a
            // file whose only generic reference was written there built no
            // template and never ran the LB0313 arity check. `FieldKey::Name`
            // is a plain `String` and references nothing.
            if let luacats::FieldKey::Indexer(key) = &f.key {
                collect(key);
            }
            collect(&f.ty);
        }
        Tag::Param(p) => collect(&p.ty),
        Tag::Return(r) => r.items.iter().for_each(|item| collect(&item.ty)),
        Tag::Type(t) => t.types.iter().for_each(&mut collect),
        Tag::Alias(a) => {
            if let Some(ty) = a.ty.as_ref() {
                collect(ty);
            }
            a.members.iter().for_each(|m| collect(&m.ty));
        }
        Tag::Generic(g) => g.params.iter().for_each(|p| {
            if let Some(ty) = p.constraint.as_ref() {
                collect(ty);
            }
        }),
        Tag::Overload(o) => collect(&o.ty),
        Tag::Cast(c) => c.ops.iter().for_each(|op| collect(&op.ty)),
        Tag::Operator(o) => {
            if let Some(ty) = o.input.as_ref() {
                collect(ty);
            }
            collect(&o.result);
        }
        Tag::Vararg(v) => collect(&v.ty),
        Tag::Enum(_)
        | Tag::Meta(_)
        | Tag::See(_)
        | Tag::Deprecated(_)
        | Tag::Nodiscard(_)
        | Tag::Async(_)
        | Tag::Diagnostic(_)
        | Tag::Version(_)
        | Tag::Source(_)
        | Tag::Package(_)
        | Tag::Private(_)
        | Tag::Protected(_)
        | Tag::Unknown(_) => {}
    }
}

fn collect_generic_classes(
    items: &[luacats::AnnotatedItem],
    inline_as: &[luacats::InlineAs],
    root: &SyntaxNode,
    ambient: Option<&crate::defs::Ambient>,
    decl: &Declared,
    env: &TypeEnv,
) -> BTreeMap<String, GenericClass> {
    // Every name a `Named` reference site would need `generic_classes` to
    // resolve — file-declared generic classes plus ambient ones. Cheap to
    // gather (no absorb, no lowering): `ambient.env.classes` is already an
    // in-memory map, and the file-declared half is the same single pass
    // over already-harvested tags the old `file_has_generics` bool used,
    // just keeping the names instead of collapsing them to a bool.
    let mut generic_names: HashSet<String> = HashSet::new();
    for item in items {
        for tag in &item.block.tags {
            if let Tag::Class(c) = tag
                && !c.params.is_empty()
            {
                generic_names.insert(c.name.clone());
            }
        }
    }
    if let Some(ambient) = ambient {
        generic_names.extend(
            ambient
                .env
                .classes
                .iter()
                .filter(|(_, def)| !def.params.is_empty())
                .map(|(name, _)| name.clone()),
        );
    }
    // Round 6 review M56: the old guard asked "does a generic class exist
    // ANYWHERE" (file or ambient) — true the moment ambient ships even ONE
    // generic class, which re-arms the full discovery-and-resolve pass
    // below for every file in the project, whether or not that file ever
    // writes the `Name<...>`/bare-`Name` reference syntax that could
    // possibly need it. This scan asks the file-local question instead:
    // does THIS file's own annotations reference one of `generic_names` at
    // all (bare or with `<Args>`) — the exact condition
    // `Lowerer::lower_named` checks (`self.generic_classes.get(name)`) when
    // it decides whether a template exists for a name it is lowering. A
    // file that never writes any of these names cannot possibly read a
    // different answer from `generic_classes` than an empty map would give,
    // so skipping the discovery pass for it changes nothing observable.
    //
    // Inline `--[[@as T]]` casts are scanned alongside the doc-block tags
    // (PR #61 finding 2): `harvest_inline_as` is disjoint from
    // `item.block.tags`, but its casts are lowered by the SAME lowerer whose
    // `generic_classes` this scan gates, so a file whose only generic
    // reference is an inline cast still reads `generic_classes` and still
    // needs the template. Measured at d20cd47 on a file whose sole
    // `Box<number>` reference was an inline cast: a false `LB0300 ... found
    // `unknown`` in Strict, a false NEGATIVE in Warn, and the answer flipped
    // the moment any unrelated tag elsewhere in the file happened to name
    // `Box`.
    //
    // The names themselves are kept, not collapsed to a bool, because the
    // trip-forwarding below is scoped to exactly this set (finding 3).
    let mut referenced: HashSet<String> = HashSet::new();
    if !generic_names.is_empty() {
        // Alias names are scanned for alongside the generic ones (PR #61
        // round-8 F6). A generic class can be reached through an alias BODY
        // that this file never writes: `---@alias Boxed Box<number>` shipped
        // by the ambient, `---@type Boxed` written here, and the file names
        // neither `Box` nor anything else in `generic_names`. A
        // FILE-declared alias was covered by accident — its own `Tag::Alias`
        // is in `items`, so the body got scanned like any other tag — which
        // is precisely why the gap was invisible. Measured at 825c61c:
        // `expand_alias` lowered the ambient alias's body against an empty
        // template map and produced an unsubstituted `Ty::Named("Box")`,
        // whose `value` stayed the free `T`.
        //
        // Cost is the same order as `generic_names` itself — one clone of the
        // already-in-memory alias key set per file build, no lowering and no
        // absorb — so this does not walk back M56's saving: what M56 bought
        // was skipping the discovery-and-resolve pass, and a file that names
        // no generic and no alias still skips it.
        let mut probe = generic_names.clone();
        probe.extend(decl.aliases.keys().cloned());
        let mut hits: HashSet<String> = HashSet::new();
        for item in items {
            for tag in &item.block.tags {
                collect_tag_references(tag, &probe, &mut hits);
            }
        }
        for cast in inline_as {
            collect_type_expr_references(&cast.ty, &probe, &mut hits);
        }
        // Follow every referenced alias into its body, transitively — one
        // alias may name another (`Outer` -> `Inner` -> `Box<number>`), and
        // `expand_alias` follows the whole chain, so the scan deciding
        // whether a template exists for what it lands on must too. `expanded`
        // bounds each alias to a single expansion, so a cyclic ambient alias
        // terminates here rather than spinning.
        let mut queue: Vec<String> = hits
            .iter()
            .filter(|name| decl.aliases.contains_key(*name))
            .cloned()
            .collect();
        let mut expanded: HashSet<String> = queue.iter().cloned().collect();
        while let Some(name) = queue.pop() {
            let Some(alias) = decl.aliases.get(&name) else {
                continue;
            };
            let mut body: HashSet<String> = HashSet::new();
            if let Some(ty) = alias.ty.as_ref() {
                collect_type_expr_references(ty, &probe, &mut body);
            }
            for member in &alias.members {
                collect_type_expr_references(&member.ty, &probe, &mut body);
            }
            for found in body {
                if decl.aliases.contains_key(&found) && expanded.insert(found.clone()) {
                    queue.push(found.clone());
                }
                hits.insert(found);
            }
        }
        // Back down to generic classes alone: `referenced` scopes the
        // trip-forwarding at the end of this function (finding 3), and an
        // alias name is not a class whose budget could have tripped.
        referenced = hits
            .into_iter()
            .filter(|name| generic_names.contains(name))
            .collect();
    }
    if referenced.is_empty() {
        return BTreeMap::new();
    }

    let mut discovery = TypeEnv::default();
    if let Some(ambient) = ambient {
        discovery.classes = ambient.env.classes.clone();
        discovery.enums = ambient.env.enums.clone();
    }
    let mut discovery_lowerer = Lowerer::new(decl);
    for item in items {
        discovery_lowerer.generics = block_generics(item);
        discovery.absorb_block(item, &mut discovery_lowerer, root);
    }

    let mut out: BTreeMap<String, GenericClass> = BTreeMap::new();
    for (name, def) in &discovery.classes {
        if def.params.is_empty() {
            continue; // only a generic class needs a reference-site template
        }
        if let Some(shape) = discovery.class_shape(name) {
            out.insert(
                name.clone(),
                GenericClass {
                    params: def.params.clone(),
                    template: shape,
                },
            );
        }
    }
    // R1 fix (production readiness measurement): forward any budget trip
    // `discovery`'s own walk recorded onto the real env's ledgers before
    // `discovery` is dropped — see `TypeEnv::absorb_limit_trips`'s doc for
    // why this is the only place that can happen, and why the reportable
    // half is scoped to `referenced` (PR #61 finding 3).
    env.absorb_limit_trips(&discovery, &referenced);
    out
}

/// The lengths of a [`Lowerer`]'s diagnostic buffers before a lowering that is
/// performed only to *build* a type, so anything it records can be discarded.
struct QuietMark {
    unknown_names: usize,
    arity_errors: usize,
    cyclic_aliases: usize,
}

impl QuietMark {
    fn of(lowerer: &Lowerer<'_>) -> Self {
        QuietMark {
            unknown_names: lowerer.unknown_names.len(),
            arity_errors: lowerer.arity_errors.len(),
            cyclic_aliases: lowerer.cyclic_aliases.len(),
        }
    }

    fn rollback(self, lowerer: &mut Lowerer<'_>) {
        lowerer.unknown_names.truncate(self.unknown_names);
        lowerer.arity_errors.truncate(self.arity_errors);
        lowerer.cyclic_aliases.truncate(self.cyclic_aliases);
    }
}

/// The positional substitution one declaration's members need before they
/// merge into the canonical class, or `None` when nothing needs renaming —
/// the declaration names no parameters of its own, or spells them exactly as
/// the canonical list does (the common case).
///
/// This is the **single owner** of the duplicate-declaration unification rule
/// (#59): every seam that folds a re-declaration's members into the canonical
/// class — the instantiation templates ([`collect_generic_classes`]), the
/// in-file member fold ([`TypeEnv::absorb_block`]), and the workspace-global
/// fold ([`TypeEnv::merge_file_types`]) — obtains its substitution here.
/// The #46 family happened because the rule lived as three hand-rolled
/// copies of guard + rename; a seam added later must call this, not re-derive
/// it.
fn class_param_unification(own: &[String], canonical: &[String]) -> Option<BTreeMap<String, Ty>> {
    (!own.is_empty() && own != canonical).then(|| positional_rename(own, canonical))
}

/// The substitution that carries one declaration's type-variable names onto
/// the canonical ones, matched by **position**: `own[i]` and `canonical[i]`
/// are one type variable however the two declarations spell it.
///
/// A surplus parameter (the declaration names more than the canonical list has
/// slots for) has no canonical variable to become, so it maps to `unknown` —
/// the same leniency a bare generic reference gets for the arguments it omits,
/// rather than a dangling placeholder no instantiation could ever substitute.
///
/// Implementation detail of [`class_param_unification`] — merge seams call
/// that, never this directly, so the "when" and the "what" of the rule stay
/// in one place.
fn positional_rename(own: &[String], canonical: &[String]) -> BTreeMap<String, Ty> {
    own.iter()
        .enumerate()
        .map(|(i, name)| {
            let to = canonical
                .get(i)
                .map_or(Ty::Unknown, |c| Ty::Named(c.clone()));
            (name.clone(), to)
        })
        .collect()
}

/// Lower one `---@cast` tag's operation list.
fn lower_cast(tag: &luacats::CastTag, lowerer: &mut Lowerer<'_>) -> CastEntry {
    CastEntry {
        var: tag.var.clone(),
        ops: tag
            .ops
            .iter()
            .map(|op| (op.kind, lowerer.lower(&op.ty)))
            .collect(),
    }
}

/// The scope a standalone `---@private` / `---@protected` / `---@package` tag
/// declares, or `None` for any other tag (#115).
fn standalone_scope(tag: &Tag) -> Option<FieldScope> {
    match tag {
        Tag::Private(_) => Some(FieldScope::Private),
        Tag::Protected(_) => Some(FieldScope::Protected),
        Tag::Package(_) => Some(FieldScope::Package),
        _ => None,
    }
}

/// Map every variable that carries a `---@class` to that class's name, so a
/// method attached to it (`function M:m()`) can be traced back to the class.
///
/// Two spellings reach a class, and they are **not** equal in rank:
///
/// 1. **Lexical** — the carrier variable, which need not share the class name
///    (`---@class Wrapper` over `local Animal = {}`). `function Animal:m()`
///    names that *binding*, which is what Lua itself resolves.
/// 2. **Nominal** — the class name, which is also a valid carrier reference
///    (`---@class Animal` … `function Animal:m()`).
///
/// **Lexical beats nominal**, and the two passes below are ordered to say so.
/// Filling one map with both, last-write-wins, made the answer depend on
/// declaration order: a later `---@class Animal` whose own carrier is some
/// other variable would enter the nominal alias `Animal -> Animal` on top of
/// an earlier lexical binding `Animal -> Wrapper`, and
/// [`absorb_carrier_members`] then folded `function Animal:speak()` onto the
/// wrong class — while the project-source inference path, which resolves the
/// binding, got it right. Swapping the two class blocks flipped the verdict
/// (Shockwave round 3); #39's whole point is defs/project parity.
///
/// Within the **nominal** rank the first declaration wins, matching every
/// other collision rule in the crate. Within the **lexical** rank the *last*
/// one does, because that rank is not a collision rule at all — it is a
/// binding lookup, and Lua's answer for a repeated `local M` is the most
/// recent binding. `function M:m()` after two `local M = {}` carriers names
/// the second; the project-source inference path resolves the binding and
/// says so, while this path used to answer with the first, so the two
/// disagreed on every shape with a repeated carrier variable (Shockwave
/// round 4). #39's whole point is defs/project parity, so the defs path
/// follows the binding too. This is a different axis from the rank order
/// above: lexical still beats nominal, whichever declaration order they come
/// in.
///
/// Shared by the standalone-visibility pass (#115) and the defs
/// carrier-member fold (#39).
fn carrier_var_classes(
    items: &[luacats::AnnotatedItem],
    root: &SyntaxNode,
) -> HashMap<String, String> {
    let classes = || {
        items.iter().flat_map(|item| {
            item.block.tags.iter().filter_map(move |tag| match tag {
                Tag::Class(c) if !c.name.is_empty() => Some((item, c)),
                _ => None,
            })
        })
    };

    // Rank 2 first, so rank 1 can overwrite it.
    let mut var_to_class: HashMap<String, String> = HashMap::new();
    for (_, c) in classes() {
        var_to_class
            .entry(c.name.clone())
            .or_insert_with(|| c.name.clone());
    }

    // Rank 1, collected last-wins among themselves (the binding a later
    // `function M:m()` names), then applied over the nominal aliases — one
    // write per variable, so the rank order survives whatever the declaration
    // order was.
    let mut lexical: HashMap<String, String> = HashMap::new();
    for (item, c) in classes() {
        if let Some(span) = item.target
            && let Some(name) =
                stmt_at(root, (span.start, span.end)).and_then(|s| carrier_var_name(&s))
        {
            lexical.insert(name, c.name.clone());
        }
    }
    var_to_class.extend(lexical);
    var_to_class
}

/// The variable a `---@class` carrier statement binds: the first name of a
/// `local M = {}`, or the sole `Name` target of an `M = {}` assignment.
fn carrier_var_name(stmt: &Stmt) -> Option<String> {
    match stmt {
        Stmt::Local(local) => Some(local.names().next()?.name()?.text().to_string()),
        Stmt::Assign(assign) => match assign.targets()?.exprs().next()? {
            Expr::Name(name) => Some(name.name()?.text().to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The `(carrier, member)` a standalone `---@private`/`---@protected`/
/// `---@package` tag targets (#115): either a `function Carrier.member()` /
/// `function Carrier:method()` declaration, or a single-target
/// `Carrier.member = <value>` field assignment. The assignment form mirrors
/// luals, which resolves the class from the assignment base variable
/// (`script/vm/visible.lua` `getParentClass` → `vm.getDefinedClass(uri,
/// source.node)` for `setfield`/`setindex` nodes), so `---@private` on
/// `Carrier.method = function() end` associates `method` with `Carrier`'s
/// class. Conservative: multi-target assignments (`a.x, b.y = f, g`) and
/// non-field targets stay unwired — no false `invisible`.
fn visibility_carrier_member(stmt: &Stmt) -> Option<(String, String)> {
    match stmt {
        Stmt::FunctionDecl(f) => {
            let segments: Vec<String> =
                f.name()?.segments().map(|s| s.text().to_string()).collect();
            let [carrier, .., member] = segments.as_slice() else {
                return None;
            };
            Some((carrier.clone(), member.clone()))
        }
        Stmt::Assign(assign) => {
            let target_list = assign.targets()?;
            let mut targets = target_list.exprs();
            let first = targets.next()?;
            // Single-target only — `a.x, b.y = f, g` stays unwired.
            if targets.next().is_some() {
                return None;
            }
            let Expr::Field(field) = first else {
                return None;
            };
            let Expr::Name(base) = field.base()? else {
                return None;
            };
            let carrier = base.name()?.text().to_string();
            let member = field.field_name()?.text().to_string();
            Some((carrier, member))
        }
        _ => None,
    }
}

/// The innermost statement whose range is exactly `target`.
fn stmt_at(root: &SyntaxNode, target: Target) -> Option<Stmt> {
    root.descendants()
        .filter(|node| {
            let range = node.text_range();
            (usize::from(range.start()), usize::from(range.end())) == target
        })
        .find_map(Stmt::cast)
}

/// Build an [`EnumDef`] from the table constructor the `---@enum` annotates.
fn enum_def(tag: &luacats::EnumTag, target: Option<luacats::Span>, root: &SyntaxNode) -> EnumDef {
    let mut members = BTreeMap::new();
    let table = target
        .and_then(|span| stmt_at(root, (span.start, span.end)))
        .and_then(|stmt| match stmt {
            Stmt::Local(local) => enum_table(&local),
            _ => None,
        });
    if let Some(table) = table {
        for field in table.fields() {
            let lua::ast::TableField::Name(named) = field else {
                continue;
            };
            let Some(name) = named.name() else {
                continue;
            };
            let value = if tag.key {
                // `---@enum (key)`: the enum's values are its *keys*.
                Some(Ty::StringLit(name.text().to_string()))
            } else {
                named.value().as_ref().and_then(literal_ty)
            };
            members.insert(name.text().to_string(), value.unwrap_or(Ty::Unknown));
        }
    }
    let value_union = Ty::union(members.values().cloned().collect());
    EnumDef {
        members,
        value_union,
    }
}

fn enum_table(local: &LocalStmt) -> Option<lua::ast::TableExpr> {
    match local.values()?.exprs().next()? {
        Expr::Table(table) => Some(table),
        _ => None,
    }
}

/// Whether an assignment's (first) right-hand side is a table constructor —
/// the `NAME = {}` def idiom that declares a plain global module table (#105).
fn is_table_constructor(assign: &lua::ast::AssignStmt) -> bool {
    matches!(
        assign.values().and_then(|v| v.exprs().next()),
        Some(Expr::Table(_))
    )
}

/// Fold a doc block's use-site tags onto the signature its `---@type fun(…)`
/// declares.
///
/// `---@deprecated`, `---@async`, `---@nodiscard` and `---@version` attach to
/// the whole block, and `fun(…)` syntax has nowhere to write them, so a block
/// carrying both an explicit `---@type` and a tag would otherwise publish the
/// declared shape with the tags stripped (#38). The declared signature stays
/// authoritative for everything it *can* express — parameters, returns,
/// overloads and generics.
pub(crate) fn merge_block_tags(mut declared: FunctionTy, block: Option<&FunctionTy>) -> FunctionTy {
    let Some(block) = block else {
        return declared;
    };
    declared.deprecated |= block.deprecated;
    declared.is_async |= block.is_async;
    declared.nodiscard |= block.nodiscard;
    if declared.version.is_none() {
        declared.version.clone_from(&block.version);
    }
    declared
}

/// A declared `---@field` that shadows a same-class carrier attachment, with
/// the attachment's use-site tags (`---@deprecated`, `---@async`,
/// `---@version`) folded in.
///
/// The `---@field` line stays authoritative for the member's type — parameters,
/// returns, overloads and generics all come from it — but its `fun(...)` syntax
/// cannot express those tags, so they only ever live on the carrier
/// (`---@deprecated` above `function C:m()`). Applied at the two points a
/// shadowed attachment is otherwise dropped — [`FileTypes::collect`], which
/// folds a file's carriers into its exported surface, and
/// [`TypeEnv::merge_file_types`], which folds that surface into the ambient —
/// so the tags survive into the *consumer* file's class shape. The same-file
/// half is `crate::infer::Inferencer::carrier_tagged`, which reads the live
/// carrier shape directly. Non-function members, and attachments carrying no
/// tags, pass the declaration through unchanged (#33).
/// Trust a carrier method's own body-inferred return type as part of the
/// class's published, workspace-global surface (finding 2 of
/// `docs/03-reference/03-class-merge-precedence.md`).
///
/// [`crate::infer::reify::Infer::reify_func`] stamps every reified,
/// unannotated function's `has_return_annotation` with
/// `returns_set && self.mode.seeds_params()` — `false` in [`InferMode::Check`]
/// (the mode both [`crate::module_surface_from_env`] and the checker itself
/// run in). That flag conflates two different questions: whether an
/// unannotated *parameter* was seeded from call-site argument types (a
/// genuine guess — SPEC §19 forbids resting a diagnostic on it, so it is
/// rightly `Display`-only) and whether a function's *return* type is known
/// at all (a plain deduction from its own `return` statements, no guessing
/// about other call sites involved — the same deduction a same-file
/// `f:m()` call already rests a diagnostic on today, via the live,
/// not-yet-reified `ITy::Func` path `infer::call::eval_method_call`'s
/// `returns_of` takes). Reusing the parameter-seeding flag to *also* gate
/// the return type erases a carrier method's return type the moment it
/// crosses into this file's published [`ClassDef::methods`] — the one seam
/// `ClassDef::methods`'s own doc comment promises behaves "exactly like
/// `---@field` members" — even with zero cross-file duplication to
/// arbitrate: `f:m()` typed `unknown` where `f.x` (a `---@field`, whose type
/// is annotation-derived and never touches this flag) resolves fine.
///
/// This promotes exactly that one case — a reified, unannotated function
/// whose return type inference *did* determine (`returns` non-empty) —
/// before it is published in a class's method surface, leaving every other
/// user of a reified [`Ty::Function`] (module exports, inlay display,
/// [`check::conformance`]'s `carrier_class_final` fallback) untouched: none
/// of those read through this function, so a `require`'d free function's
/// unannotated return type still does not seed a consumer's diagnostics —
/// only a class's own carrier-attached methods, which are declarations, not
/// call-site guesses.
fn publish_carrier_method_ty(field: &FieldTy) -> FieldTy {
    let Ty::Function(sig) = &field.ty else {
        return field.clone();
    };
    if sig.has_return_annotation || sig.returns.is_empty() {
        return field.clone();
    }
    let mut sig = (**sig).clone();
    sig.has_return_annotation = true;
    FieldTy {
        ty: Ty::Function(Box::new(sig)),
        optional: field.optional,
    }
}

fn with_carrier_tags(declared: &FieldTy, carrier: &FieldTy) -> FieldTy {
    let (Ty::Function(declared_sig), Ty::Function(carrier_sig)) = (&declared.ty, &carrier.ty)
    else {
        return declared.clone();
    };
    if !carrier_sig.deprecated && !carrier_sig.is_async && carrier_sig.version.is_none() {
        return declared.clone();
    }
    let mut sig = (**declared_sig).clone();
    sig.deprecated |= carrier_sig.deprecated;
    sig.is_async |= carrier_sig.is_async;
    sig.version = sig.version.or_else(|| carrier_sig.version.clone());
    FieldTy {
        ty: Ty::Function(Box::new(sig)),
        optional: declared.optional,
    }
}

/// The literal type of a literal expression, if it is one.
pub(crate) fn literal_ty(expr: &Expr) -> Option<Ty> {
    let Expr::Literal(lit) = expr else {
        return None;
    };
    let token = lit.token()?;
    Some(match token.kind() {
        SyntaxKind::NIL_KW => Ty::Nil,
        SyntaxKind::TRUE_KW => Ty::BoolLit(true),
        SyntaxKind::FALSE_KW => Ty::BoolLit(false),
        SyntaxKind::NUMBER => Ty::NumberLit(token.text().to_string()),
        SyntaxKind::STRING => Ty::StringLit(unquote_lua(token.text())),
        _ => return None,
    })
}

/// Strip the delimiters from a Lua string literal (quotes or long
/// brackets). Escape sequences are kept verbatim (MVP: literal-type
/// comparison is textual).
#[expect(
    clippy::string_slice,
    reason = "both slices are bounded by ASCII delimiters verified on the byte array first (quotes at [0]/[len-1], or `[==[`/`]==]` long brackets), so the indices are char boundaries within bounds"
)]
pub(crate) fn unquote_lua(raw: &str) -> String {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        return raw[1..raw.len() - 1].to_string();
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let level = rest.bytes().take_while(|&b| b == b'=').count();
        let open = level + 2;
        let close = format!("]{}]", "=".repeat(level));
        if raw.len() >= open + close.len() && raw.ends_with(close.as_str()) {
            return raw[open..raw.len() - close.len()].to_string();
        }
    }
    raw.to_string()
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

    fn env_of(source: &str) -> TypeEnv {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        TypeEnv::build(&parsed)
    }

    /// The wall-clock ceiling a bounded test gets unless it names its own.
    ///
    /// Several tests in this module assert a *termination* property: a walk
    /// that is O(V) with its guard in place and exponential (or infinite)
    /// without it. No assertion on the returned value can see "correct but
    /// exponential", so the bound IS the assertion — and it exists so that a
    /// mutant which kills the guard fails FAST rather than hanging the suite,
    /// which cargo-mutants would read as a timeout rather than a kill.
    ///
    /// One number, one rationale, one place to change it (round 11 review
    /// R11-3's clean-code note: the scaffold was copy-pasted across ~7 tests
    /// with 5/10/20/30s constants scattered through them, which is the
    /// policy drift that produced R11-3 itself). 10s is comfortably above
    /// every bounded fixture's measured runtime here — the slowest, the
    /// k=50 conflicting-diamond discovery walks, run well under a second on
    /// an unoptimized debug build — and comfortably below
    /// `mutants-gate.sh`'s own `--timeout 90`, so the in-test bound always
    /// fires first and a slow mutant is a KILL, not a gate-failing TIMEOUT.
    /// A test needing more must say so with [`run_bounded_within`] AND stay
    /// under that 90s ceiling.
    const BOUND: std::time::Duration = std::time::Duration::from_secs(10);

    /// Run `body` on its own thread and fail — fast — if it has not finished
    /// within [`BOUND`]. `what` completes the sentence "…must finish within
    /// the bound: ".
    fn run_bounded<T: Send + 'static>(what: &str, body: impl FnOnce() -> T + Send + 'static) -> T {
        run_bounded_within(BOUND, what, body)
    }

    /// [`run_bounded`] with an explicit ceiling, for a fixture whose cost is
    /// genuinely different from the rest.
    ///
    /// The two failure modes are told apart, not both reported as the slow
    /// one (round 12 review): `Disconnected` means the worker dropped its
    /// sender without sending — it panicked, in microseconds — and blaming
    /// that on a timeout it never came close to hitting is exactly the
    /// caught-vs-timeout confusion this helper exists to protect in the
    /// mutants gate.
    fn run_bounded_within<T: Send + 'static>(
        bound: std::time::Duration,
        what: &str,
        body: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(body());
        });
        match rx.recv_timeout(bound) {
            Ok(value) => value,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what} must finish within {bound:?}")
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{what} panicked (see the worker thread's own message above)")
            }
        }
    }

    #[test]
    fn a_single_parents_own_field_declaration_overrides_its_inherited_value() {
        // production-readiness-assessment-9natxz A1's own regression: the
        // *first* shape of the `member_wins`/`is_first_binding` fix merged
        // `own` (this class's own fields/methods) into `shape` using the
        // same gate as two unrelated siblings, WITHOUT moving `own` ahead of
        // the parents loop — and a root (or any first-visited) class's own
        // `is_first_binding` is `true` exactly like an unrelated sibling's,
        // so `Sub`'s own field refused to overwrite what `Sub`'s own
        // recursion into `Base`, moments earlier in the same visit, had just
        // written. The most basic single-inheritance override in the
        // language broke before this shipped. `collect_class` now merges a
        // class's own contribution *before* recursing into its parents (see
        // its doc comment), which fixes this by construction: `Sub`'s own
        // value claims the key before `Base` is ever visited.
        let env = env_of(
            "\
---@class Base
---@field x number
---@class Sub : Base
---@field x string
",
        );
        let shape = env.class_shape("Sub").expect("declared class");
        assert_eq!(
            shape.fields["x"].ty,
            Ty::String,
            "Sub's own override must win over Base's inherited field"
        );
    }

    #[test]
    #[ignore = "manual stack-depth probe, not a CI assertion — see doc comment"]
    fn stack_probe_deep_single_parent_chain() {
        // Manual probe for round 5 review N2's ORIGINAL (non-durable) fix:
        // `luabox`'s dispatcher/rayon threads run with a 16 MiB pinned stack
        // (`luabox-cli/src/main.rs::PINNED_STACK_BYTES`). Run with
        // `cargo test -p luabox-types --lib --release -- --ignored --nocapture
        // env::tests::stack_probe_deep_single_parent_chain` and watch for
        // SIGSEGV/abort rather than a clean assertion failure — a stack
        // overflow does not unwind, so this cannot assert its own failure.
        //
        // Superseded by `MAX_ANCESTRY_DEPTH` (N2's DURABLE fix, see its doc
        // comment): every `n` below now returns `ok=true` well inside the
        // guard's cap, so this probe can no longer find a crash floor — it
        // is kept only as a manual sanity check that the cap holds even at
        // scales the guard was never meant to reach for real, not as a live
        // measurement tool — the actual `MAX_ANCESTRY_DEPTH` reasoning was
        // bisected by hand with a scratch probe of the same shape (not left
        // in the tree; see the constant's own doc comment for the numbers
        // it found), and this probe predates that measurement.
        for n in [20_000, 25_000, 29_000, 31_000, 40_000, 50_000] {
            let mut src = String::from("---@class C0\n---@field item number\n");
            for i in 1..=n {
                use std::fmt::Write as _;
                let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
            }
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::Builder::new()
                .stack_size(16 * 1024 * 1024)
                .spawn(move || {
                    let env = env_of(&src);
                    let shape = env.class_shape(&format!("C{n}"));
                    let _ = tx.send(shape.is_some());
                })
                .expect("spawn probe thread")
                .join()
                .expect("probe thread must not panic/abort");
            let ok = rx.recv().expect("probe thread must send a result");
            eprintln!("n={n:<6} ok={ok}");
        }
    }

    /// The workspace-global surface one source file contributes.
    fn surface(source: &str) -> FileTypes {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let items = luacats::harvest(&parsed);
        let env = TypeEnv::build_from_items(&parsed, &items, None);
        FileTypes::collect(&items, &env, &HashMap::new(), "surface.lua")
    }

    // --- string-literal delimiters ---------------------------------------

    #[test]
    fn unquote_strips_matching_quotes() {
        assert_eq!(unquote_lua("\"abc\""), "abc");
        assert_eq!(unquote_lua("'abc'"), "abc");
        // Escapes are kept verbatim — literal-type comparison is textual.
        assert_eq!(unquote_lua("\"a\\nb\""), "a\\nb");
        assert_eq!(unquote_lua("\"\""), "");
    }

    #[test]
    fn unquote_strips_long_brackets_at_every_level() {
        assert_eq!(unquote_lua("[[abc]]"), "abc");
        assert_eq!(unquote_lua("[=[abc]=]"), "abc");
        assert_eq!(unquote_lua("[===[abc]===]"), "abc");
        // A nested `]]` inside a level-1 long string survives.
        assert_eq!(unquote_lua("[=[a]]b]=]"), "a]]b");
        assert_eq!(unquote_lua("[[]]"), "");
    }

    #[test]
    fn unquote_leaves_unrecognized_text_alone() {
        // Mismatched or absent delimiters: return the raw text rather than
        // slicing into something that is not there.
        assert_eq!(unquote_lua("\"abc'"), "\"abc'");
        assert_eq!(unquote_lua("[=[abc]==]"), "[=[abc]==]");
        assert_eq!(unquote_lua("[["), "[[");
        assert_eq!(unquote_lua(""), "");
        assert_eq!(unquote_lua("\""), "\"");
        // A token that ENDS like a long bracket but is shorter than its own
        // delimiters (#58 mutation audit): the length half of the guard is
        // the only thing standing between this and an out-of-range slice —
        // every input above satisfies or fails BOTH halves at once, so only
        // this one can catch the length check weakening.
        assert_eq!(unquote_lua("[]]"), "[]]");
    }

    // --- definition-layer merge order ------------------------------------

    #[test]
    fn merge_keep_first_is_first_wins() {
        // The `workspace.library` precedence: the value already present on a
        // key survives (the opposite of `BTreeMap::append`).
        let mut into: BTreeMap<String, u8> = BTreeMap::from([("a".into(), 1)]);
        merge_keep_first(
            &mut into,
            BTreeMap::from([("a".into(), 2), ("b".into(), 3)]),
        );
        assert_eq!(into, BTreeMap::from([("a".into(), 1), ("b".into(), 3)]));
    }

    #[test]
    fn merging_a_file_surface_inserts_classes_absent_from_the_base() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface("---@class Fresh\n---@field n number\n"));
        let shape = env.class_shape("Fresh").expect("class merged in whole");
        assert_eq!(shape.fields["n"].ty, Ty::Number);
    }

    #[test]
    fn merging_a_present_class_is_member_wise_with_the_base_winning() {
        // The base (defs layer) already declares `Both`; the project file
        // re-declares it with a different `n` and adds `extra`, a parent, an
        // indexer, an operator and a private member. Base members win on a
        // collision; everything new is additive.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Parent
---@class Both
---@field n number
",
        ));
        env.merge_file_types(&surface(
            "\
---@class Other
---@class Both : Parent, Other
---@field n string
---@field extra boolean
---@field [integer] string
---@field private hidden number
---@operator add(Both): Both
",
        ));
        let def = env.classes.get("Both").expect("merged class");
        // Base's `n` survives; the second declaration's `extra` is added.
        assert_eq!(def.fields["n"].ty, Ty::Number);
        assert_eq!(def.fields["extra"].ty, Ty::Boolean);
        // Full `ParentRef` equality (name *and* args), not just the name
        // projection `parent_names` gives — F41: the weakened form would
        // still pass if a seam dropped or mis-substituted a parent's bound
        // arguments, which is exactly what F38 found.
        assert_eq!(
            def.parents,
            vec![
                ParentRef {
                    name: "Parent".to_string(),
                    args: Vec::new(),
                },
                ParentRef {
                    name: "Other".to_string(),
                    args: Vec::new(),
                },
            ]
        );
        assert_eq!(def.indexers, vec![(Ty::Integer, Ty::String)]);
        assert_eq!(def.operators["add"].len(), 1);
        assert_eq!(def.visibility.get("hidden"), Some(&FieldScope::Private));
    }

    #[test]
    fn merging_repeated_declarations_never_duplicates_parents_or_operators() {
        let file = surface(
            "\
---@class Base
---@class Dup : Base
---@field [integer] string
---@operator add(Dup): Dup
",
        );
        let mut env = TypeEnv::default();
        env.merge_file_types(&file);
        env.merge_file_types(&file);
        let def = env.classes.get("Dup").expect("merged class");
        // Full equality (F41): a name-only projection cannot see a seam that
        // duplicates the same name with different (or mis-substituted) args.
        assert_eq!(
            def.parents,
            vec![ParentRef {
                name: "Base".to_string(),
                args: Vec::new(),
            }]
        );
        assert_eq!(def.indexers.len(), 1);
        assert_eq!(def.operators["add"].len(), 1);
    }

    #[test]
    fn merge_file_types_keeps_the_first_declarations_conflicting_indexer_value() {
        // `merge_file_types`'s indexer loop doc comment (round 5 review N7):
        // "first declaration wins" for a re-declared `---@field [K] V`. The
        // only existing coverage of this seam's indexer path
        // (`merging_a_present_class_is_member_wise_with_the_base_winning`
        // declares the indexer once; `merging_repeated_declarations_never_
        // duplicates_parents_or_operators` re-declares it with an IDENTICAL
        // value) — so first-wins and an unconditional overwrite produce the
        // same observable entry count and value in both, and neither can
        // tell the two rules apart. Here the second file's `[string]`
        // indexer conflicts on VALUE (`string` vs the first file's
        // `number`), so only a real first-wins rule keeps `number`.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface("---@class Both\n---@field [string] number\n"));
        env.merge_file_types(&surface("---@class Both\n---@field [string] string\n"));
        let def = env.classes.get("Both").expect("merged class");
        assert_eq!(
            def.indexers,
            vec![(Ty::String, Ty::Number)],
            "the first file's conflicting indexer value must win, not the second's"
        );
    }

    #[test]
    fn all_three_indexer_seams_dedupe_one_key_the_same_way() {
        // PR #61 round-8 F20. The `HashMap<Ty, usize>` scaffold that pairs an
        // indexer key with its slot was hand-copied at three sites —
        // `merge_file_types`' duplicate fold, `absorb_block`'s per-tag fold,
        // and `collect_class`'s ancestor fold — in three local shapes, so any
        // change to which `---@field [K] V` counts as a duplicate had to be
        // made three times or the copies diverged. They now share
        // `IndexerIndex`, and this is what fails if one grows its own again.
        //
        // Each seam is fed the same three tags, and the shape makes BOTH
        // halves of "what counts as a duplicate key" observable:
        //  - `[string|number]` arrives twice with different values, so a seam
        //    that stopped deduping shows up as a surplus entry; and
        //  - `[string|boolean]` is a NEAR MISS — a distinct `Ty` that any
        //    canonicalisation shallower than `Ty`'s own equality (a rendered
        //    name, the union's first member, the key's discriminant) collapses
        //    onto the first key, losing an entry. A bare `[string]` fixture
        //    cannot tell those apart; a compound key can.
        let first = "---@field [string|number] number\n";
        let repeat = "---@field [string|number] boolean\n";
        let near_miss = "---@field [string|boolean] string\n";

        // Seam 1 — `merge_file_types`, two files re-declaring one class.
        let mut cross_file = TypeEnv::default();
        cross_file.merge_file_types(&surface(&format!("---@class M\n{first}{near_miss}")));
        cross_file.merge_file_types(&surface(&format!("---@class M\n{repeat}")));
        let cross_file = cross_file.classes["M"].indexers.clone();

        // Seam 2 — `absorb_block`, three tags on one declaration.
        let in_file = env_of(&format!("---@class A\n{first}{near_miss}{repeat}")).classes["A"]
            .indexers
            .clone();

        // Seam 3 — `collect_class`, two unrelated parents, the repeat
        // arriving from the second-listed one.
        let ancestor_fold = env_of(&format!(
            "---@class P1\n{first}{near_miss}---@class P2\n{repeat}---@class C : P1, P2\n"
        ))
        .class_shape("C")
        .expect("C is declared")
        .indexers;

        assert_eq!(
            cross_file,
            vec![
                (Ty::Union(vec![Ty::String, Ty::Number]), Ty::Number),
                (Ty::Union(vec![Ty::String, Ty::Boolean]), Ty::String),
            ],
            "two distinct keys, two slots, each holding its first-declared value"
        );
        assert_eq!(
            in_file, cross_file,
            "the same-file fold must canonicalise the key exactly as the cross-file one does"
        );
        assert_eq!(
            ancestor_fold, cross_file,
            "and so must the ancestor fold — three seams, one rule about what a duplicate \
             indexer key IS"
        );
    }

    #[test]
    fn indexer_resolution_matches_an_independent_truth_table() {
        // Round 6 review M10: `indexer_resolution` used to hand-maintain its
        // own three-way decision 48 lines apart from `member_wins`'s
        // two-way one, with nothing enforcing the two stayed in agreement —
        // exhaustive over every `(exists, arrival)` combination, against a
        // literal expected outcome rather than either function's own logic,
        // so this stays a real check before *and* after `indexer_resolution`
        // is rewritten to call `member_wins` internally (not a tautology
        // that would pass no matter what either body says).
        let cases = [
            (false, MemberArrival::Duplicate, "Insert"),
            (
                false,
                MemberArrival::AncestorFold {
                    is_first_binding: true,
                },
                "Insert",
            ),
            (
                false,
                MemberArrival::AncestorFold {
                    is_first_binding: false,
                },
                "Insert",
            ),
            (true, MemberArrival::Duplicate, "Keep"),
            (
                true,
                MemberArrival::AncestorFold {
                    is_first_binding: true,
                },
                "Keep",
            ),
            (
                true,
                MemberArrival::AncestorFold {
                    is_first_binding: false,
                },
                "Overwrite",
            ),
        ];
        for (exists, arrival, expected) in cases {
            let got = match indexer_resolution(exists, arrival) {
                IndexerResolution::Insert => "Insert",
                IndexerResolution::Overwrite => "Overwrite",
                IndexerResolution::Keep => "Keep",
            };
            assert_eq!(got, expected, "exists={exists} arrival={arrival:?}");
        }
    }

    #[test]
    fn a_class_with_thousands_of_indexer_fields_dedupes_in_bounded_time() {
        // Round 6 review M55: the same-file indexer dedup in `absorb_block`
        // (the `FieldKey::Indexer` arm of its `Tag::Field` match) used to
        // scan `class.indexers` with a plain `.iter().any(..)` on every one
        // of a class's `---@field [K] V` tags, so a single class with K such
        // fields cost O(K²) to absorb — quadratic growth confirmed on a
        // release CLI build: K=5,000/10,000/20,000 measured 0.269s/1.009s/
        // 3.078s before this fix's `indexer_index` cache, 0.026s/0.045s/
        // 0.089s after (linear). `absorb_block` calls `indexer_resolution`
        // once per tag either way, so this is the one seam actually
        // exercised by "many `---@field [K] V` entries on one class" (the
        // finding's own repro shape) — `merge_file_types`'s and
        // `collect_class`'s own indexer loops got the identical fix but are
        // batch merges of an already-built list, not a per-tag scan, so
        // they were never the dominant K² cost here.
        //
        // K=20,000 (matching the CLI table above) under this module's shared
        // bound (`run_bounded`/`BOUND`): comfortably fast
        // after the fix (well under 1s even on this unoptimized debug
        // build); reverting just the `indexer_index` cache reproduces a
        // real timeout here, confirmed by hand.
        let k = 20_000;
        let mut source = String::from("---@class Big\n");
        for i in 0..k {
            use std::fmt::Write as _;
            let _ = writeln!(source, "---@field [{i}] number");
        }
        let len = run_bounded(
            "absorbing 20,000 indexer fields on one class — an O(K²) regression would not \
             return in this process's lifetime",
            move || {
                let env = env_of(&source);
                env.classes.get("Big").map(|def| def.indexers.len())
            },
        )
        .expect("the class is declared");
        assert_eq!(len, k, "every distinct indexer key must survive the dedup");
    }

    #[test]
    fn parents_dedupe_by_name_identically_whether_the_redeclaration_is_same_file_or_cross_file() {
        // Round 6 review M9: the `parents` cell had no single owner — an
        // identical dedupe-by-name, first-wins body was hand-copied into
        // `absorb_block`'s same-file re-declaration case and
        // `merge_file_types`'s cross-file case, the one member kind missing
        // from the table every other kind in this section is named after.
        // Both seams now call the shared `push_parent_ref`; this pins that
        // they still agree once it is the only body either one runs.
        let same_file = env_of(
            "\
---@class Base<T>
---@class Dup : Base<number>
---@class Dup : Base<string>
",
        );
        let def = same_file.classes.get("Dup").expect("class declared");
        assert_eq!(
            def.parents,
            vec![ParentRef {
                name: "Base".to_string(),
                args: vec![Ty::Number],
            }],
            "same-file: the first-declared parent binding must win"
        );

        let mut cross_file = TypeEnv::default();
        cross_file.merge_file_types(&surface(
            "---@class Base<T>\n---@class Dup : Base<number>\n",
        ));
        cross_file.merge_file_types(&surface("---@class Dup : Base<string>\n"));
        let def = cross_file.classes.get("Dup").expect("merged class");
        assert_eq!(
            def.parents,
            vec![ParentRef {
                name: "Base".to_string(),
                args: vec![Ty::Number],
            }],
            "cross-file: the first-declared parent binding must win, matching same-file"
        );
    }

    #[test]
    fn merging_a_renamed_redeclaration_substitutes_its_parents_type_arguments() {
        // F38 (cross-file seam): the base declares `Both<T>`; the incoming
        // file re-declares it as `Both<U> : Container<U>` — its OWN
        // parameter is spelled `U`, and its parent reference passes that same
        // `U` through to `Container`. The merge must rename `U` to the
        // canonical `T` in the re-declaration's field bodies (already
        // covered) *and* in `parent.args` — otherwise `Container`'s bound
        // argument is left naming a parameter `Both` itself does not have,
        // unresolvable at every downstream instantiation site (#59, F38).
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface("---@class Both<T>\n---@field n T\n"));
        env.merge_file_types(&surface(
            "\
---@class Container<U>
---@class Both<U> : Container<U>
---@field extra U
",
        ));
        let def = env.classes.get("Both").expect("merged class");
        assert_eq!(def.params, vec!["T".to_string()]);
        assert_eq!(
            def.parents,
            vec![ParentRef {
                name: "Container".to_string(),
                args: vec![Ty::Named("T".to_string())],
            }],
            "the parent's argument must be renamed onto the canonical `T`, not left as the free `U`"
        );
        // …and the rename reaches the re-declaration's own field body too,
        // matching the parent-args fix rather than regressing it.
        assert_eq!(def.fields["extra"].ty, Ty::Named("T".to_string()));
    }

    #[test]
    fn a_declaration_that_first_supplies_a_bare_classs_parameter_keeps_its_own_field_named() {
        // Coordinator follow-up (round 3 review 2(a) fix uncovered this):
        // `class_rename` used to be computed against `existing.params`
        // *before* the "a re-declaration may supply the parameters the first
        // one omitted" update a few lines below it — so for a bare first
        // declaration (`params` starts empty), the rename saw an empty
        // canonical list, `class_param_unification(["T"], [])` did not
        // consider `["T"]` equal to `[]`, and `positional_rename` mapped `T`
        // to `unknown` (no canonical slot 0 to become) — the re-declaration's
        // OWN, now-canonical parameter, renamed to nothing, on the
        // declaration that just fixed the canonical list to be exactly that.
        // Invisible to every test that only read the class through the old
        // template-only generic-reference path, which never consulted
        // `class_shape`/`env.classes` for this; the plain-reference path
        // (`---@type Boxed` with no args, or anything routed through
        // `class_shape`) had this wrong all along.
        let env = env_of(
            "\
---@class Boxed
---@class Boxed<T>
---@field value T
",
        );
        let def = env.classes.get("Boxed").expect("declared class");
        assert_eq!(def.params, vec!["T".to_string()]);
        assert_eq!(
            def.fields["value"].ty,
            Ty::Named("T".to_string()),
            "the re-declaration's own field must stay bound to its own \
             (now-canonical) parameter, not collapse to `unknown`"
        );
    }

    #[test]
    fn a_same_file_renamed_redeclaration_substitutes_its_parents_type_arguments() {
        // F38 (in-file seam, `absorb_block`): the same shape as above, but
        // both declarations live in one file, so the fix must apply on the
        // `absorb_block` path independently of `merge_file_types`.
        let env = env_of(
            "\
---@class Container<U>
---@class Both<T>
---@field n T
---@class Both<U> : Container<U>
---@field extra U
",
        );
        let def = env.classes.get("Both").expect("declared class");
        assert_eq!(def.params, vec!["T".to_string()]);
        assert_eq!(
            def.parents,
            vec![ParentRef {
                name: "Container".to_string(),
                args: vec![Ty::Named("T".to_string())],
            }]
        );
        assert_eq!(def.fields["extra"].ty, Ty::Named("T".to_string()));
    }

    #[test]
    fn absorb_block_keeps_the_first_declarations_conflicting_indexer_value() {
        // `absorb_block`'s `FieldKey::Indexer` arm doc comment (round 5
        // review N7): the same first-wins rule as
        // `merge_file_types_keeps_the_first_declarations_conflicting_indexer_value`
        // above, pinned on the in-file duplicate-`---@class` seam instead of
        // the cross-file one — the two seams keep separate implementations
        // (round 5 review N7's note in `collect_class`'s own doc comment)
        // and neither test can stand in for the other. Same conflicting-
        // value shape: the first `Both` declares `[string] number`, the
        // second re-declares `[string] string`.
        let env = env_of(
            "\
---@class Both
---@field [string] number
---@class Both
---@field [string] string
",
        );
        let def = env.classes.get("Both").expect("declared class");
        assert_eq!(
            def.indexers,
            vec![(Ty::String, Ty::Number)],
            "the first declaration's conflicting indexer value must win, not the second's"
        );
    }

    #[test]
    fn absorb_block_dedupes_a_byte_identical_operator_repeat_the_same_as_merge_file_types() {
        // `push_operator_overload`'s doc comment: the same-file
        // (`absorb_block`) and cross-file (`merge_file_types`) duplicate-
        // `---@class` seams used to disagree on whether a byte-identical
        // `---@operator` repeat collapses to one entry or survives as two —
        // `merge_file_types` always deduped (pinned by
        // `merging_repeated_declarations_never_duplicates_parents_or_operators`
        // below), `absorb_block` never did. Two `Both` blocks in one file
        // declaring the *identical* `add` overload must now collapse to one
        // entry here too, matching the cross-file rule for the identical
        // shape.
        let env = env_of(
            "\
---@class Both
---@operator add(Both): Both
---@class Both
---@operator add(Both): Both
",
        );
        let def = env.classes.get("Both").expect("declared class");
        let sigs = def.operators.get("add").expect("operator carried over");
        assert_eq!(
            sigs.len(),
            1,
            "a byte-identical `---@operator` repeat in one file must dedupe, \
             matching merge_file_types's cross-file rule for the same shape: {sigs:?}"
        );
    }

    #[test]
    fn absorb_block_keeps_two_operator_overloads_that_differ_on_result() {
        // The other half of the same doc comment: dedupe compares the whole
        // signature, so two overloads that genuinely differ (here, on
        // `result`) both survive — this is `operator-B`'s own fixture shape
        // (`docs/03-reference/03-class-merge-precedence.md`), unaffected by
        // the fix above, which only collapses a truly identical repeat.
        let env = env_of(
            "\
---@class Both
---@operator add(Both): number
---@class Both
---@operator add(Both): string
",
        );
        let def = env.classes.get("Both").expect("declared class");
        let sigs = def.operators.get("add").expect("operator carried over");
        assert_eq!(
            sigs.len(),
            2,
            "two overloads that disagree on result must both survive, not dedupe: {sigs:?}"
        );
    }

    #[test]
    fn merging_a_renamed_redeclaration_substitutes_its_own_operators_type_arguments() {
        // Round 4 review finding 3 (`merge_file_types` seam): R5's fix
        // renames a re-declaration's OWN `---@operator` bodies onto the
        // canonical parameter list, the same as it already does for fields,
        // parents and indexers — but had zero test coverage of its own.
        // `Both<T>` is canonical; the incoming file re-declares it as
        // `Both<U>` and attaches `---@operator add(U): U`. Un-renamed, that
        // signature would name `U`, a parameter `Both` does not have (its
        // canonical list reads `T`) — unresolvable at every use site.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface("---@class Both<T>\n---@field n T\n"));
        env.merge_file_types(&surface(
            "\
---@class Both<U>
---@operator add(U): U
",
        ));
        let def = env.classes.get("Both").expect("merged class");
        assert_eq!(def.params, vec!["T".to_string()]);
        let sigs = def.operators.get("add").expect("operator carried over");
        assert_eq!(sigs.len(), 1);
        assert_eq!(
            sigs[0].input,
            Some(Ty::Named("T".to_string())),
            "the re-declaration's operator input must be renamed onto the \
             canonical `T`, not left as the stale `U`"
        );
        assert_eq!(
            sigs[0].result,
            Ty::Named("T".to_string()),
            "the re-declaration's operator result must be renamed onto the \
             canonical `T`, not left as the stale `U`"
        );
    }

    #[test]
    fn a_same_file_renamed_redeclaration_substitutes_its_own_operators_type_arguments() {
        // Round 4 review finding 3 (`absorb_block` seam): the same shape as
        // above, but both declarations live in one file, so the fix must
        // apply on the in-file fold independently of `merge_file_types`
        // (matching `a_same_file_renamed_redeclaration_substitutes_its_parents_type_arguments`'s
        // pairing for the parent-args rename).
        let env = env_of(
            "\
---@class Both<T>
---@field n T
---@class Both<U>
---@operator add(U): U
",
        );
        let def = env.classes.get("Both").expect("declared class");
        assert_eq!(def.params, vec!["T".to_string()]);
        let sigs = def.operators.get("add").expect("operator carried over");
        assert_eq!(sigs.len(), 1);
        assert_eq!(
            sigs[0].input,
            Some(Ty::Named("T".to_string())),
            "the re-declaration's operator input must be renamed onto the \
             canonical `T`, not left as the stale `U`"
        );
        assert_eq!(
            sigs[0].result,
            Ty::Named("T".to_string()),
            "the re-declaration's operator result must be renamed onto the \
             canonical `T`, not left as the stale `U`"
        );
    }

    #[test]
    fn a_diamond_reaching_one_ancestor_through_two_bindings_keeps_both() {
        // F39: the recursion guard used to key purely on the ancestor's
        // *name*, so once a first parent's visit to a shared ancestor
        // claimed the guard, a second parent's edge into that SAME ancestor
        // was dropped outright — even when it binds the ancestor's
        // parameters differently. `C : A, B` where `A : Base` (bare, `U`
        // unbound) and `B : Base<number>` (bound): before the fix, whichever
        // parent is listed *first* is the only one ever actually visited (the
        // other's binding vanishes silently), so `C.item` tracks declaration
        // position instead of each parent's own binding.
        //
        // The two orderings below are an accepting/rejecting pair for the
        // same fix: post-fix, `collect_class` merges fields sibling-parent by
        // sibling-parent with the later one overwriting (mirroring "own
        // overrides inherited"), so the *last-listed* parent's binding wins —
        // deterministic, and different in each direction from what the
        // pre-fix "first parent claims the guard" behaviour produces.
        let listed_second_wins = env_of(
            "\
---@class Base<U>
---@field item U
---@class A : Base
---@class B : Base<number>
---@class C : A, B
",
        );
        let shape = listed_second_wins.class_shape("C").expect("declared class");
        assert_eq!(
            shape.fields["item"].ty,
            Ty::Number,
            "B's explicit `Base<number>` binding must reach C.item; pre-fix this reads as the free `U` (A's bare visit claims the guard first)"
        );

        let swapped = env_of(
            "\
---@class Base<U>
---@field item U
---@class A : Base
---@class B : Base<number>
---@class C : B, A
",
        );
        let swapped_shape = swapped.class_shape("C").expect("declared class");
        assert_eq!(
            swapped_shape.fields["item"].ty,
            Ty::Named("U".to_string()),
            "A's bare visit must still reach C.item when listed second; pre-fix this reads `number` (B's bound visit claims the guard first)"
        );
    }

    /// A k-level "every level forks into two siblings that reconverge"
    /// diamond, matching round 4 review R1's benchmark shape ("one file,
    /// three declarations per level"): `L{i}A` and `L{i}B` both extend
    /// `L{i-1}C`, and `L{i}C` extends both. No generics — the plain
    /// bare-inheritance case already trips the guard-vs-memo defect; a
    /// generic ancestor is F39's separate, still-covered concern.
    fn diamond_source(k: usize) -> String {
        use std::fmt::Write as _;

        let mut src = String::from("---@class L0\n---@field item number\n");
        // Level 1 hangs off the bare base `L0`; every later level hangs off
        // the *previous level's converged class* `L{i-1}C` — the actual
        // diamond, not a chain of unrelated single-parent classes.
        let mut prev = "L0".to_string();
        for i in 1..=k {
            let _ = write!(
                src,
                "---@class L{i}A : {prev}\n---@class L{i}B : {prev}\n---@class L{i}C : L{i}A, L{i}B\n"
            );
            prev = format!("L{i}C");
        }
        src
    }

    /// A k-level diamond that, unlike [`diamond_source`], genuinely
    /// disagrees at every level: `A{i}<T> : A{i-1}<T>, A{i-2}<number>` — one
    /// edge passes the caller's own type parameter through, the other
    /// hard-codes `number` for the grandparent, so `A{i-2}` is reached with
    /// two different bindings at every level once the top-level reference's
    /// own argument is not itself `number`. Round 5 review N1:
    /// `DiamondGuard`'s guard only re-walks a repeat when *something* has
    /// disagreed with it since it was last resolved — [`diamond_source`]'s
    /// shape never disagrees (every binding is the same empty `args`), so it
    /// cannot exercise that path at all, and a round 5 regression that made
    /// the guard re-walk far more than it needs to (reported:
    /// unconditionally, on any disagreement anywhere in the whole call, not
    /// just since a key's own last resolution) went uncaught by every cost
    /// assertion in this file.
    ///
    /// The disagreeing edge points at the **grandparent** rather than the
    /// parent (PR #61 finding 6). This fixture used to write both edges to
    /// the same name — `A{i}<T> : A{i-1}<T>, A{i-1}<number>` — which stopped
    /// being a diamond at all once a single declaration's parent list began
    /// deduping by name like every other duplicate-declaration axis: the two
    /// edges collapsed to the first and the whole hierarchy degenerated to a
    /// plain chain with nothing to disagree about (measured: zero cost trips
    /// at any k). Naming two *different* ancestors restores the identical
    /// pathology through edges the dedupe cannot merge.
    ///
    /// Measured on the new shape, `DiamondGuard::resolutions` at the
    /// uncapped top-level query: 6 at k=5, 36 at k=10, 171 at k=20 — exactly
    /// `(k-1)(k-2)/2`, i.e. the old shape's own `k(k-1)/2` evaluated one
    /// level lower, so every closed form and table this fixture backs holds
    /// unchanged with k shifted by one.
    fn conflicting_diamond_source(k: usize) -> String {
        use std::fmt::Write as _;

        let mut src = String::from("---@class A0<T>\n---@field item T\n---@class A1<T> : A0<T>\n");
        for i in 2..=k {
            let _ = writeln!(
                src,
                "---@class A{i}<T> : A{parent}<T>, A{grandparent}<number>",
                parent = i - 1,
                grandparent = i - 2
            );
        }
        src
    }

    #[test]
    fn a_k_level_diamond_with_conflicting_bindings_resolves_in_bounded_time() {
        // Round 5 review N1/N36: the ignored `diamond_perf_sweep` below is
        // the only cost assertion for this walk, and its `diamond_source`
        // fixture cannot reach a genuine binding conflict (see
        // `conflicting_diamond_source`'s doc comment) — so nothing in this
        // suite, run by CI or otherwise, could fail on the exponential N1
        // measured (0.17s at k=20, 32s at k=20 on the shipped round-5 guard
        // — over an unbounded 2.0-2.4x per level). This test is exactly
        // that missing assertion, using the fixture that actually reaches
        // the guard's conflict-driven re-walk path, at a k CI can afford
        // (k=60, matching `a_k_level_diamond_over_a_shared_ancestor_...`'s
        // own k) with the same background-thread-plus-timeout shape so a
        // regression back to exponential times out the test rather than
        // hanging the suite.
        let k = 60;
        let source = conflicting_diamond_source(k);
        let shape = run_bounded(
            "a k=60 conflicting diamond's resolution — an exponential regression would not \
             return in this process's lifetime",
            move || {
                let env = env_of(&source);
                env.class_shape_bound(&format!("A{k}"), &[Ty::String])
            },
        )
        .expect("the top-level diamond class is declared");
        // Termination and a non-corrupted shape is what this test is for;
        // the guard's *correctness* on conflicting bindings (which binding
        // wins) is pinned by `duplicate_class_merge.rs`'s dedicated
        // fixtures.
        assert!(
            matches!(shape.fields["item"].ty, Ty::Number | Ty::String),
            "resolution must still produce a real field type, not a corrupted shape: {:?}",
            shape.fields["item"].ty
        );
    }

    #[test]
    fn a_deep_single_parent_chain_does_not_overflow_a_pinned_stack() {
        // Round 5 review N2's ORIGINAL fix (the `DiamondGuard` rewrite this
        // comment used to describe) only pushed the survivable floor from
        // ~29-31k to ~40-50k on a 16 MiB pinned stack — the walk was still
        // unbounded, so a long enough chain always found a floor to fall
        // through. `MAX_ANCESTRY_DEPTH` is N2's DURABLE fix: `DiamondGuard`
        // now refuses to recurse past a fixed depth at all (see its doc
        // comment for the measured reasoning), so "does this depth overflow
        // the stack" is no longer the right question for ANY chain length —
        // see `a_class_chain_at_the_ancestry_limit_resolves_fully_and_correctly`
        // for the floor side (a chain within the limit still resolves
        // completely and correctly) and
        // `a_class_chain_past_the_ancestry_limit_does_not_crash` for the
        // ceiling side (a chain past it is refused cleanly, on any stack,
        // not just a pinned 16 MiB one).
        let n = MAX_ANCESTRY_DEPTH - 1;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let env = env_of(&src);
                let shape = env
                    .class_shape(&format!("C{n}"))
                    .expect("the deepest class is declared");
                assert_eq!(shape.fields["item"].ty, Ty::Number);
            })
            .expect("spawn probe thread")
            .join()
            .expect("must not overflow an 8 MiB stack at this depth");
    }

    #[test]
    fn a_class_chain_at_the_ancestry_limit_resolves_fully_and_correctly() {
        // The floor side of `MAX_ANCESTRY_DEPTH` (round 5 review N2's
        // durable fix): a chain of EXACTLY the limit's class count (C0
        // through C(MAX_ANCESTRY_DEPTH - 1), `MAX_ANCESTRY_DEPTH` classes
        // total) must resolve completely — the guard trips only on the
        // class that would push `on_path` PAST the limit, not on the one
        // that reaches it exactly. A regression here (the guard firing one
        // class too early) would silently truncate real, legitimately-sized
        // hierarchies — "a limit that fires on real code is a worse bug
        // than the crash it guards against".
        let n = MAX_ANCESTRY_DEPTH - 1;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        let env = env_of(&src);
        let shape = env
            .class_shape(&format!("C{n}"))
            .expect("the deepest class is declared");
        assert_eq!(
            shape.fields["item"].ty,
            Ty::Number,
            "a chain within the limit must still merge its root ancestor's field"
        );
        assert!(
            env.take_depth_limit_hits().is_empty(),
            "a chain within the limit must not record a depth-limit hit"
        );
    }

    #[test]
    fn a_class_chain_one_class_past_the_ancestry_limit_already_truncates() {
        // The other half of the same boundary, pinned tight enough to catch
        // an off-by-one in `DiamondGuard::should_apply`'s
        // `self.on_path.len() >= MAX_ANCESTRY_DEPTH` check (`>=` weakened to
        // `>` would fire one class late). The test above proves the LARGEST
        // chain that must resolve fully sits at `MAX_ANCESTRY_DEPTH` classes
        // (C0..C(MAX_ANCESTRY_DEPTH - 1)); this proves the SMALLEST chain
        // that must already report truncation is exactly one class more —
        // C0..C(MAX_ANCESTRY_DEPTH), `MAX_ANCESTRY_DEPTH + 1` classes total.
        // Neither existing test pins this: the floor test above sits exactly
        // AT the boundary from the resolving side, and
        // `a_class_chain_past_the_ancestry_limit_does_not_crash` below uses
        // n=50,000 — 250x past the boundary, so shifting the trip point by
        // one class in either direction changes nothing it asserts. Only a
        // chain exactly one class past the true limit can tell `>=` and `>`
        // apart.
        let n = MAX_ANCESTRY_DEPTH;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        let env = env_of(&src);
        let shape = env
            .class_shape(&format!("C{n}"))
            .expect("the class is declared, even though its ancestry is truncated");
        assert!(
            !shape.fields.contains_key("item"),
            "one class past the limit must already truncate before reaching C0's field"
        );
        assert_eq!(
            env.take_depth_limit_hits(),
            vec![format!("C{n}")],
            "the query root must be recorded as a depth-limit hit exactly one class past the boundary"
        );
    }

    #[test]
    fn class_ancestry_truncated_survives_take_depth_limit_hits_draining() {
        // Round 6 review M13: `class_ancestry_truncated` used to peek the
        // same `Mutex` `take_depth_limit_hits` drains, so a class the check
        // path had already reported (and therefore drained) silently
        // stopped reading as truncated for every later query — even though
        // its shape is still, durably, incomplete. Reproduces the exact
        // sequence `crate::check::run` exercises: resolve the truncated
        // class (trips the guard), drain the report queue once (as
        // `report_depth_limit_hits` does at the end of one file's check),
        // then ask the durable peek again — it must still say truncated.
        let n = MAX_ANCESTRY_DEPTH;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        let env = env_of(&src);
        let root = format!("C{n}");
        env.class_shape(&root)
            .expect("the class is declared, even though its ancestry is truncated");
        assert!(
            env.class_ancestry_truncated(&root),
            "must read truncated immediately after resolution trips the guard"
        );
        let drained = env.take_depth_limit_hits();
        assert_eq!(
            drained,
            vec![root.clone()],
            "the report queue must drain the hit exactly once"
        );
        assert!(
            env.class_ancestry_truncated(&root),
            "a durable peek must still say truncated after the one-shot report queue has \
             drained — the class's shape is no less incomplete just because it was already \
             reported once"
        );
    }

    #[test]
    fn a_self_referential_class_cycle_is_recorded_for_reporting() {
        // Round 6 review M67: `---@class A : A` resolves silently — the true-
        // cycle guard keeps the walk from looping forever, so `A`'s own
        // field still resolves fine, but nothing said the cyclic `: A` parent
        // was ignored. `DiamondGuard::should_apply`'s cycle branch now
        // records the name it refused, and `note_cyclic`/
        // `take_cyclic_class_hits` surface it the same way the depth-limit
        // guard's own hits are surfaced — proving the resolver-side half of
        // this finding without needing the (out-of-scope) diagnostic wired
        // up yet.
        let env = env_of(
            "\
---@class A : A
---@field item number
",
        );
        let shape = env.class_shape("A").expect("declared class");
        assert_eq!(
            shape.fields["item"].ty,
            Ty::Number,
            "the class's own field must still resolve despite the cyclic parent"
        );
        assert_eq!(
            env.take_cyclic_class_hits(),
            vec!["A".to_string()],
            "a self-referential parent must be recorded as a true-cycle hit"
        );
    }

    #[test]
    fn a_mutual_class_cycle_is_recorded_for_reporting() {
        // The other half of M67: `A : B` / `B : A` behaves identically to
        // the self-referential case — both classes' own fields resolve, and
        // the cycle back to the query root is recorded once resolution
        // reaches it.
        let env = env_of(
            "\
---@class A : B
---@field a number
---@class B : A
---@field b string
",
        );
        let shape = env.class_shape("A").expect("declared class");
        assert_eq!(shape.fields["a"].ty, Ty::Number, "A's own field resolves");
        assert_eq!(
            shape.fields["b"].ty,
            Ty::String,
            "B's field resolves too, reached before the walk loops back onto A"
        );
        assert_eq!(
            env.take_cyclic_class_hits(),
            vec!["A".to_string()],
            "querying A's shape must record the cycle the walk loops back into"
        );
    }

    #[test]
    fn a_cycle_reached_through_an_innocent_subclass_is_attributed_to_the_cyclic_classes_not_the_query_root()
     {
        // Round 6 review M53: `note_cyclic` used to record the QUERY ROOT
        // (mirroring `note_depth_limit`), which is wrong for this shape —
        // `D` itself is not cyclic, it merely extends `C`, and `C : B` / `B
        // : C` is the actual cycle. Recording `D` would point a future
        // diagnostic at an innocent class and say nothing about the two
        // that actually loop. `note_cyclic` now records every name
        // `DiamondGuard::cyclic` collected — the class(es) the walk actually
        // found already on its path — so querying `D`'s shape must report
        // `C` (or `B`, whichever the walk re-visits first), never `D`.
        let env = env_of(
            "\
---@class D : C
---@class C : B
---@field c string
---@class B : C
---@field b string
",
        );
        env.class_shape("D").expect("declared class");
        let hits = env.take_cyclic_class_hits();
        assert!(
            !hits.contains(&"D".to_string()),
            "the query root is not itself cyclic and must not be reported: {hits:?}"
        );
        assert!(
            hits.contains(&"C".to_string()) || hits.contains(&"B".to_string()),
            "one of the two classes that actually form the cycle must be reported: {hits:?}"
        );
    }

    /// A global allocator that counts every allocation made by the
    /// **current thread only** — via a `thread_local` counter, so
    /// concurrently-running tests on other threads (the default under
    /// `cargo test`) never pollute a reading taken on this thread. Exists
    /// solely to prove production readiness review F3: `should_apply`'s
    /// true-cycle branch used to `cyclic.insert(name.to_string())`
    /// unconditionally on every refusal of an already-recorded name — a
    /// fresh heap allocation per repeat, on what can be a per-keystroke LSP
    /// walk over an unresolved cycle. Scoped to `#[cfg(test)]`, so it never
    /// becomes this crate's allocator for any real caller — only for
    /// `cargo test -p luabox-types`'s own test binary.
    #[cfg(test)]
    #[allow(unsafe_code)]
    mod alloc_probe {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        thread_local! {
            static COUNT: Cell<u64> = const { Cell::new(0) };
        }

        pub(super) struct CountingAlloc;

        // SAFETY: every method delegates straight to `System`, which is
        // itself a valid `GlobalAlloc`; the only addition is a same-thread
        // counter increment around the delegate call, which touches no
        // allocator state and cannot violate any of `GlobalAlloc`'s safety
        // requirements on its own. This crate otherwise denies
        // `unsafe_code` (workspace `Cargo.toml`); `GlobalAlloc` cannot be
        // implemented without it, so this module — test-only, and the sole
        // reason F3's allocation-count proof needs `unsafe` at all — opts
        // back in explicitly rather than weakening the crate-wide lint.
        unsafe impl GlobalAlloc for CountingAlloc {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                COUNT.with(|c| c.set(c.get() + 1));
                unsafe { System.alloc(layout) }
            }
            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        pub(super) fn current_thread_alloc_count() -> u64 {
            COUNT.with(Cell::get)
        }
    }
    #[cfg(test)]
    #[global_allocator]
    static ALLOC_PROBE: alloc_probe::CountingAlloc = alloc_probe::CountingAlloc;

    #[test]
    fn a_repeat_cycle_refusal_of_an_already_recorded_name_allocates_nothing() {
        // Production readiness review F3: the true-cycle branch of
        // `should_apply` refuses every visit to a name already on `on_path`
        // — the common case is the SAME back-edge hit over and over on a
        // walk that keeps re-entering an unresolved cycle (an LSP re-running
        // inference on every keystroke, say). Only the FIRST such refusal of
        // a given name is new information for `cyclic`; every later repeat
        // must find it via a borrowed `contains` lookup and skip the
        // allocating `insert` entirely.
        let mut guard = DiamondGuard::default();
        guard.on_path.insert("A".to_string());

        // Prime: the first refusal of "A" is a genuine first-ever insert —
        // it MAY allocate (a fresh `String`, and possibly the `HashSet`'s
        // own first backing table). Excluded from the measured delta below
        // on purpose: this test is about REPEATS, not the unavoidable
        // one-time cost of recording a name for the first time.
        assert!(guard.should_apply("A", &[]).is_none());
        assert_eq!(guard.cyclic.len(), 1);

        let before = alloc_probe::current_thread_alloc_count();
        for _ in 0..999 {
            assert!(
                guard.should_apply("A", &[]).is_none(),
                "a true cycle back-edge must always be refused"
            );
        }
        let after = alloc_probe::current_thread_alloc_count();

        assert_eq!(
            guard.cyclic.len(),
            1,
            "repeat refusals of the same name must not grow the set"
        );
        assert_eq!(
            after,
            before,
            "999 repeat refusals of an already-recorded cyclic name allocated {} times; \
             only the FIRST refusal of a given name may allocate",
            after - before
        );
    }

    #[test]
    fn a_class_chain_past_the_ancestry_limit_does_not_crash() {
        // The ceiling side, and the durable half of round 5 review N2: a
        // chain deep enough to abort the process outright under the OLD,
        // unbounded walk (`n` here is 250x `MAX_ANCESTRY_DEPTH`, and the
        // measured crash floors this constant is reasoned against — see its
        // doc comment — never got anywhere close even on a 16 MiB pinned
        // stack) must now resolve WITHOUT recursing anywhere near a stack
        // limit at all.
        //
        // Proven on a deliberately modest 2 MiB stack — well under the
        // 16 MiB every other test in this suite pins, and close to the
        // scale an embedder calling `luabox_lsp::run_stdio` directly gets
        // with no explicit pin of its own (the most exposed caller the
        // finding names, and the same scale `MAX_ANCESTRY_DEPTH`'s own
        // measurement used): if the guard's cap were removed or widened
        // back past where `MAX_ANCESTRY_DEPTH` puts it, this thread
        // overflows — confirmed by hand while picking the constant (a
        // 200-class-per-frame budget this thin does not survive a chain
        // anywhere near 50,000 without the cap).
        let n = 50_000;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let env = env_of(&src);
                let shape = env
                    .class_shape(&format!("C{n}"))
                    .expect("the class is declared, even though its ancestry is truncated");
                // Truncated, not silently wrong: the root ancestor's field
                // sits far below the depth the guard permits, so it must be
                // ABSENT from the returned shape — a caller reading a
                // truncated shape as if it were complete is exactly the
                // "quietly wrong" outcome the guard exists to avoid papering
                // over. Reporting that fact is `crate::check::run`'s job
                // (see `class_ancestry_depth_limit.rs`); this level only
                // proves the walk terminates safely and knows it happened.
                assert!(
                    !shape.fields.contains_key("item"),
                    "a truncated shape must not silently carry the unreached root's field"
                );
                let hits = env.take_depth_limit_hits();
                assert_eq!(
                    hits,
                    vec![format!("C{n}")],
                    "the query root must be recorded as a depth-limit hit"
                );
            })
            .expect("spawn probe thread")
            .join()
            .expect(
                "must not overflow a 2 MiB stack at 250x the ancestry limit — \
                 the depth cap must have bounded native recursion long before this",
            );
    }

    #[test]
    fn is_subclass_does_not_reach_further_than_the_shape_walk_does() {
        // Round 6 review M12: `walk_ancestor_names` had no depth cap while
        // `collect_class` (via `DiamondGuard`) truncates at
        // `MAX_ANCESTRY_DEPTH` — so `is_subclass` could approve an upcast to
        // an ancestor `class_shape` had already truncated away, which is
        // exactly the "is_subclass says yes, the merged shape says the
        // field doesn't exist" split the finding describes. A single-parent
        // chain of `MAX_ANCESTRY_DEPTH + 10` classes puts the root, `C0`,
        // 9 classes past where the cap already refuses to recurse further
        // (the boundary tests above pin the cap trips at exactly
        // `MAX_ANCESTRY_DEPTH` classes deep) — `is_subclass` must agree
        // with `collect_class` that `C0` is unreachable, not silently walk
        // further than the shape it is supposed to describe ever does.
        let n = MAX_ANCESTRY_DEPTH + 10;
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        let env = env_of(&src);
        assert!(
            !env.is_subclass(&format!("C{n}"), "C0"),
            "an ancestor `collect_class` truncates away before reaching must not be \
             reported reachable by `is_subclass` either"
        );
    }

    #[test]
    fn class_operators_also_refuses_to_recurse_past_the_ancestry_limit() {
        // `TypeEnv::collect_operators` shares `DiamondGuard`/`should_apply`
        // with `collect_class` verbatim (round 4 review finding 7's shared
        // owner) — proven directly rather than only inferred from
        // `class_shape`'s coverage above: an `---@operator` declared on the
        // chain's root must be reachable through an ordinary chain, and
        // absent (not crashed, not silently kept) once the chain is deep
        // enough to trip the cap.
        let n = 10_000;
        let mut src = String::from("---@class C0\n---@operator add(number): number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let env = env_of(&src);
                let ops = env.class_operators(&format!("C{n}"), "add");
                assert!(
                    ops.is_empty(),
                    "the root's operator sits far below the depth the guard permits, \
                     so it must not be reachable through a truncated walk"
                );
                assert_eq!(
                    env.take_depth_limit_hits(),
                    vec![format!("C{n}")],
                    "the query root must be recorded as a depth-limit hit"
                );
            })
            .expect("spawn probe thread")
            .join()
            .expect("must not overflow a 2 MiB stack — collect_operators must share the cap");
    }

    #[test]
    fn a_k_level_diamond_over_a_shared_ancestor_resolves_in_bounded_time() {
        // Round 4 review R1: the F39 fix (`seen`/`on_path` guards only the
        // currently-expanding path, never memoised) made `collect_class`'s
        // visit count the number of root-to-node PATHS through a diamond,
        // not nodes — O(2^k). Measured on binaries built at both heads, this
        // exact fixture shape: 71 lines (k=22) took 20.77s, 80 lines (k=25)
        // took 144.67s, geometric mean 2.00x per level — on the editor's
        // per-keystroke path, since nothing bounds it (no cache, no depth
        // cap, no fuel). The fix memoises `collect_class` on `(name, args)`
        // within one `class_shape_bound` call: every edge of a diamond that
        // binds its shared ancestor identically (the case this fixture
        // generates — no generics, every binding is the same empty `args`)
        // is expanded once, not once per path, which collapses this exact
        // shape to O(k) — not merely sub-exponential.
        //
        // A regression that drops the memo (or widens the guard back to
        // "ever visited", re-losing F39's per-edge distinction) makes this
        // either hang or return the wrong shape. Running the resolution on
        // a background thread with a bounded `recv_timeout` catches a hang
        // without hanging the test suite itself; the field assertion below
        // catches a wrong-shape regression the timeout alone would not.
        let k = 60;
        let source = diamond_source(k);
        let shape = run_bounded(
            "a k=60 diamond's resolution on the memoised path — an O(2^60) regression would \
             not return in this process's lifetime",
            move || {
                let env = env_of(&source);
                env.class_shape(&format!("L{k}C"))
            },
        )
        .expect("the top-level diamond class is declared");
        assert_eq!(
            shape.fields["item"].ty,
            Ty::Number,
            "the base field must still resolve correctly, not merely terminate"
        );
    }

    /// A [`conflicting_diamond_source`]-shaped group, but with every class
    /// name given `prefix` so several independent groups can share one file
    /// without colliding — the "one file, G independently-conflicting
    /// diamond groups" shape [`MAX_ANCESTRY_RESOLUTIONS`]'s own doc measures
    /// (round 6 review M3): `MAX_ANCESTRY_DEPTH` bounds one group's
    /// recursion depth, but says nothing about the *sum* across groups, so
    /// this is the fixture that actually exercises the total-work budget
    /// rather than the single-group depth cap.
    fn conflicting_diamond_group(prefix: &str, k: usize) -> String {
        use std::fmt::Write as _;

        let mut src = format!(
            "---@class {prefix}A0<T>\n---@field item T\n---@class {prefix}A1<T> : {prefix}A0<T>\n"
        );
        for i in 2..=k {
            let _ = writeln!(
                src,
                "---@class {prefix}A{i}<T> : {prefix}A{}<T>, {prefix}A{}<number>",
                i - 1,
                i - 2
            );
        }
        src
    }

    #[test]
    fn a_k_level_diamond_with_conflicting_bindings_grows_no_worse_than_linearly_with_group_count() {
        // Round 6 review M3: `a_k_level_diamond_with_conflicting_bindings_resolves_in_bounded_time`
        // above is a single wall-clock ceiling on ONE group at k=60 — it
        // asserts nothing about how cost scales with either k or the number
        // of independently-conflicting groups one file can contain, so a
        // regression that keeps each individual group's own resolution
        // O(k²) (removing `MAX_ANCESTRY_RESOLUTIONS`'s cap entirely, or
        // widening it back past any real bound) would still pass it. This
        // test is the missing growth assertion.
        //
        // Deliberately asserted on `DiamondGuard::resolutions` — a
        // deterministic count — rather than wall-clock time: a shared,
        // possibly-contended CI/dev box can make even a fast, correctly-
        // budgeted resolution take an unpredictable amount of *wall* time
        // (this file's own `diamond_perf_sweep`/bounded-time tests already
        // accept that risk for a single, generous ceiling, but a *ratio*
        // between two separately-timed runs is far more exposed to exactly
        // that noise). `resolutions` is what the budget in `should_apply`
        // actually caps, so it is the direct, noise-free way to prove total
        // work grows with group count and not with each group's own k² —
        // see `MAX_ANCESTRY_RESOLUTIONS`'s own doc for the CLI wall-clock
        // numbers this same fixture produces (2.29s → 0.21s for 1 group,
        // 25.28s → 2.13s for 10, budgeted vs not).
        //
        // Wrapped in a spawned thread + bounded `recv_timeout`, the same
        // pattern `a_k_level_diamond_over_a_shared_ancestor_resolves_in_bounded_time`
        // uses below: `MAX_ANCESTRY_RESOLUTIONS`'s own doc comment measures
        // this EXACT fixture — one group alone, k=190 — at >90s uncapped. A
        // regression that stops `resolutions` from ever advancing (`+=`
        // weakening to a no-op, say) does not merely miscount: the
        // assertions below on `resolutions` alone cannot see it either,
        // because a counter stuck at 0 still satisfies
        // `0 <= MAX_ANCESTRY_RESOLUTIONS + 1` and `0 <= per_group_ceiling *
        // groups` — both inequalities a broken cap trivially passes. The
        // timeout is what actually detects "the budget never fires", fast,
        // instead of the whole run either passing on a lie or hanging
        // however long ten uncapped k=190 groups take.
        let k = 190;
        let groups = 10;
        let mut multi_source = String::new();
        for g in 0..groups {
            multi_source.push_str(&conflicting_diamond_group(&format!("G{g}_"), k));
        }

        // The one fixture in this module that genuinely needs more than
        // `BOUND`: ten k=190 groups measure ~5s in a debug build, so a 10s
        // ceiling would be a 2x margin on a contended runner. 30s keeps ~6x
        // and still sits well inside `mutants-gate.sh`'s `--timeout 90`, so
        // this bound fires before cargo-mutants' does (round 11 R11-3).
        let per_group = run_bounded_within(
            std::time::Duration::from_secs(30),
            "10 independently-conflicting k=190 diamond groups, with the total-work budget \
             capping each one (measured ~5s in a debug build) — a single uncapped k=190 \
             group alone measures >90s (MAX_ANCESTRY_RESOLUTIONS's own doc comment), and a \
             resolutions counter that never advances blows past any reasonable timeout \
             rather than merely miscounting",
            move || {
                let env = env_of(&multi_source);
                let mut per_group = Vec::with_capacity(groups);
                for g in 0..groups {
                    let mut shape = TableTy::default();
                    let mut guard = DiamondGuard::default();
                    env.collect_class(
                        &format!("G{g}_A{k}"),
                        &[Ty::String],
                        &mut shape,
                        &mut guard,
                        false,
                    );
                    per_group.push((
                        shape.fields.get("item").map(|f| f.ty.clone()),
                        guard.resolutions,
                    ));
                }
                per_group
            },
        );

        let mut total_resolutions: u64 = 0;
        for (g, (item_ty, resolutions)) in per_group.into_iter().enumerate() {
            assert!(
                matches!(item_ty, Some(Ty::Number | Ty::String)),
                "group {g}'s own resolution must still produce a real field type"
            );
            assert!(
                resolutions <= MAX_ANCESTRY_RESOLUTIONS + 1,
                "group {g} alone must be capped by the total-work budget: {resolutions} \
                 resolutions, budget {MAX_ANCESTRY_RESOLUTIONS}"
            );
            total_resolutions += resolutions;
        }

        // The growth assertion itself: G independently-conflicting groups
        // must cost at most G times one group's own (budget-capped) cost —
        // linear in group count — never something that also grows with k on
        // top of that. A regression that widened or removed the per-group
        // cap would make an individual group's own `resolutions` exceed the
        // per-group assertion above; a regression that capped correctly
        // per-group but somehow let cost accumulate ACROSS groups (sharing
        // one `DiamondGuard`, say) would be caught here instead.
        let per_group_ceiling = MAX_ANCESTRY_RESOLUTIONS + 1;
        assert!(
            total_resolutions <= per_group_ceiling * u64::try_from(groups).unwrap(),
            "total resolutions across {groups} groups ({total_resolutions}) exceeded \
             {groups} × the per-group budget ({per_group_ceiling}) — total work is no \
             longer bounded linearly by group count"
        );
    }

    /// Not run by default — `cargo test -p luabox-types --lib -- --ignored
    /// --nocapture env::tests::diamond_perf_sweep` prints the k-vs-time
    /// tables round 4 review R1 asked for, plus (round 5 review N36) a
    /// `conflicting_diamond_source` sweep — see [`DiamondGuard`]'s doc
    /// comment for that table and what it shows. Kept rather than thrown
    /// away: the next person doubting the memo's flatness can re-run this
    /// instead of re-deriving the fixture from the review transcript. The
    /// `diamond_source` (no-conflict) sweep below this comment never
    /// exercises [`DiamondGuard`]'s conflict/re-walk path at all (every
    /// binding in that fixture is the same empty `args`) — a cost
    /// assertion on it alone cannot see N1's regression, which is why
    /// `a_k_level_diamond_with_conflicting_bindings_resolves_in_bounded_time`,
    /// unlike this test, is *not* `#[ignore]`d.
    ///
    /// The range used to stop at k=100 — round 4 review finding 5 found
    /// that too small to tell `O(k)` from the `Vec`-memo's slower,
    /// super-linear growth apart (both finish in low milliseconds through
    /// k=100, so the claim was unfalsifiable from this table alone).
    /// Extended to k=1600 (release
    /// build; k=3200 stack-overflows on the walk's recursion depth, a
    /// separate, pre-existing limit unrelated to the memo) — measured, this
    /// fixed [`DiamondGuard`]'s `HashSet` memo against a reverted `Vec`-memo
    /// build at the same sizes:
    ///
    ///   k      `HashSet` (this fix)   `Vec` (pre-fix)
    ///   200    3.30ms                 4.39ms
    ///   400    6.53ms                 9.31ms
    ///   800    13.16ms                28.12ms
    ///   1600   30.00ms                60.64ms
    ///
    /// The `HashSet` column roughly doubles per doubling of k (linear). The
    /// `Vec` column grows faster — 2.12x / 3.02x / 2.16x per doubling
    /// (k=200→400, 400→800, 800→1600) — but that is well short of the ~4x a
    /// truly quadratic memo would produce over the same doublings (round 6
    /// review M15: an earlier revision of this comment claimed the gap
    /// between the two columns was "widening"; measured, the `Vec`/`HashSet`
    /// ratio is 2.14x at k=800 and 2.02x at k=1600 — flat to slightly
    /// *shrinking*, not widening). What these four points do show is that
    /// the `Vec` memo is consistently, non-trivially slower than the
    /// `HashSet` one across the whole range, not that the gap is
    /// accelerating. Both totals still include this fixture's O(k)
    /// source-text parse/harvest cost, which is why the `HashSet` column is
    /// not perfectly flat-doubling — the memo lookup itself is O(1)
    /// amortized; the walk around it is inherently O(k).
    #[test]
    #[ignore = "manual measurement, not a CI assertion — see doc comment"]
    fn diamond_perf_sweep() {
        eprintln!("-- conflicting_diamond_source (genuine per-level disagreement) --");
        for k in [8, 12, 16, 18, 20, 22, 24, 30, 40, 60, 80, 100] {
            let source = conflicting_diamond_source(k);
            let start = std::time::Instant::now();
            let env = env_of(&source);
            let shape = env
                .class_shape_bound(&format!("A{k}"), &[Ty::String])
                .expect("declared class");
            let elapsed = start.elapsed();
            assert!(matches!(shape.fields["item"].ty, Ty::Number | Ty::String));
            eprintln!("k={k:<4} lines={:<5} {elapsed:?}", source.lines().count());
        }
        eprintln!("-- diamond_source (no generics, never disagrees) --");
        for k in [16, 18, 20, 22, 24, 25, 40, 60, 80, 100, 200, 400, 800, 1600] {
            let source = diamond_source(k);
            let start = std::time::Instant::now();
            let env = env_of(&source);
            let shape = env.class_shape(&format!("L{k}C")).expect("declared class");
            let elapsed = start.elapsed();
            assert_eq!(shape.fields["item"].ty, Ty::Number);
            eprintln!("k={k:<4} lines={:<5} {elapsed:?}", source.lines().count());
        }
    }

    /// A wide, shallow, **non-conflicting** mixin hierarchy: `width`
    /// independent 5-deep single-inheritance chains (`Base{i}` ->
    /// `Lay{i}_1` -> ... -> `Lay{i}_4`), one `Wide` class inheriting every
    /// chain's deepest link. At `width=120` this is 601 classes total, max
    /// recursion depth 6, and — the property this fixture exists to pin —
    /// **zero diamond conflicts anywhere**: every one of the 601 names is
    /// visited exactly once, by exactly one path. Production readiness
    /// review F1: the OLD `DiamondGuard::resolutions` counting (every
    /// surviving `should_apply` arm, first bindings included) put this
    /// single, entirely legitimate query over any few-hundred budget —
    /// `MAX_ANCESTRY_RESOLUTIONS`'s own doc comment has the measured before/
    /// after numbers this fixture produced through `luabox check` itself.
    fn wide_mixin_source(width: usize) -> String {
        use std::fmt::Write as _;

        let mut src = String::new();
        for i in 0..width {
            let _ = writeln!(src, "---@class Base{i}\n---@field b{i} number");
            for d in 1..=4 {
                let prev = if d == 1 {
                    format!("Base{i}")
                } else {
                    format!("Lay{i}_{}", d - 1)
                };
                let _ = writeln!(src, "---@class Lay{i}_{d} : {prev}");
            }
        }
        let parents: Vec<String> = (0..width).map(|i| format!("Lay{i}_4")).collect();
        let _ = writeln!(src, "---@class Wide : {}", parents.join(", "));
        src
    }

    #[test]
    fn a_wide_shallow_non_conflicting_hierarchy_resolves_cleanly_and_cheaply() {
        // Production readiness review F1's own regression test: before the
        // fix, `DiamondGuard::resolutions` counted this fixture's ~601
        // first-ever-binding visits as if they were re-resolutions, tripping
        // the budget and truncating `Wide`'s shape with a false `LB0317`
        // even though this hierarchy has no diamond conflict anywhere in it
        // (measured, merge base: clean, 0.09s). Re-scoping the counter to
        // only the stale-generation re-resolution arm fixes this by
        // construction — see `wide_mixin_source`'s own doc comment.
        let env = env_of(&wide_mixin_source(120));

        let shape = env
            .class_shape_bound("Wide", &[])
            .expect("Wide is declared");
        assert_eq!(
            shape.fields.len(),
            120,
            "every one of the 120 independent mixin chains must contribute its own field \
             — a truncated walk would merge fewer than all 120"
        );
        assert_eq!(
            shape.fields["b0"].ty,
            Ty::Number,
            "a representative field's type must still resolve correctly, not just be present"
        );
        assert!(
            env.take_depth_limit_hits().is_empty(),
            "a 601-class / depth-6 hierarchy is nowhere near MAX_ANCESTRY_DEPTH"
        );
        assert!(
            env.take_cost_limit_hits().is_empty(),
            "a hierarchy with zero diamond conflicts must never trip the re-resolution \
             budget, regardless of how many classes it declares"
        );
        assert!(
            !env.class_ancestry_truncated("Wide"),
            "Wide's shape must not be recorded as durably truncated either"
        );
    }

    #[test]
    fn a_conflicting_diamond_trips_the_cost_budget_distinctly_from_the_depth_budget() {
        // Production readiness review F1: `LimitTrip` splits what used to be
        // one shared `depth_exceeded: Option<String>` field into `Depth` and
        // `Cost` variants, so a walk that ran out of re-resolution budget
        // without ever nesting anywhere near `MAX_ANCESTRY_DEPTH` reports as
        // `LB0319` (cost), not `LB0317` (depth) — reporting one as the other
        // actively misleads about the fix (flatten the hierarchy vs. remove
        // a conflicting diamond). k=50 gives `49*48/2 = 1176` re-resolutions
        // if run to completion — comfortably past `MAX_ANCESTRY_RESOLUTIONS`
        // (200) — while needing nowhere near 200 levels of `on_path` depth
        // to get there.
        let k = 50;
        let source = conflicting_diamond_source(k);
        let env = env_of(&source);
        let root = format!("A{k}");

        // R1 (production readiness measurement) changed what `env_of` alone
        // already leaves in `take_cost_limit_hits` before the explicit query
        // below ever runs: `conflicting_diamond_source` is self-referential
        // (every `Ai<T>` names `A(i-1)`/`A(i-2)` in its own `parents`), so
        // `TypeEnv::build_from_items`'s discovery pass
        // (`collect_generic_classes`) resolves a template for every one of
        // A0..A50 while constructing `env` — and, since that pass's trips
        // now reach `env`'s own ledger instead of a throwaway discovery env
        // nobody drained (the exact silent-bypass R1 fixed), every class
        // from A22 on (the first `i` whose `(i-1)(i-2)/2` re-resolution
        // count, `MAX_ANCESTRY_RESOLUTIONS`'s own doc comment, exceeds 200:
        // `21*20/2 = 210`, against `20*19/2 = 190` for A21) is already a
        // cost-limit hit before `class_shape_bound` below is ever called.
        // This test's own claim — a pure cost trip drains into
        // `take_cost_limit_hits`, not the depth queue — is unaffected; only
        // the exact membership of the drained set grew from "just the
        // queried root" to "every class construction alone already proved
        // over budget, plus the queried root" (which construction had
        // already included, since A50 is itself past the threshold).
        //
        // The threshold moved A21 → A22 with PR #61 finding 6: a single
        // declaration's parent list now dedupes by name, so this fixture's
        // disagreeing edge had to move from the parent to the GRANDPARENT to
        // stay a diamond at all, which shifts its closed form one level (see
        // `conflicting_diamond_source`'s own doc for the re-measurement).
        // Finding 3's re-scope of trip forwarding — a file surfaces trips
        // only for the generic roots it references — is a no-op *here*, and
        // deliberately asserted as one: this file declares the whole
        // hierarchy itself and names every class from A0 to A49 in some
        // `---@class` parent list, so every over-budget class is a root it
        // references. The cross-file case, where the difference is 30
        // diagnostics against 1, is pinned by
        // `a_consuming_file_surfaces_a_cost_trip_only_for_the_root_it_names`.
        let mut expected_from_construction: Vec<String> =
            (22..=k).map(|i| format!("A{i}")).collect();
        expected_from_construction.sort();

        let shape = env.class_shape_bound(&root, &[Ty::String]);
        assert!(
            shape.is_some(),
            "a cost-budget trip truncates the shape, it does not refuse to produce one"
        );

        let mut hits = env.take_cost_limit_hits();
        hits.sort();
        assert_eq!(
            hits, expected_from_construction,
            "a pure cost trip must drain into take_cost_limit_hits, not the depth queue — \
             expected every class construction's own discovery pass already proved over \
             budget (A22..=A{k}), the queried root (A{k}) among them"
        );
        assert!(
            env.take_depth_limit_hits().is_empty(),
            "this walk never approaches MAX_ANCESTRY_DEPTH — it must not also appear as a \
             depth-limit hit"
        );
        assert!(
            env.class_ancestry_truncated(&root),
            "the shape is genuinely incomplete regardless of which budget truncated it, so \
             the durable truncation record must still be set"
        );
    }

    #[test]
    fn a_cost_trip_is_recorded_as_the_cost_variant_not_the_depth_variant() {
        // Unit-level companion to the end-to-end test above, driving
        // `DiamondGuard` directly: oscillates one name `"X"` through a
        // stream of distinct bindings, each new one invalidating (via a
        // `generation` bump) an earlier one due for a genuine stale
        // re-resolution on its next visit — the exact re-resolution pattern
        // `MAX_ANCESTRY_RESOLUTIONS` bounds — entirely at the top level, so
        // `on_path` never exceeds a handful of entries. Proves `LimitTrip`
        // picks `Cost`, not `Depth`, purely from which budget actually gave
        // out, independent of any real class-hierarchy fixture.
        let mut guard = DiamondGuard::default();
        let v0 = [Ty::Named("V0".to_string())];
        guard.visit("X", &v0, |_, _| {});
        for i in 1..=(MAX_ANCESTRY_RESOLUTIONS + 5) {
            let vi = [Ty::Named(format!("V{i}"))];
            // A new distinct binding: bumps `generation`, staling `v0`'s
            // now-outdated entry — no `resolutions` cost of its own.
            guard.visit("X", &vi, |_, _| {});
            // Re-resolving `v0` here is THE re-resolution this budget
            // counts: `v0` was resolved before, and something else (the
            // line above) has disagreed with `"X"` since.
            guard.visit("X", &v0, |_, _| {});
        }

        assert_eq!(
            guard.limit_exceeded,
            Some(LimitTrip::Cost),
            "oscillating one name past the resolutions budget, with on_path never nested, \
             must trip the COST budget"
        );
        // Attribution asserted the way production asserts it (PR #61 finding
        // 7): `LimitTrip` no longer carries the refused ancestor's name —
        // nothing outside a test ever read it — so the name a trip is filed
        // under is the query root `note_limit_trip` is given, read back off
        // the same ledger `crate::check::report_cost_limit_hits` drains.
        let env = env_of("");
        env.note_limit_trip(&guard, "X");
        assert_eq!(
            env.take_cost_limit_hits(),
            vec!["X".to_string()],
            "a cost trip must file its query root under the COST queue"
        );
        assert!(
            env.take_depth_limit_hits().is_empty(),
            "…and not also under the depth queue"
        );
        assert!(
            guard.on_path.len() < 5,
            "this scenario never nests — a cost trip must not incidentally look like a depth \
             trip by way of a deep on_path"
        );
    }

    #[test]
    fn a_walk_with_exactly_the_resolution_budget_completes_cleanly() {
        // R3 (production readiness issue): the depth budget has an adjacent
        // pair pinning exactly-at-limit (clean) vs one-past (reports) —
        // `class_ancestry_depth_limit.rs`'s
        // `a_chain_exactly_at_the_ancestry_limit_reads_the_root_field_with_no_diagnostics`
        // / `a_chain_one_class_past_the_ancestry_limit_is_reported_at_the_integration_level`,
        // added for round 6 review M64 after exactly this class of
        // regression (`>=` silently weakening from `>`) slipped past every
        // other test. The cost budget had nothing equivalent —
        // `should_apply`'s `if self.resolutions > MAX_ANCESTRY_RESOLUTIONS`
        // could weaken to `>=` and nothing here would notice. A Lua fixture
        // cannot pin this boundary directly: `conflicting_diamond_source(k)`'s
        // resolutions count is the triangular number `k(k-1)/2`
        // (`MAX_ANCESTRY_RESOLUTIONS`'s own doc comment), which never lands
        // on `MAX_ANCESTRY_RESOLUTIONS` for any integer `k` — so, like the
        // test above, this oscillates one name directly through
        // `DiamondGuard::visit` instead, which can hit any exact count.
        let mut guard = DiamondGuard::default();
        let v0 = [Ty::Named("V0".to_string())];
        guard.visit("X", &v0, |_, _| {});
        for i in 1..=MAX_ANCESTRY_RESOLUTIONS {
            let vi = [Ty::Named(format!("V{i}"))];
            // A new distinct binding: bumps `generation`, staling `v0`'s
            // now-outdated entry — no `resolutions` cost of its own.
            guard.visit("X", &vi, |_, _| {});
            // Re-resolving `v0` here is the one re-resolution this
            // iteration counts.
            guard.visit("X", &v0, |_, _| {});
        }

        assert_eq!(
            guard.resolutions, MAX_ANCESTRY_RESOLUTIONS,
            "the oscillation must have driven exactly the budgeted number of \
             re-resolutions, not one more or fewer"
        );
        assert_eq!(
            guard.limit_exceeded, None,
            "exactly MAX_ANCESTRY_RESOLUTIONS re-resolutions must not trip the guard — \
             `should_apply`'s check is `self.resolutions > MAX_ANCESTRY_RESOLUTIONS`, \
             which is false at equality"
        );
    }

    #[test]
    fn a_walk_with_one_more_resolution_than_the_budget_trips_the_cost_limit() {
        // The adjacent case: one more oscillation than the test above — the
        // smallest walk the guard must refuse. Paired with the test above
        // the same way `class_ancestry_depth_limit.rs`'s pair is: the exact
        // transition is now pinned by two adjacent assertions, not one
        // boundary and a gap a `>`-to-`>=` weakening could hide in.
        let mut guard = DiamondGuard::default();
        let v0 = [Ty::Named("V0".to_string())];
        guard.visit("X", &v0, |_, _| {});
        for i in 1..=(MAX_ANCESTRY_RESOLUTIONS + 1) {
            let vi = [Ty::Named(format!("V{i}"))];
            guard.visit("X", &vi, |_, _| {});
            guard.visit("X", &v0, |_, _| {});
        }

        assert_eq!(
            guard.resolutions,
            MAX_ANCESTRY_RESOLUTIONS + 1,
            "the oscillation must have driven exactly one more than the budgeted number \
             of re-resolutions"
        );
        assert_eq!(
            guard.limit_exceeded,
            Some(LimitTrip::Cost),
            "the smallest walk that re-resolves one more than the budget must trip the \
             cost limit — the guard's boundary moved"
        );

        // Wire the trip through to the report queue too — which is also
        // where this test's *attribution* claim now lives (PR #61 finding
        // 7): `LimitTrip` is fieldless, so who a trip is filed under is
        // decided solely by the root `note_limit_trip` is handed. Matching the
        // integration-level assertion the depth-side boundary pair carries
        // (`class_ancestry_depth_limit.rs`'s pair asserts on `LB0317`
        // itself via `check_file`; there is no equivalent Lua fixture here
        // to drive `check_file` with, so this asserts the same wiring one
        // level down, directly against the ledger `report_cost_limit_hits`
        // drains).
        let env = env_of("");
        env.note_limit_trip(&guard, "Root");
        assert_eq!(env.take_cost_limit_hits(), vec!["Root".to_string()]);
        assert!(
            env.take_depth_limit_hits().is_empty(),
            "a pure cost trip at the exact boundary must not also file as a depth hit"
        );
    }

    #[test]
    fn a_diamond_reaching_a_shared_generic_ancestor_with_equal_bindings_carries_one_indexer() {
        // Round 4 review R4: the memo that fixes R1 also collapses the
        // memory half — a diamond over a generic ancestor bound the SAME way
        // on every edge used to carry the identical indexer entry once per
        // edge (2 for one diamond, 4 nested two deep — old head 1 and 1).
        // Distinct bindings still coexist as distinct entries (the diamond
        // below binds `Base`'s `V` to `number` on both edges, so there is
        // exactly one binding to carry); `class_shape_bound`'s overwrite-by-
        // key insert (rather than a bare `Vec::extend`) is what keeps a
        // *differently*-bound diamond (R3, pinned in `duplicate_class_merge.rs`)
        // down to one entry per key too, not two stale ones.
        let env = env_of(
            "\
---@class Base<V>
---@field [string] V
---@class A : Base<number>
---@class B : Base<number>
---@class C : A, B
",
        );
        let shape = env.class_shape("C").expect("declared class");
        assert_eq!(
            shape.indexers,
            vec![(Ty::String, Ty::Number)],
            "one binding reached through two edges must carry one indexer entry, not two"
        );
    }

    #[test]
    fn two_unrelated_parents_with_the_same_indexer_key_keep_the_first_listed_one() {
        // `collect_class`'s indexer merge doc comment (round 5 review N6):
        // two genuinely UNRELATED classes — no shared name, no common
        // ancestor — that each declare their own indexer for the same key
        // resolve first-LISTED-wins, unlike the field merge just above it
        // (always last-processed) and unlike the *other* indexer rule for a
        // shared ancestor reached twice (last-wins, pinned below by
        // `a_shared_generic_ancestor_reached_with_conflicting_bindings_keeps_the_last_listed_one`).
        // The one variable that separates the two rules is identity: `P1`
        // and `P2` here are two distinct classes that happen to pick the
        // same key, not one class reached via two edges — `is_first_binding`
        // is `true` on both of their visits, so the loop's `(Some(_), true)`
        // arm leaves whichever was listed first standing.
        //
        // Both orderings below are an accepting/rejecting pair: swapping
        // which parent is listed first flips the winner, proving the rule is
        // genuinely first-LISTED, not some other order-independent tie-break
        // that happens to agree with one ordering by accident.
        let first_wins = env_of(
            "\
---@class P1
---@field [string] number
---@class P2
---@field [string] string
---@class C1 : P1, P2
",
        );
        let shape = first_wins.class_shape("C1").expect("declared class");
        assert_eq!(
            shape.indexers,
            vec![(Ty::String, Ty::Number)],
            "P1, listed first, must win an unrelated same-key indexer conflict"
        );

        let swapped = env_of(
            "\
---@class P1
---@field [string] number
---@class P2
---@field [string] string
---@class C2 : P2, P1
",
        );
        let swapped_shape = swapped.class_shape("C2").expect("declared class");
        assert_eq!(
            swapped_shape.indexers,
            vec![(Ty::String, Ty::String)],
            "P2, listed first here, must win when the listing order is reversed"
        );
    }

    #[test]
    fn a_shared_generic_ancestor_reached_with_conflicting_bindings_keeps_the_last_listed_one() {
        // The other half of the same indexer-merge doc comment: unlike two
        // unrelated classes above, ONE ancestor (`Base`) reached twice
        // through a diamond with two DIFFERENT type arguments is the same
        // name bound two different ways — `is_first_binding` is `false` on
        // the second (and any later) visit, so the `(Some(slot), false)` arm
        // overwrites, matching the field-merge rule (last-processed wins)
        // instead of the unrelated-parents rule just pinned above. Same
        // shape (two parents feeding one child, one key in conflict),
        // differing only in whether the two edges name the same ancestor —
        // the one variable that actually decides which rule applies.
        //
        // Both orderings again flip the winner, proving this is genuinely
        // last-LISTED and not a coincidence of declaration order.
        let last_wins = env_of(
            "\
---@class Base<V>
---@field [string] V
---@class A : Base<number>
---@class B : Base<string>
---@class D1 : A, B
",
        );
        let shape = last_wins.class_shape("D1").expect("declared class");
        assert_eq!(
            shape.indexers,
            vec![(Ty::String, Ty::String)],
            "B's binding, listed last, must win a shared-ancestor indexer conflict"
        );

        let swapped = env_of(
            "\
---@class Base<V>
---@field [string] V
---@class A : Base<number>
---@class B : Base<string>
---@class D2 : B, A
",
        );
        let swapped_shape = swapped.class_shape("D2").expect("declared class");
        assert_eq!(
            swapped_shape.indexers,
            vec![(Ty::String, Ty::Number)],
            "A's binding, listed last here, must win when the listing order is reversed"
        );
    }

    #[test]
    fn merging_never_lets_a_carrier_attachment_shadow_a_declared_field() {
        // `method` is a real `---@field` on the base and only a carrier
        // attachment on the incoming file: the declared field must win.
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Shadow
---@field method string
",
        ));
        let mut incoming = FileTypes::default();
        incoming.classes.insert(
            "Shadow".to_string(),
            ClassDef {
                methods: BTreeMap::from([(
                    "method".to_string(),
                    FieldTy {
                        ty: Ty::Boolean,
                        optional: false,
                    },
                )]),
                ..ClassDef::default()
            },
        );
        env.merge_file_types(&incoming);
        let def = env.classes.get("Shadow").expect("merged class");
        assert_eq!(def.fields["method"].ty, Ty::String);
        assert!(!def.methods.contains_key("method"));
    }

    #[test]
    fn merging_adds_carrier_attachments_the_base_does_not_declare() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@class Attach
---@field declared string
",
        ));
        let mut incoming = FileTypes::default();
        incoming.classes.insert(
            "Attach".to_string(),
            ClassDef {
                methods: BTreeMap::from([(
                    "extra".to_string(),
                    FieldTy {
                        ty: Ty::Boolean,
                        optional: false,
                    },
                )]),
                ..ClassDef::default()
            },
        );
        env.merge_file_types(&incoming);
        let def = env.classes.get("Attach").expect("merged class");
        assert_eq!(def.methods["extra"].ty, Ty::Boolean);
        // Attachments resolve on reads, folded in beside the declared field.
        let shape = env.class_shape("Attach").expect("shape");
        assert_eq!(shape.fields["declared"].ty, Ty::String);
        assert_eq!(shape.fields["extra"].ty, Ty::Boolean);
    }

    #[test]
    fn merging_enums_is_first_wins() {
        let mut env = TypeEnv::default();
        env.merge_file_types(&surface(
            "\
---@enum Color
local Color = { red = 1 }
",
        ));
        env.merge_file_types(&surface(
            "\
---@enum Color
local Color = { blue = 2 }
",
        ));
        let members = env.enum_members("Color").expect("enum merged");
        assert_eq!(
            members,
            vec![("red".to_string(), Ty::NumberLit("1".into()))]
        );
    }

    #[test]
    fn a_same_file_duplicate_enum_declaration_is_also_first_wins() {
        // Round 6 review M53: `absorb_block`'s `Tag::Enum` arm used to
        // `insert` unconditionally, so a same-FILE duplicate `---@enum` was
        // last-wins — the one axis in this block that disagreed with every
        // other same-file duplicate-declaration rule (fields, methods,
        // indexers, visibility, parents, operators, type params all keep
        // the first) and with `merging_enums_is_first_wins` just above,
        // which already pins the identical rule for the CROSS-file seam.
        // "The file boundary does not change the merge" only held here by
        // accident of which seam a duplicate happened to cross.
        let env = env_of(
            "\
---@enum Color
local Color = { red = 1 }
---@enum Color
local Color2 = { blue = 2 }
",
        );
        let members = env.enum_members("Color").expect("enum declared");
        assert_eq!(
            members,
            vec![("red".to_string(), Ty::NumberLit("1".into()))],
            "the first same-file `---@enum Color` declaration must win, matching the \
             cross-file rule and every other duplicate-declaration axis in this file"
        );
    }

    #[test]
    fn a_file_surface_carries_its_enum_definitions() {
        let types = surface(
            "\
---@enum Status
local Status = { ok = 1, err = 2 }
return Status
",
        );
        let def = types.enums.get("Status").expect("enum on the surface");
        assert_eq!(
            def.members.keys().collect::<Vec<_>>(),
            vec!["err", "ok"] // BTreeMap order
        );
        assert!(!types.is_empty());
    }

    // --- `---@enum` table shapes -----------------------------------------

    #[test]
    fn key_mode_enum_takes_its_member_names_as_values() {
        // `---@enum (key)`: the enum's values are its *keys*, whatever the
        // table's values are.
        let env = env_of(
            "\
---@enum (key) Mode
local Mode = { fast = 1, slow = \"two\" }
",
        );
        let members = env.enum_members("Mode").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("fast".to_string(), Ty::StringLit("fast".into())),
                ("slow".to_string(), Ty::StringLit("slow".into())),
            ]
        );
    }

    #[test]
    fn enum_members_with_non_literal_values_stay_unknown() {
        let env = env_of(
            "\
---@enum Computed
local Computed = { a = 1, b = some_call(), c = { nested = true } }
",
        );
        let members = env.enum_members("Computed").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("a".to_string(), Ty::NumberLit("1".into())),
                ("b".to_string(), Ty::Unknown),
                ("c".to_string(), Ty::Unknown),
            ]
        );
    }

    #[test]
    fn enum_on_a_non_table_target_has_no_members() {
        // `---@enum` must annotate a table constructor; anything else yields
        // an empty (but declared) enum rather than a panic or a guess.
        let env = env_of(
            "\
---@enum Empty
local Empty = other_table
",
        );
        assert_eq!(env.enum_members("Empty"), Some(Vec::new()));
    }

    #[test]
    fn boolean_and_nil_enum_members_lower_to_literal_types() {
        let env = env_of(
            "\
---@enum Flags
local Flags = { on = true, off = false, none = nil }
",
        );
        let members = env.enum_members("Flags").expect("enum declared");
        assert_eq!(
            members,
            vec![
                ("none".to_string(), Ty::Nil),
                ("off".to_string(), Ty::BoolLit(false)),
                ("on".to_string(), Ty::BoolLit(true)),
            ]
        );
    }

    // --- carriers & standalone visibility --------------------------------

    #[test]
    fn a_same_file_duplicate_standalone_visibility_is_first_wins_like_cross_file() {
        // Round 6 review M6: `absorb_standalone_visibility` used to write
        // unconditionally, so two standalone visibility blocks for one
        // member in ONE file were last-wins, while `merge_file_types`'s
        // identical cross-file case already routed through `member_wins`
        // and was first-wins — one member kind deciding its own precedence
        // two different ways depending on which side of a file boundary the
        // duplicate landed on. `---@private` is declared first here; a
        // last-wins bug reports `protected`.
        let env = env_of(
            "\
---@class Solo
local Solo = {}
---@private
function Solo:hide() end
---@protected
function Solo:hide() end
",
        );
        let def = env.classes.get("Solo").expect("class declared");
        assert_eq!(
            def.visibility.get("hide"),
            Some(&FieldScope::Private),
            "the FIRST standalone visibility declaration must win, matching \
             merge_file_types's cross-file rule for the identical duplicate"
        );
    }

    #[test]
    fn a_same_file_duplicate_carrier_method_is_first_wins() {
        // Round 6 review M7: `absorb_carrier_members` used to write
        // unconditionally, so a package declaring one carrier member twice
        // in ONE file published the LAST signature — the identical defect
        // as M6, one member kind over. Two `function Widget.get()`
        // attachments with different `---@return` types; only a real
        // first-wins rule keeps the first one's `string`.
        let (env, _) = TypeEnv::build_ambient(&[{
            let source = "\
---@class Widget
local M = {}
---@return string
function M.get() end
---@return number
function M.get() end
";
            let parsed = parse(source, Dialect::Lua54);
            assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
            let items = luacats::harvest(&parsed);
            (parsed, items)
        }]);
        let def = env.classes.get("Widget").expect("class declared");
        let Some(FieldTy {
            ty: Ty::Function(sig),
            ..
        }) = def.methods.get("get")
        else {
            panic!("`get` must be folded in as a carrier method: {def:?}");
        };
        assert_eq!(
            sig.returns,
            vec![Ty::String],
            "the FIRST carrier attachment's signature must win, not the last"
        );
    }

    #[test]
    fn a_global_assignment_carrier_maps_back_to_its_class() {
        // The carrier variable need not share the class name, and `Zoo = {}`
        // (a global assignment, not a `local`) is a carrier just the same: the
        // standalone `---@private` on `Zoo.hide` has to land on `Animal`.
        let env = env_of(
            "\
---@class Animal
Zoo = {}
---@private
function Zoo.hide() end
",
        );
        let def = env.classes.get("Animal").expect("class declared");
        assert_eq!(def.visibility.get("hide"), Some(&FieldScope::Private));
    }

    /// Shockwave round 3: a later `---@class` whose NAME equals an earlier
    /// class's carrier VARIABLE used to clobber the lexical binding, so
    /// `function Animal:speak()` folded onto class `Animal` instead of onto
    /// `Wrapper` — the class the local `Animal` actually carries. Swapping the
    /// two class blocks flipped the verdict; both orders must now agree.
    #[test]
    fn a_carrier_variable_binding_beats_a_same_named_class() {
        let wrapper_first = "\
---@class Wrapper
local Animal = {}

---@class Animal
local Zoo = {}

---@private
function Animal:speak() end
";
        let animal_first = "\
---@class Animal
local Zoo = {}

---@class Wrapper
local Animal = {}

---@private
function Animal:speak() end
";
        for (label, source) in [
            ("Wrapper first", wrapper_first),
            ("Animal first", animal_first),
        ] {
            let env = env_of(source);
            let wrapper = env.classes.get("Wrapper").expect("Wrapper declared");
            let animal = env.classes.get("Animal").expect("Animal declared");
            assert_eq!(
                wrapper.visibility.get("speak"),
                Some(&FieldScope::Private),
                "{label}: `speak` belongs to the class the local `Animal` carries"
            );
            assert!(
                animal.visibility.is_empty(),
                "{label}: nothing attaches to the same-named class: {:?}",
                animal.visibility
            );
        }
    }

    /// The nominal spelling still works when nothing shadows it: a class whose
    /// own name is used as the carrier reference.
    #[test]
    fn a_class_name_is_still_a_carrier_reference_when_unshadowed() {
        let env = env_of(
            "\
---@class Solo
local Solo = {}
---@private
function Solo:hide() end
",
        );
        let def = env.classes.get("Solo").expect("class declared");
        assert_eq!(def.visibility.get("hide"), Some(&FieldScope::Private));
    }

    /// Two carriers with the same variable name: the **last** one wins,
    /// because `function M:only()` names the binding in scope and Lua's
    /// answer for a repeated `local M` is the most recent binding. This is
    /// not a collision rule — it is a lookup, and the project-source
    /// inference path resolves it that way (Shockwave round 4).
    #[test]
    fn the_last_carrier_wins_a_repeated_variable_name() {
        let env = env_of(
            "\
---@class First
local M = {}

---@class Second
local M = {}

---@private
function M:only() end
",
        );
        let first = env.classes.get("First").expect("First declared");
        let second = env.classes.get("Second").expect("Second declared");
        assert_eq!(second.visibility.get("only"), Some(&FieldScope::Private));
        assert!(first.visibility.is_empty(), "{:?}", first.visibility);
    }

    /// The three shapes Shockwave measured against `lua5.4` and against the
    /// project-source path. Each names two carriers `M`; the member belongs
    /// to whichever class the *second* `M` carries, whatever the class names
    /// and whichever carrier form is used.
    #[test]
    fn a_repeated_carrier_variable_follows_the_binding_in_every_shape() {
        let shapes = [
            (
                "two local carriers",
                "\
---@class Alpha
local M = {}

---@class Beta
local M = {}

---@private
function M:only() end
",
                "Beta",
                "Alpha",
            ),
            (
                "the same two, declared the other way round",
                "\
---@class Beta
local M = {}

---@class Alpha
local M = {}

---@private
function M:only() end
",
                "Alpha",
                "Beta",
            ),
            (
                "a local carrier then an assigned one",
                "\
---@class Alpha
local M = {}

---@class Beta
M = {}

---@private
function M:only() end
",
                "Beta",
                "Alpha",
            ),
        ];
        for (label, source, winner, loser) in shapes {
            let env = env_of(source);
            let winner_def = env.classes.get(winner).expect("class declared");
            let loser_def = env.classes.get(loser).expect("class declared");
            assert_eq!(
                winner_def.visibility.get("only"),
                Some(&FieldScope::Private),
                "{label}: `only` belongs to `{winner}`"
            );
            assert!(
                loser_def.visibility.is_empty(),
                "{label}: nothing attaches to `{loser}`: {:?}",
                loser_def.visibility
            );
        }
    }

    #[test]
    fn a_non_name_assignment_target_is_not_a_carrier() {
        // `outer.inner = {}` names no single carrier variable, so the class is
        // declared but no variable maps to it and the standalone tag is
        // dropped rather than misattributed.
        let env = env_of(
            "\
local outer = {}
---@class Nested
outer.inner = {}
---@private
function outer.hidden() end
",
        );
        let def = env.classes.get("Nested").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    #[test]
    fn standalone_visibility_tags_bind_to_dotted_and_assigned_members() {
        let env = env_of(
            "\
---@class Vis
local Vis = {}
---@private
function Vis.hidden() end
---@protected
function Vis:guarded() end
---@package
Vis.internal = 1
function Vis.open() end
",
        );
        let def = env.classes.get("Vis").expect("class declared");
        assert_eq!(def.visibility.get("hidden"), Some(&FieldScope::Private));
        assert_eq!(def.visibility.get("guarded"), Some(&FieldScope::Protected));
        assert_eq!(def.visibility.get("internal"), Some(&FieldScope::Package));
        assert_eq!(def.visibility.get("open"), None);
    }

    #[test]
    fn visibility_tags_on_unaddressable_targets_are_dropped() {
        // Each of these has no `(carrier, member)` pair to bind to, so the
        // tag is discarded rather than misattributed.
        let env = env_of(
            "\
---@class Drop
local Drop = {}
---@private
function bare() end
---@private
Drop = {}
---@private
local shadow = 1
---@private
Drop[1] = 2
",
        );
        let def = env.classes.get("Drop").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    #[test]
    fn a_multi_target_assignment_never_carries_a_visibility_tag() {
        // `a.x, b.y = ...` is deliberately left unwired — the tag would be
        // ambiguous, so nothing is recorded rather than the wrong thing.
        let env = env_of(
            "\
---@class Multi
local Multi = {}
---@private
Multi.a, Multi.b = 1, 2
",
        );
        let def = env.classes.get("Multi").expect("class declared");
        assert!(def.visibility.is_empty(), "{:?}", def.visibility);
    }

    // --- signature reconciliation ----------------------------------------

    #[test]
    fn an_explicit_param_self_survives_reconciliation() {
        // A `:` method's `self` is absent from the AST parameter list, so the
        // explicit `---@param self T` tag has to be re-inserted at position 0
        // rather than dropped as "unmatched".
        let env = env_of(
            "\
---@class Recv
local Recv = {}
---@param self Recv
---@param n number
function Recv:m(n) end
",
        );
        // `:` methods are range-keyed, not registered by dotted name.
        let sig = env
            .fn_sigs
            .values()
            .find(|f| f.params.iter().any(|p| p.name == "n"))
            .expect("method signature");
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.params[0].name, "self");
        assert_eq!(sig.params[0].ty, Ty::Named("Recv".into()));
        assert_eq!(sig.params[1].name, "n");
        assert_eq!(sig.params[1].ty, Ty::Number);
    }

    #[test]
    fn an_ast_vararg_without_a_tag_becomes_unknown_varargs() {
        let env = env_of(
            "\
---@param a number
function spread(a, ...) end
",
        );
        let sig = env.function("spread").expect("signature");
        assert_eq!(sig.varargs, Some(Ty::Unknown));
    }

    #[test]
    fn an_undeclared_ast_parameter_is_optional_unknown() {
        // A parameter with no `---@param` tag must not become a required
        // `unknown` slot — that would reject every call.
        let env = env_of(
            "\
---@param a number
function partial(a, b) end
",
        );
        let sig = env.function("partial").expect("signature");
        assert_eq!(sig.params[1].name, "b");
        assert_eq!(sig.params[1].ty, Ty::Unknown);
        assert!(sig.params[1].optional);
    }

    // --- collect_generic_classes's cheap pre-scan (round 5 review N27) ----

    /// An ambient definition layer whose ONLY declared class is generic —
    /// deliberately built from `Ambient::build` directly rather than
    /// `crate::defs::build_ambient`/`stdlib`, which always merge in the real
    /// stdlib and so would always contribute at least one non-generic class
    /// (`string`'s carrier, file handles, ...) alongside it. That mix is
    /// exactly what let a bug in `ambient_has_generics`'s per-class scan slip
    /// past `tests/generics.rs`'s `ambient_codes` fixtures: they answer "does
    /// ANY class in the ambient have params" correctly by accident, because
    /// the stdlib always supplies a non-generic class to satisfy an inverted
    /// check too. An ambient with nothing BUT a generic class is the only
    /// fixture that tells `def.params.is_empty()` and `!def.params.is_empty()`
    /// apart.
    fn pure_generic_ambient() -> crate::defs::Ambient {
        crate::defs::Ambient::build(&["---@meta\n---@class Box<T>\n---@field value T\n"])
    }

    /// [`collect_generic_classes`]'s pre-scan result for `source` seen
    /// against `ambient` — the ambient-layer counterpart of
    /// [`generic_templates_of`], and the one seam every ambient pre-scan test
    /// below goes through so "what the scan decides" is measured one way.
    fn generic_templates_against(
        source: &str,
        ambient: &crate::defs::Ambient,
    ) -> BTreeMap<String, GenericClass> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly: {source}");
        let items = luacats::harvest(&parsed);
        let root = parsed.syntax();
        let mut decl = Declared::default();
        for name in ambient.env.classes.keys() {
            decl.classes.insert(name.clone());
        }
        for (name, alias) in &ambient.aliases {
            decl.aliases.insert(name.clone(), alias.clone());
        }
        decl.absorb_tags(&items);
        let inline_as = luacats::harvest_inline_as(&parsed);
        collect_generic_classes(
            &items,
            &inline_as,
            &root,
            Some(ambient),
            &decl,
            &TypeEnv::default(),
        )
    }

    #[test]
    fn a_generic_class_declared_only_in_the_ambient_still_instantiates() {
        // The consuming file declares no class of its own (`file_has_generics`
        // is false), so whether `Box<T>`'s template is built at all rests
        // entirely on `ambient_has_generics` correctly seeing that Box HAS
        // parameters. If that scan were inverted (seeing "has an EMPTY
        // params list" instead), an ambient of only-generic classes would
        // read as having none, `collect_generic_classes`'s cheap pre-scan
        // would short-circuit to an empty map, and `Box<number>` would lower
        // to the bare, unbound `Named("Box")` — the field stays the free `T`
        // and a string can no longer be told apart from a number.
        let ambient = pure_generic_ambient();
        let parsed = lua::parse(
            "---@type Box<number>\nlocal b = { value = \"x\" }\n",
            Dialect::Lua54,
        );
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let diags = crate::check_file_with_ambient(
            &parsed,
            "test.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert_eq!(
            diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
            vec!["LB0300"],
            "value bound to `number` by the ambient-only template must reject a string literal"
        );

        // The accepting pair: the same binding, given a conforming literal,
        // must stay clean — proving this is a genuine substitution and not a
        // blanket rejection of ambient-only generic references.
        let parsed_ok = lua::parse(
            "---@type Box<number>\nlocal b = { value = 1 }\n",
            Dialect::Lua54,
        );
        assert_eq!(parsed_ok.errors(), &[], "fixture must parse cleanly");
        let diags_ok = crate::check_file_with_ambient(
            &parsed_ok,
            "test.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert_eq!(
            diags_ok
                .iter()
                .map(|d| d.code.to_string())
                .collect::<Vec<_>>(),
            Vec::<String>::new(),
            "a correctly-typed field must be accepted once Box<number> resolves"
        );
    }

    #[test]
    fn a_file_that_never_names_an_ambient_generic_class_skips_the_discovery_pass() {
        // Round 6 review M56: the pre-scan used to ask "does a generic class
        // exist ANYWHERE" (file or ambient) — one generic class in ambient
        // re-armed the full discovery-and-resolve pass for every file in the
        // project, whether or not that file ever wrote the class's name at
        // all. This file's annotations never mention `Box` in any form (no
        // `Box<...>`, no bare `Box`), so the fixed, file-scoped scan must
        // find nothing to build a template for, regardless of the ambient's
        // own generic class — proven directly against
        // `collect_generic_classes`'s own return value, not just indirectly
        // through a diagnostic outcome.
        let ambient = pure_generic_ambient();
        let templates =
            generic_templates_against("---@type string\nlocal unrelated = \"x\"\n", &ambient);
        assert!(
            templates.is_empty(),
            "a file that never names the ambient's generic class must skip building a \
             template for it: {templates:?}"
        );
    }

    #[test]
    fn a_bare_reference_to_an_ambient_generic_class_still_builds_its_template() {
        // The other half of M56: the fix must not go too far — a file that
        // names the ambient generic class at all, even a BARE reference with
        // no `<Args>`, still needs `generic_classes` populated (`lower_named`
        // checks the same name regardless of whether the reference carries
        // arguments), so the scan must not narrow to "written with `<...>`
        // only".
        let ambient = pure_generic_ambient();
        let templates = generic_templates_against("---@type Box\nlocal b\n", &ambient);
        assert!(
            templates.contains_key("Box"),
            "a bare reference to a generic ambient class must still trigger its template: \
             {templates:?}"
        );
    }

    /// An ambient whose generic class is reachable only THROUGH an alias the
    /// ambient also declares — `Boxed` names `Box<number>`, and a consuming
    /// file that writes `---@type Boxed` names neither `Box` nor anything
    /// else in `generic_names` (PR #61 round-8 F6).
    fn aliased_generic_ambient() -> crate::defs::Ambient {
        crate::defs::Ambient::build(&[
            "---@meta\n---@class Box<T>\n---@field value T\n---@alias Boxed Box<number>\n",
        ])
    }

    #[test]
    fn an_ambient_alias_to_a_generic_class_still_builds_the_template() {
        // PR #61 round-8 F6. The M56 pre-scan matches names in
        // `generic_names` — file-declared generic classes plus ambient ones —
        // and a file whose only generic reference is `---@type Boxed` names
        // none of them. A FILE-declared `---@alias Boxed Box<number>` is
        // covered by accident (its own `Tag::Alias` is in `items`, so the
        // body is scanned like any other tag); an AMBIENT one has no tag in
        // this file to scan, so `referenced` read empty and the whole
        // discovery pass was skipped.
        let templates =
            generic_templates_against("---@type Boxed\nlocal b\n", &aliased_generic_ambient());
        assert!(
            templates.contains_key("Box"),
            "a generic class reachable only through an AMBIENT alias body must still get its \
             template: {templates:?}"
        );
    }

    #[test]
    fn an_ambient_alias_chain_to_a_generic_class_is_followed_to_the_end() {
        // Alias-to-alias: `Outer` -> `Inner` -> `Box<number>`. One level of
        // expansion is not the rule — `expand_alias` follows the whole chain,
        // so the pre-scan that decides whether a template exists for what it
        // lands on must follow it too. Bounded: each alias is expanded at
        // most once, so a cyclic ambient alias cannot spin here.
        let ambient =
            crate::defs::Ambient::build(&["---@meta\n---@class Box<T>\n---@field value T\n\
             ---@alias Inner Box<number>\n---@alias Outer Inner\n"]);
        let templates = generic_templates_against("---@type Outer\nlocal b\n", &ambient);
        assert!(
            templates.contains_key("Box"),
            "an alias chain must be followed to whatever generic sits at the end of it: \
             {templates:?}"
        );
    }

    #[test]
    fn an_ambient_alias_to_nothing_generic_still_skips_the_discovery_pass() {
        // F6's control, and the half that keeps the fix from undoing M56: an
        // ambient alias whose body names no generic class must leave the
        // pre-scan exactly where it was — empty map, discovery pass skipped.
        // Without this, "follow every referenced alias" degrades back into
        // "any project with a generic class anywhere re-absorbs every file".
        let ambient = crate::defs::Ambient::build(&[
            "---@meta\n---@class Box<T>\n---@field value T\n---@alias Plain string\n",
        ]);
        let templates = generic_templates_against("---@type Plain\nlocal p\n", &ambient);
        assert!(
            templates.is_empty(),
            "an alias body that names nothing generic arms nothing: {templates:?}"
        );
    }

    #[test]
    fn an_ambient_alias_to_a_generic_binds_its_member_in_both_directions() {
        // F6 end to end, both error directions through the public entry
        // point. Measured at 825c61c on this ambient: `expand_alias` lowered
        // `Boxed`'s body with an empty template map, so `Box<number>` fell
        // through to an unsubstituted `Ty::Named("Box")` whose `value` is
        // still the free `T`.
        let ambient = aliased_generic_ambient();
        let diagnose = |source: &str| -> Vec<String> {
            let parsed = lua::parse(source, Dialect::Lua54);
            assert_eq!(parsed.errors(), &[], "fixture must parse cleanly: {source}");
            crate::check_file_with_ambient(
                &parsed,
                "test.lua",
                crate::Strictness::Strict,
                Dialect::Lua54,
                Some(&ambient),
            )
            .iter()
            .map(|d| d.code.to_string())
            .collect()
        };
        assert_eq!(
            diagnose("---@type Boxed\nlocal b = { value = \"x\" }\n"),
            vec!["LB0300"],
            "the false NEGATIVE half: `value` is bound to `number` through the alias, so a \
             string literal must be rejected"
        );
        assert_eq!(
            diagnose("---@type Boxed\nlocal b = { value = 1 }\n"),
            Vec::<String>::new(),
            "the false POSITIVE half: the same binding given a conforming literal must stay \
             clean — this is a substitution, not a blanket rejection"
        );
    }

    /// `collect_generic_classes`'s pre-scan result for a source with no
    /// ambient layer — the file-declared half of the M56 scan (see
    /// `pure_generic_ambient`'s siblings above), factored out for the
    /// arm-by-arm regression tests below: each one pins a single recursion
    /// arm inside `collect_type_expr_references`/`collect_tag_references`
    /// with a fixture whose ONLY generic reference sits on that specific
    /// arm, every sibling arm always empty. Drop the arm and such a fixture's
    /// reference is never collected, `referenced` reads empty, and the
    /// discovery pass — and with it the whole `templates` map — is skipped
    /// entirely (CI mutants-pr gate, 2026-08-08: four survivors at
    /// env.rs:3442/3449/3479/3497, none of which any test before this group
    /// could distinguish; those arms were `||` disjunctions then and an `&&`
    /// mutant sufficed to kill each — PR #61 finding 3 turned the scan into
    /// a collector, so the same fixtures now pin dropped *statements*
    /// instead, one per arm, unchanged in what they prove).
    fn generic_templates_of(source: &str) -> BTreeMap<String, GenericClass> {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly: {source}");
        let items = luacats::harvest(&parsed);
        let root = parsed.syntax();
        let mut decl = Declared::default();
        decl.absorb_tags(&items);
        let inline_as = luacats::harvest_inline_as(&parsed);
        collect_generic_classes(&items, &inline_as, &root, None, &decl, &TypeEnv::default())
    }

    /// env.rs:3442:54 — `type_expr_references_any(key, names) ||
    /// type_expr_references_any(value, names)` in the `TableField::Indexer`
    /// arm. `Box` sits only in the indexer's KEY; the value (`string`)
    /// never mentions it, so `&&` reduces the whole arm to `false`.
    #[test]
    fn an_indexer_key_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field data { [Box]: string }
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only as an indexer's KEY type must still trigger its \
             template: {templates:?}"
        );
    }

    /// The paired arm: `Box` sits only in the indexer's VALUE, the key
    /// (`string`) never mentions it.
    #[test]
    fn an_indexer_value_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field data { [string]: Box }
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only as an indexer's VALUE type must still trigger its \
             template: {templates:?}"
        );
    }

    /// PR #61 round-8 F5 — `Tag::Field`'s arm walked `f.ty` and nothing
    /// else, so a generic named in the FIELD KEY itself
    /// (`---@field [Box<number>] string`, `FieldKey::Indexer(TypeExpr)`)
    /// was invisible to the scan. Distinct from the two indexer tests
    /// above: those put the indexer inside `f.ty` as a table literal
    /// (`---@field data { [Box]: string }`), which the `f.ty` walk reaches
    /// anyway — the key of the `---@field` tag itself is a separate
    /// `TypeExpr` on a separate struct field, and it was the only one of
    /// the fourteen `TypeExpr`-carrying sites across every `Tag` payload
    /// that nothing scanned.
    #[test]
    fn a_field_key_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field [Box<number>] string
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in a `---@field`'s own indexer KEY must still trigger \
             its template: {templates:?}"
        );
    }

    #[test]
    fn a_wrong_argument_count_in_a_field_key_alone_still_reports_lb0313() {
        // F5's consequence, end to end: with no template built, `lower_named`
        // falls through to a bare `Ty::Named("Box")` and the LB0313 arity
        // check (`lower.rs:203`, gated on `generic_classes.get(name)`) never
        // runs at all. Measured at 825c61c on this fixture: zero arity
        // errors for `Box<number, string>` against a one-parameter `Box<T>`,
        // where the same reference anywhere else in the file reports one.
        let env = env_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field [Box<number, string>] boolean
",
        );
        let reported: Vec<(&str, usize, usize)> = env
            .arity_errors
            .iter()
            .map(|e| (e.name.as_str(), e.expected, e.got))
            .collect();
        assert_eq!(
            reported,
            vec![("Box", 1, 2)],
            "a wrong-arity generic reference is a wrong-arity generic reference wherever it is \
             written — the field key is not an exemption"
        );
    }

    /// env.rs:3449:16 — the `TypeExprKind::Fun` arm ORs a params scan
    /// against a returns scan. `Box` sits only in the PARAM, the return
    /// (`string`) never mentions it, so `&&` reduces the whole arm to
    /// `false`.
    #[test]
    fn a_fun_params_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field make fun(a: Box): string
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in a `fun` type's PARAMS must still trigger its \
             template: {templates:?}"
        );
    }

    /// The paired arm: `Box` sits only in the RETURN, the param (`string`)
    /// never mentions it.
    #[test]
    fn a_fun_returns_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@field make fun(a: string): Box
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in a `fun` type's RETURNS must still trigger its \
             template: {templates:?}"
        );
    }

    /// env.rs:3479:17 — `Tag::Alias`'s arm ORs the single-line `ty` against
    /// the multiline `members` list. `Box` sits only in `ty` (single-line
    /// form, `members` is empty and so trivially false), so `&&` reduces
    /// the whole arm to `false` regardless of `ty`.
    #[test]
    fn an_alias_ty_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@alias Foo Box
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in an alias's single-line TY must still trigger its \
             template: {templates:?}"
        );
    }

    /// The paired arm: `Box` sits only in a multiline alias's MEMBERS list
    /// (`ty` is `None` for this form and so trivially false).
    #[test]
    fn an_alias_members_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@alias Foo
---| Box
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in an alias's MEMBERS list must still trigger its \
             template: {templates:?}"
        );
    }

    /// env.rs:3497:17 — `Tag::Operator`'s arm ORs the optional `input`
    /// against the always-present `result`. `Box` sits only in `input`, the
    /// result (`number`) never mentions it, so `&&` reduces the whole arm
    /// to `false`.
    #[test]
    fn an_operator_input_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@operator add(Box): number
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in an operator's INPUT must still trigger its \
             template: {templates:?}"
        );
    }

    /// The paired arm: `Box` sits only in the RESULT, the input (`number`)
    /// never mentions it.
    #[test]
    fn an_operator_result_only_reference_to_a_generic_class_is_still_seen() {
        let templates = generic_templates_of(
            "\
---@class Box<T>
---@field value T
---@class Holder
---@operator add(number): Box
",
        );
        assert!(
            templates.contains_key("Box"),
            "a generic class named only in an operator's RESULT must still trigger its \
             template: {templates:?}"
        );
    }

    // --- PR #61 remediation ------------------------------------------------
    //
    // Every test below pins one finding the round-7 adversarial verifier
    // reproduced empirically against `d20cd47`. Each carries the measured
    // before/after in its own comment; none of the numbers here are guesses.

    /// [`env_of`]'s counterpart for a file checked *beneath* a definition
    /// package — the only way to exercise the seam where an ambient
    /// declaration and a file declaration name the same type.
    fn env_with_ambient(source: &str, ambient: &crate::defs::Ambient) -> TypeEnv {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let items = luacats::harvest(&parsed);
        TypeEnv::build_from_items(&parsed, &items, Some(ambient))
    }

    #[test]
    fn a_files_own_enum_declaration_shadows_a_same_named_ambient_one() {
        // Finding 1. `build_from_items` clones `ambient.env.enums` into
        // `env.enums` (:1067) *before* any `absorb_block` runs, so the
        // `Tag::Enum` arm's `member_wins(self.enums.get(..), Duplicate)`
        // (:1897) saw the seeded ambient entry and skipped the file's own
        // declaration entirely — last-wins in reverse, and silent.
        // `---@class` escapes this via `local_classes` (:1686, "the *first*
        // declaration in this file still replaces any ambient class of the
        // same name whole"); enums had no counterpart, contradicting
        // `build_from_items`' own doc (":1039 environment first, so a
        // same-named file declaration shadows them").
        //
        // Measured at d20cd47: `enum_members("Color")` returned the
        // ambient's `[("Red", 1)]`, so user code reading `Color.Green` /
        // `Color.Blue` got two false `LB0300`s on members it had just
        // declared, plus a false ACCEPT of the ambient-only `Color.Red`.
        let ambient =
            crate::defs::Ambient::build(&["---@meta\n---@enum Color\nlocal Color = { Red = 1 }\n"]);
        let env = env_with_ambient(
            "\
---@enum Color
local Color = { Green = 2, Blue = 3 }
",
            &ambient,
        );
        assert_eq!(
            env.enum_members("Color"),
            Some(vec![
                ("Blue".to_string(), Ty::NumberLit("3".into())),
                ("Green".to_string(), Ty::NumberLit("2".into())),
            ]),
            "the file's own `---@enum` must shadow the ambient's whole, exactly as its own \
             `---@class` does — not be discarded as if it were a duplicate of it"
        );
    }

    #[test]
    fn a_same_file_duplicate_enum_declaration_still_keeps_the_first() {
        // The other side of finding 1's fix: shadowing the ambient must not
        // weaken round 6 review M53's same-file rule. Only the *first* local
        // declaration resets the ambient-seeded entry; a second one in the
        // same file is an ordinary duplicate and loses, ambient present or
        // not — both fixtures below assert the identical result.
        let ambient =
            crate::defs::Ambient::build(&["---@meta\n---@enum Color\nlocal Color = { Red = 1 }\n"]);
        let source = "\
---@enum Color
local First = { Green = 2 }
---@enum Color
local Second = { Blue = 3 }
";
        let expected = Some(vec![("Green".to_string(), Ty::NumberLit("2".into()))]);
        assert_eq!(
            env_with_ambient(source, &ambient).enum_members("Color"),
            expected,
            "a same-file duplicate `---@enum` is still first-wins (M53) even once the first \
             one is allowed to shadow the ambient"
        );
        assert_eq!(
            env_of(source).enum_members("Color"),
            expected,
            "the no-ambient control must be unchanged by the shadowing fix"
        );
    }

    /// An ambient whose only generic class is `Box<T>` — [`pure_generic_ambient`]
    /// re-stated for the inline-cast fixtures below, which need the same
    /// "nothing but a generic class" property for the same reason.
    #[test]
    fn an_inline_as_cast_alone_arms_the_generic_template_scan() {
        // Finding 2. The M56 pre-scan (:3562) only walks `item.block.tags`,
        // but `--[[@as T]]` casts are harvested separately
        // (`harvest_inline_as`, :1091) and lowered through the *same*
        // lowerer whose `generic_classes` that scan gates. A file whose only
        // generic reference is an inline cast therefore got an EMPTY
        // template map and lowered `Box<number>` to the bare, unbound
        // `Named("Box")`.
        //
        // Measured at d20cd47: this fixture reported `LB0300 ... found
        // `unknown`` in Strict mode on correct code, and the result flipped
        // the moment any unrelated tag elsewhere in the file happened to
        // name `Box`.
        let ambient = pure_generic_ambient();
        let source = "\
---@param n number
local function take(n) end
local b = value --[[@as Box<number>]]
take(b.value)
";
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let diags = crate::check_file_with_ambient(
            &parsed,
            "test.lua",
            crate::Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
        );
        assert_eq!(
            diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
            Vec::<String>::new(),
            "an inline `--[[@as Box<number>]]` must build Box's template exactly as a \
             `---@type Box<number>` tag does, so `b.value` resolves to `number`: {diags:?}"
        );
    }

    #[test]
    fn an_inline_as_cast_to_a_file_declared_generic_still_reports_a_wrong_argument() {
        // Finding 2's false-NEGATIVE half, and the file-declared (rather
        // than ambient-declared) template: with the scan blind to inline
        // casts, `Box<number>` stayed unbound, `b.value` read as the free
        // `T`/`unknown`, and a genuinely wrong argument was ACCEPTED
        // silently. Warn strictness is what the verifier measured the false
        // negative in, so it is what this pins.
        let source = "\
---@class Box<T>
---@field value T
---@param s string
local function take(s) end
local b = value --[[@as Box<number>]]
take(b.value)
";
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let diags = crate::check_file(&parsed, "test.lua", crate::Strictness::Warn, Dialect::Lua54);
        assert_eq!(
            diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
            vec!["LB0300"],
            "passing `Box<number>`'s `value` where a string is wanted must be reported — a \
             template the scan never built cannot disagree with anything: {diags:?}"
        );
    }

    /// A [`conflicting_diamond_group`]-shaped hierarchy shipped as a
    /// definition package: the "one over-budget ambient hierarchy, many
    /// consuming files" shape finding 3 is about.
    fn costly_generic_ambient(k: usize) -> crate::defs::Ambient {
        let src = format!("---@meta\n{}", conflicting_diamond_group("", k));
        crate::defs::Ambient::build(&[Box::leak(src.into_boxed_str()) as &str])
    }

    /// `absorb_limit_trips` folds FOUR ledgers, and only the two report
    /// queues are roots-scoped; `truncated_classes` and `cyclic_class_hits`
    /// fold unscoped, deliberately (their consumers — leniency and cycle
    /// reporting — need every class the walk actually gave up on, not just
    /// the ones this file names). The two tests below are what kill the
    /// `extend`-becomes-`()` mutant the first properly-judged `mutants-pr`
    /// run surfaced: without the unscoped fold, a cycle or truncation found
    /// only by the discovery walk silently never reaches the real env.
    #[test]
    fn a_cycle_discovered_while_building_a_generic_template_still_reports_lb0318() {
        let ambient = crate::defs::Ambient::build(&[
            "---@meta\n---@class GCycA<T> : GCycB<T>\n---@class GCycB<T> : GCycA<T>\n",
        ]);
        let env = env_with_ambient("---@type GCycA<string>\nlocal x\n", &ambient);
        let hits = env.take_cyclic_class_hits();
        assert!(
            hits.iter().any(|name| name == "GCycA" || name == "GCycB"),
            "a cyclic generic ancestry resolved only by the template discovery walk must \
             still fold its cycle record into the consuming file's env: {hits:?}"
        );
    }

    #[test]
    fn truncation_discovered_while_building_a_generic_template_still_answers_truncated() {
        let k = 50;
        let ambient = costly_generic_ambient(k);
        // thread + recv_timeout, like the k-level group test above: this
        // walk finishes fast ONLY because the resolution budget trips. A
        // budget counter that stops advancing (`+=` weakened to `*=` — 0
        // stays 0 forever) turns this k=50 conflicting group into the
        // round-5 exponential, and an unbounded test then HANGS the whole
        // suite instead of failing — cargo-mutants reads that as a timeout,
        // not a kill.
        let env_truncated = run_bounded("a budget-capped k=50 discovery walk", move || {
            let env = env_with_ambient(&format!("---@type A{k}<string>\nlocal x\n"), &ambient);
            env.class_ancestry_truncated("A22")
        });
        // A22 tripped the budget during discovery but is NOT the referenced
        // root, so the roots-scoped report queues drop it — the unscoped
        // truncation ledger must still answer for it, or member-checking
        // leniency would treat its silently-incomplete shape as complete.
        assert!(
            env_truncated,
            "a class the discovery walk gave up on must read as truncated in the consuming \
             file's env even when it is not the referenced root"
        );
    }

    #[test]
    fn a_consuming_file_surfaces_a_cost_trip_only_for_the_root_it_names() {
        // Finding 3. `collect_generic_classes` resolves a template for
        // EVERY generic class the discovery walk can reach (:3585) and then
        // forwards every trip that walk recorded onto the consuming file's
        // ledger wholesale (:3603 → :2373). One over-budget ambient
        // hierarchy therefore spilled one `LB0319` per over-budget class
        // into every file that named anything generic at all.
        //
        // Measured at d20cd47 with k=50: 30 distinct-message diagnostics
        // (A21..A50 — `i(i-1)/2 > MAX_ANCESTRY_RESOLUTIONS` first holds at
        // i=21), all at `Span(file, 0..0)`, and the CLI's dedupe cannot
        // collapse them because its key is message+file.
        //
        // The bound this pins: a file surfaces a trip only for the roots its
        // OWN annotations reference — the names the pre-scan actually found.
        // A referenced root whose walk trips yields exactly one diagnostic
        // for that root; a deeper ancestor's own trip belongs to whichever
        // file references THAT ancestor.
        let k = 50;
        let ambient = costly_generic_ambient(k);
        // Bounded for the same reason as the truncation test above: with a
        // dead budget counter this walk is the round-5 exponential.
        let hits = run_bounded("a budget-capped k=50 discovery walk", move || {
            let env = env_with_ambient(&format!("---@type A{k}<string>\nlocal x\n"), &ambient);
            env.take_cost_limit_hits()
        });
        assert_eq!(
            hits,
            vec![format!("A{k}")],
            "naming one over-budget root must report that root once, not every over-budget \
             class the discovery walk happened to resolve on the way"
        );
    }

    #[test]
    fn a_file_that_names_nothing_generic_surfaces_no_cost_trip_at_all() {
        // Finding 3's control: the pre-scan's early return (:3569) already
        // skips discovery entirely for such a file, so the flood was never
        // universal — it hit exactly the files that named ANY generic type,
        // which in a project with a generic-heavy defs package is most of
        // them. Pinned so a re-scope of the forwarding rule cannot
        // accidentally start forwarding here instead.
        let ambient = costly_generic_ambient(50);
        let env = env_with_ambient("---@type string\nlocal x\n", &ambient);
        assert!(
            env.take_cost_limit_hits().is_empty(),
            "a file that references no generic class must carry no cost-limit hit"
        );
    }

    #[test]
    fn a_cyclic_ambient_alias_pair_terminates_the_pre_scan() {
        // Round-8 mutants-pr survivor at the alias-following seam
        // (`decl.aliases.contains_key(&found) && expanded.insert(...)`):
        // with `&&` weakened to `||`, `expanded.insert` never runs for a
        // real alias, so a cyclic ambient alias pair re-queues itself
        // forever and the pre-scan never returns. The `expanded` set IS the
        // termination argument; this pins it with a bound so the failure is
        // a fast red, not a hung suite.
        let ambient = crate::defs::Ambient::build(&[
            "---@meta\n---@class LoopBox<T>\n---@field value T\n---@alias LoopA LoopB\n---@alias LoopB LoopA\n",
        ]);
        run_bounded(
            "a cyclic ambient alias pair's pre-scan — each alias expands at most once, which \
             is the termination argument",
            move || {
                let env = env_with_ambient("---@type LoopA\nlocal x\n", &ambient);
                env.take_cost_limit_hits().len()
            },
        );
    }

    #[test]
    fn a_wide_diamond_is_subclass_miss_completes_in_bounded_time() {
        // Round-8 mutants-pr survivor at `walk_ancestor_names`' skip guard
        // (`Some(&previous) if previous <= depth => continue`): with the
        // guard weakened to never match, every re-reach re-expands its whole
        // subtree — answers stay right, cost goes exponential on a diamond,
        // and no assertion can see "slower but correct". The bound IS the
        // assertion: a full MISS over the grandparent-diamond shape pops
        // O(V) nodes with the skip and ~1.6^k without it.
        let src = conflicting_diamond_source(60);
        let is_sub = run_bounded(
            "a k=60 diamond `is_subclass` miss — the expanded_at skip is what makes the walk \
             O(V), not exponential",
            move || {
                let env = env_of(&src);
                env.is_subclass("A60", "NotAnAncestorAtAll")
            },
        );
        assert!(!is_sub, "the probe class is not an ancestor");
    }

    #[test]
    fn a_union_vs_union_upcast_over_a_deep_diamond_completes_in_bounded_time() {
        // Round 11 review R11-5. The bound above exercises ONE env-level
        // `is_subclass`. `assign.rs` calls it per named pair inside a
        // union-vs-union conformance walk with no union-size cap and no
        // result cache, so the worst case is M x K walks, not one — and
        // nothing measured that compounded path. The claim standing in for a
        // measurement was "realistic input is ~1ms; an adversarial deep
        // diamond across wide unions could cost tens of seconds".
        //
        // MEASURED, on the most adversarial shape this file can build — 50
        // value members drawn from the k=60 conflicting diamond against 50
        // target members whose only assignable one is LAST, so neither
        // `all` nor `any` short-circuits and all 2,500 pairs are paid:
        //
        //   release  0.58 s   (2,450 `is_subclass` misses alone: 0.22 s)
        //   debug    7.73 s   (2,450 `is_subclass` misses alone: 2.52 s)
        //
        // So: bounded, not "tens of seconds", and no `is_subclass` memo was
        // added. A per-`Ctx` result cache would not have helped this shape
        // anyway — every one of the 2,500 pairs is DISTINCT, so a memo pays
        // an allocation and a hash per pair and returns no hit, while the
        // realistic input it would also tax is two or three members wide.
        // Note the split, too: the ancestry walks are only a third of the
        // cost; the rest is the named-resolution-plus-structural-compare
        // fallback each miss falls through to, which no `is_subclass` cache
        // touches.
        //
        // The committed fixture is the same shape at 15 x 15 (210 pairs,
        // 0.91 s debug) rather than 50 x 50: the property being pinned is
        // TERMINATION IN POLYNOMIAL TIME — a walk that goes exponential blows
        // any of these bounds by orders of magnitude — and a 7.7 s test would
        // more than double this crate's suite to measure the same fact.
        let mut src = conflicting_diamond_source(60);
        for i in 0..14 {
            use std::fmt::Write as _;
            // Declared, so a miss cannot short-circuit on "an undeclared
            // target name resolves to unknown, therefore assignable", and
            // distinctly shaped, so it cannot pass structurally either.
            let _ = write!(src, "---@class Foreign{i}\n---@field foreign{i} string\n");
        }
        let value = Ty::Union((46..=60).map(|i| Ty::Named(format!("A{i}"))).collect());
        let mut target: Vec<Ty> = (0..14).map(|i| Ty::Named(format!("Foreign{i}"))).collect();
        // The one member every value member IS assignable to, placed LAST so
        // the 14 misses ahead of it are paid once per value member.
        target.push(Ty::Named("A0".to_owned()));
        let target = Ty::Union(target);
        let assignable = run_bounded(
            "a 15x15 union-vs-union upcast over a k=60 conflicting diamond — 210 uncached \
             `is_subclass` walks through `assign.rs`'s conformance path",
            move || {
                let env = env_of(&src);
                crate::assign::assignable(&env, crate::assign::Exactness::Strict, &value, &target)
            },
        );
        assert!(
            assignable,
            "every member of the value union descends from `A0`, the last member of the \
             target union — a false here means the fixture stopped paying the full product"
        );
    }

    #[test]
    fn a_directly_listed_parent_is_a_subclass_even_behind_a_too_deep_sibling() {
        // Finding 4. `walk_ancestor_names` marked a node `seen` (:2799)
        // BEFORE the depth cut (:2811), so a name first *reached* past the
        // cap was permanently poisoned: the walk refused to visit it later
        // even at a depth it would gladly have expanded.
        //
        // `A : C1, X` with a 199-long C-chain ending at X puts X at depth
        // 200 through the chain (exactly `MAX_ANCESTRY_DEPTH`, the first cut
        // depth) and at depth 1 as A's own second parent. The chain is
        // popped first (parents are pushed reversed so the first-listed one
        // pops first), so the deep X wins the race to `seen`.
        //
        // Measured at d20cd47: `is_subclass("A", "X")` was `false` for a
        // parent A lists literally on its own `---@class` line — a spurious
        // "missing marker" `LB0300` — and flipped to `true` merely by
        // swapping the two parents' order.
        let mut src = String::from("---@class X\n---@field marker boolean\n");
        src.push_str("---@class C199 : X\n");
        for i in (1..199).rev() {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i + 1);
        }
        let deep_first = format!("{src}---@class A : C1, X\n");
        let shallow_first = format!("{src}---@class A : X, C1\n");
        assert!(
            env_of(&deep_first).is_subclass("A", "X"),
            "a parent listed directly on A must be reachable regardless of how deep an \
             EARLIER-listed sibling's chain reaches the same name"
        );
        assert!(
            env_of(&shallow_first).is_subclass("A", "X"),
            "the control: listing the same two parents the other way round must give the \
             same answer — an order-dependent `is_subclass` is the bug"
        );
    }

    #[test]
    fn a_near_cap_nodes_whole_subtree_survives_a_later_shallow_re_reach() {
        // PR #61 round-8 F1. Finding 4 moved the depth cut ahead of
        // `seen.insert` for the *popped node*, which fixed the node case
        // (`a_directly_listed_parent_is_a_subclass_even_behind_a_too_deep_sibling`)
        // and left the SUBTREE case open: a node first expanded at depth
        // `MAX-1` is under the cap, so it marks itself `seen` and pushes its
        // own parents at depth `MAX`, where they are dropped. The later
        // SHALLOW re-reach of that same node then hits `seen` and never
        // re-expands — its parents are lost for the whole walk, not just for
        // the over-deep branch.
        //
        // `Z` / `Y : Z` / a 198-link `B` chain ending at `Y` / `A : B1, Y`
        // puts `Y` at depth 199 through the chain (expanded: 199 < 200) and
        // `Z` at 200 (dropped), while `A` lists `Y` itself at depth 1, from
        // which `Z` sits at a perfectly reachable depth 2.
        //
        // Measured at 825c61c: `is_subclass("A", "Z")` was `false` with the
        // chain listed first and `true` on swapping the two parents — an
        // order-dependent answer that `assign.rs`'s nominal upcast turns into
        // a spurious `LB0300` (that path reads `is_subclass` directly and is
        // NOT gated behind `class_ancestry_truncated`, unlike the member-read
        // `LB0306` family).
        use std::fmt::Write as _;

        let mut src = String::from("---@class Z\n---@field marker boolean\n---@class Y : Z\n");
        let _ = writeln!(src, "---@class B198 : Y");
        for i in (1..198).rev() {
            let _ = writeln!(src, "---@class B{i} : B{}", i + 1);
        }
        let deep_first = env_of(&format!("{src}---@class A : B1, Y\n"));
        let shallow_first = env_of(&format!("{src}---@class A : Y, B1\n"));
        assert!(
            deep_first.is_subclass("A", "Y"),
            "the node case (already pinned): `Y` is listed directly on `A`"
        );
        assert!(
            deep_first.is_subclass("A", "Z"),
            "the subtree case: `Z` sits one link BELOW the near-cap `Y`, at depth 2 from `A` \
             through the parent `A` declares itself — an earlier sibling's chain reaching `Y` \
             near the cap must not delete `Y`'s parents from the whole walk"
        );
        assert!(
            shallow_first.is_subclass("A", "Z"),
            "the control: the same two parents the other way round must give the same answer — \
             an order-dependent `is_subclass` is the bug"
        );
    }

    #[test]
    fn a_cycle_on_the_current_path_is_recorded_even_after_a_sibling_tripped_a_budget() {
        // Finding 5. `should_apply`'s sticky `limit_exceeded` early-return
        // (:747) ran BEFORE the `on_path` cycle check (:750), so once any
        // branch tripped a budget the walk stopped *recording* cycles as
        // well as stopping — and there is no per-class query loop to recover
        // the missed `LB0318` later, because resolution is reference-driven.
        //
        // The fixture is the minimal shape that separates the two: `Cyc`'s
        // own parent list names an over-budget generic group FIRST and then
        // `Cyc` itself. By the time the back-edge onto `Cyc` — a name still
        // on the recursion path — is examined, the cost budget has already
        // tripped, so the old order threw the cycle away.
        //
        // Measured at d20cd47: `LB0319` recorded, `LB0318` absent.
        use std::fmt::Write as _;

        let k = 50;
        let mut src = conflicting_diamond_group("", k);
        let _ = writeln!(src, "---@class Cyc : A{k}, Cyc");
        let env = env_of(&src);
        env.class_shape_bound("Cyc", &[])
            .expect("Cyc is declared, however truncated its ancestry");
        assert!(
            env.take_cost_limit_hits().contains(&"Cyc".to_string()),
            "the costly sibling must still trip the cost budget for the query root"
        );
        assert_eq!(
            env.take_cyclic_class_hits(),
            vec!["Cyc".to_string()],
            "a true `---@class` cycle on the current recursion path is real information \
             regardless of whether some unrelated sibling branch has already run out of budget"
        );
    }

    #[test]
    fn one_declarations_repeated_parent_dedupes_exactly_as_two_declarations_do() {
        // Finding 6. `push_parent_ref` (:1020) documents parents as
        // name-keyed FIRST-wins, "matching every other duplicate-declaration
        // axis in this file" — but only the re-declaration path (:1749) went
        // through it. A single declaration's own parent list was pushed raw
        // (:1635), so the same two edges gave opposite answers depending on
        // whether they were written on one line or two, both silently.
        //
        // Measured at d20cd47: one declaration kept BOTH edges and the
        // last-visited one won (`item` = String); two declarations deduped
        // to the first (`item` = Number).
        let base = "---@class Base<T>\n---@field item T\n";
        let one_declaration = env_of(&format!("{base}---@class C : Base<number>, Base<string>\n"));
        let two_declarations = env_of(&format!(
            "{base}---@class C : Base<number>\n---@class C : Base<string>\n"
        ));
        assert_eq!(
            one_declaration.classes["C"].parents, two_declarations.classes["C"].parents,
            "one declaration naming a parent twice is the same edge, resolved the same way, \
             as two declarations naming it once each"
        );
        assert_eq!(
            one_declaration
                .class_shape("C")
                .expect("declared class")
                .fields["item"]
                .ty,
            Ty::Number,
            "first-wins means the FIRST binding of the repeated parent survives"
        );
        assert_eq!(
            two_declarations
                .class_shape("C")
                .expect("declared class")
                .fields["item"]
                .ty,
            Ty::Number,
            "the two-declaration control's answer is the one both forms must agree on"
        );
    }

    #[test]
    fn a_class_naming_itself_twice_is_still_exactly_one_cycle_record() {
        // Finding 6's guard-rail: collapsing a repeated parent must not
        // change how many cycles the walk reports. `---@class A : A, A`
        // records exactly one `LB0318` at d20cd47 (the `cyclic` HashSet
        // already deduped the two back-edges) and must still record exactly
        // one once the second edge no longer exists at all.
        let env = env_of("---@class A : A, A\n---@field x number\n");
        env.class_shape_bound("A", &[]).expect("A is declared");
        assert_eq!(
            env.take_cyclic_class_hits(),
            vec!["A".to_string()],
            "a self-cycle is one cycle however many times the parent list repeats it"
        );
    }

    #[test]
    fn a_truncated_class_stays_lenient_rather_than_agreeing_with_is_subclass() {
        // Finding 8. `walk_ancestor_names`' depth cap (:2811) and
        // `DiamondGuard`'s STICKY `limit_exceeded` are not the same cut:
        // the cap refuses one over-deep branch and carries on with the rest,
        // while the sticky flag refuses every later sibling too. So
        // `C : D210, Shallow` gives an EMPTY merged shape (the D-chain trips
        // first, and `Shallow` is then refused) while `is_subclass(C,
        // Shallow)` stays `true` (the cap skips the over-deep D nodes and
        // still visits `Shallow` at depth 1).
        //
        // Measured at d20cd47, and no diagnostic is wrong today: every
        // consumer of the disagreement is gated behind
        // `class_ancestry_truncated`, which is `true` here. THAT is what
        // this test pins — the leniency, not an agreement that does not
        // exist. `is_subclass_does_not_reach_further_than_the_shape_walk_does`
        // covers the single-parent direction only, where the two agree.
        use std::fmt::Write as _;

        let n = MAX_ANCESTRY_DEPTH + 10;
        let mut src = String::from("---@class Shallow\n---@field s number\n---@class D0\n");
        for i in 1..=n {
            let _ = writeln!(src, "---@class D{i} : D{}", i - 1);
        }
        let _ = writeln!(src, "---@class C : D{n}, Shallow");
        let env = env_of(&src);
        let shape = env.class_shape("C").expect("C is declared");
        assert!(
            !shape.fields.contains_key("s"),
            "the sticky budget refuses `Shallow` after the D-chain trips, so its field is \
             genuinely NOT in the merged shape: {:?}",
            shape.fields
        );
        assert!(
            env.is_subclass("C", "Shallow"),
            "`walk_ancestor_names` skips only the over-deep branch, so it still reaches a \
             directly-listed shallow parent — the two walks disagree by construction"
        );
        assert!(
            env.class_ancestry_truncated("C"),
            "the disagreement is safe only because every diagnostic path reads this and \
             stays lenient — losing the durable truncation record is what would turn it \
             into a false `missing field`"
        );
    }
}

#[cfg(test)]
mod publish_carrier_method_tests {
    use crate::ty::Ty;
    use luabox_syntax::lua::{Dialect, parse};

    /// The published class surface for one module, through the same
    /// `module_surface` entry point the CLI and LSP use — `TypeEnv`'s own
    /// test `surface` helper passes no carrier map, so it never populates
    /// `ClassDef::methods` and cannot see this behaviour at all.
    fn methods_of(source: &str, class: &str) -> crate::ty::FieldTy {
        let parsed = parse(source, Dialect::Lua54);
        assert_eq!(parsed.errors(), &[], "fixture must parse cleanly");
        let types = crate::module_surface(&parsed, "mod.lua", None).types;
        let def = types.classes.get(class).expect("class collected");
        def.methods
            .values()
            .next()
            .expect("carrier method published")
            .clone()
    }

    /// A carrier method with **no** `return` statement must be published
    /// unchanged: `returns` is empty, so there is no inferred return type to
    /// promote, and promoting anyway publishes it as an *annotated* signature
    /// declaring it returns nothing — a stronger claim than the code makes,
    /// turning "inference found nothing" into "this returns no value".
    ///
    /// Measured: the guard is `has_return_annotation || returns.is_empty()`.
    /// With `||` swapped for `&&`, this method is promoted and
    /// `has_return_annotation` flips to `true` on an empty `returns` list.
    /// That mutant survived the full suite before this test existed.
    #[test]
    fn a_carrier_method_with_no_return_statement_is_published_unpromoted() {
        let method = methods_of(
            "\
---@class Silent
local S = {}
function S:noop() end
return S
",
            "Silent",
        );
        let Ty::Function(sig) = &method.ty else {
            panic!("carrier method is a function, got {:?}", method.ty);
        };
        assert!(
            sig.returns.is_empty(),
            "fixture must have no inferred returns: {:?}",
            sig.returns
        );
        assert!(
            !sig.has_return_annotation,
            "a method inference found no return type for must not be published \
             as declaring one"
        );
    }

    /// The control separating the two halves of the same guard, one variable
    /// apart: this body *does* return, so promotion is correct — that is
    /// finding 2's fix, and without it the return type is erased at this seam.
    #[test]
    fn a_carrier_method_with_an_inferred_return_is_published_promoted() {
        let method = methods_of(
            "\
---@class Talker
local T = {}
function T:speak() return \"hi\" end
return T
",
            "Talker",
        );
        let Ty::Function(sig) = &method.ty else {
            panic!("carrier method is a function, got {:?}", method.ty);
        };
        assert!(!sig.returns.is_empty(), "fixture must infer a return type");
        assert!(
            sig.has_return_annotation,
            "an inferred return type must survive publication into the class \
             surface (finding 2)"
        );
    }
}
