// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Property coverage for the duplicate-`---@class` unification invariant
//! (#59): however two declarations of one generic class spell their type
//! parameters, wherever their `---@field` bodies sit, whichever order they
//! appear in, and whether they share a file or not, the merged class's
//! members must monomorphise through the **canonical** parameter list.
//!
//! The #46 family was found shape-by-shape — the reviewer named two shapes,
//! a sweep found a third (cross-file) — because each shape exercised a
//! different merge seam. This generator covers the shape space instead of
//! enumerating it: had it existed, the cross-file variant would have fallen
//! out mechanically.
//!
//! Round 3 review (F77): the first version of this generator emitted no
//! `: Parent<...>` at all and never crossed the `require` boundary, so
//! neither defect family this PR actually fixes — a parent's type argument
//! surviving the positional-unification rename (F38), and an unbound
//! parameter erasing correctly once the class has crossed `require` (F42)
//! — was reachable from it. `Case` now also generates: a parent reference
//! with no argument, with a concrete argument, and with an argument that is
//! the CARRYING declaration's own type parameter (`ParentShape`, below);
//! and, independently, an export that crosses `require` rather than a bare
//! local (`crosses_require`).
//!
//! Running the extended generator against the pre-fix code first surfaced
//! something upstream of either named defect: `collect_generic_classes`
//! (`env.rs`, the table `---@type Name<Args>` monomorphises against at the
//! reference site) built its template from one declaration's own
//! `---@field`s only and never walked `def.parents`, so ANY class's
//! inherited members — duplicated or not — were invisible through a
//! generic `---@type Name<Args>` reference, masking whatever a
//! parent-argument probe would show for F38 underneath it. That is fixed
//! now too — `collect_generic_classes` runs a full `absorb_block` discovery
//! pass and reads the template back off `class_shape`, which does walk
//! parents — so `declared_fields` folds `OwnParam`/`Concrete` into the
//! monomorphising probe below same as any other field, and 8000+ generated
//! cases across several seeds confirm it holds. What remains pinned as a
//! genuine, in-scope-boundary limitation (not asserted here) is
//! `undeclared_members_on_a_generic_reference_stay_lenient_lb0300_not_lb0306`
//! below: a generic reference never carries `Ty::Named` identity at all, so
//! an UNDECLARED member on it reads lenient regardless of this fix.
//!
//! The probes are behavioural, not structural, and each field is probed in
//! **both directions**: a field that monomorphised correctly to `string`
//! passes `want_string` clean AND trips `want_number` with exactly `LB0300`.
//! The positive probe catches the #46 leak itself (an unsubstituted
//! parameter arrives as `found 'U'`, `unknown` as `found unknown` — strict
//! mode rejects both). The negative probe pins the type *exactly*: a field
//! that drifted to a both-ways-assignable type would pass the positive
//! probe silently, and only the negative probe can tell. This is the
//! audited-fixture rule from #58 applied here: every generated case must be
//! able to fail, in each direction separately.

use std::collections::HashMap;

use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{Strictness, check_file_with_requires, module_surface, stdlib_defs};
use proptest::prelude::*;

// F79 (round 3 review): `check`/`surface`/`check_cross` used to be ~40 lines
// defined here near-verbatim identically to `duplicate_class_merge.rs` — now
// one shared module both import. `support::codes` is imported as `check`:
// every call site in this file already wants codes, not full `Diagnostic`s,
// so the alias keeps every one of them unchanged.
mod support;
use support::{check_cross, codes as check, surface};

/// The fixed parent class every `ParentShape` other than `None` inherits
/// from — one non-duplicated declaration, prepended verbatim to whichever
/// source(s) a case needs it in. Prepending it to more than one file (the
/// `cross_file` shape) is itself harmless duplication (#49), not a variable
/// this axis is testing.
const WRAPPED_PREAMBLE: &str = "---@class Wrapped<S>\n---@field slot S\n\n";

/// A parent reference one of this case's declarations can carry (round 3
/// review F77).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParentShape {
    /// No parent reference — the original shape space.
    None,
    /// `: Wrapped`, no argument — `Wrapped`'s own parameter is unbound, so
    /// the inherited field must cross as `unknown` in BOTH directions
    /// (mirrors `cross_file_require.rs`'s
    /// `a_parent_written_bare_leaves_its_parameter_unbound_and_erased`).
    Bare,
    /// `: Wrapped<X>` where `X` is the CARRYING declaration's own first
    /// type parameter — the exact shape `env.rs:784-788` pushes into
    /// `existing.parents` BEFORE the positional rename (`:806-809`) is
    /// computed: when the carrying declaration is not the one whose
    /// parameters won the canonical list, its parameter name must still be
    /// rewritten before it reaches `Wrapped` (F38).
    OwnParam,
    /// `: Wrapped<string>` — a concrete argument, immune to which
    /// declaration's parameter naming wins the canonical slot. The control.
    Concrete,
}

/// Where the `---@field` bodies sit — the axis the wave-21 fixture collapsed
/// (it only covered `First`, the one shape that cannot fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fields {
    First,
    Second,
    Both,
}

/// One generated duplicate-declaration case.
// `swapped`/`cross_file`/`parent_on_second`/`crosses_require` are four
// independent orthogonal shape axes (declaration order, file boundary,
// which declaration carries a parent, and the export/require boundary),
// not state that wants a state machine — a generated `proptest` case, not
// a library type other code constructs.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
struct Case {
    /// Type-parameter names of the first declaration, e.g. `["T", "U"]`.
    first_params: Vec<&'static str>,
    /// Type-parameter names of the second declaration — same arity, possibly
    /// the same names (the control) or different ones (the #46 shape).
    second_params: Vec<&'static str>,
    fields: Fields,
    /// Swap which declaration comes first in source/file order.
    swapped: bool,
    cross_file: bool,
    /// A parent reference one declaration carries — `None` for the original
    /// shape space (F77).
    parent: ParentShape,
    /// Which identity ("first"/"second") carries `parent`, when it is not
    /// `None`. Independent of `swapped`, so the carrying declaration is
    /// sometimes the file-order-first (canonical) declaration and sometimes
    /// not — only the latter exercises F38.
    parent_on_second: bool,
    /// Type `b` via `require("mod")` instead of a bare local — the export
    /// seam F77 named as the third gap. The field probes still bind `b`
    /// through an explicit `---@type` override on the require result (as
    /// `binding_the_type_argument_types_the_required_generic_carrier` in
    /// `cross_file_require.rs` does), which is what keeps this axis's
    /// expected types identical to the non-crossing case while still
    /// exercising the `module_surface` / `with_project_types` /
    /// `check_file_with_requires` seam a `---@type Boxed<...> local b`
    /// never touches. The unbound/erasure boundary itself (F42) is a
    /// hand-written regression below, not this axis.
    crosses_require: bool,
}

impl Case {
    fn arity(&self) -> usize {
        self.first_params.len()
    }

    /// The `: Wrapped<...>` clause `decl` appends for whichever declaration
    /// carries `self.parent` — `None` for `ParentShape::None` and for the
    /// declaration `parent_on_second` does not select.
    fn parent_clause(&self, params: &[&str], first_identity: bool) -> Option<String> {
        if first_identity == self.parent_on_second {
            return None;
        }
        match self.parent {
            ParentShape::None => None,
            ParentShape::Bare => Some(" : Wrapped".to_string()),
            ParentShape::OwnParam => Some(format!(" : Wrapped<{}>", params[0])),
            ParentShape::Concrete => Some(" : Wrapped<string>".to_string()),
        }
    }

    /// The `---@class` block for one declaration. `value`/`second` belong to
    /// the "first" declaration's identity, `extra` to the "second"'s, so a
    /// swap moves the whole block, never re-attributes fields.
    fn decl(&self, params: &[&str], own_fields: bool, first_identity: bool) -> String {
        use std::fmt::Write as _;
        let mut s = format!("---@class Boxed<{}>", params.join(", "));
        if let Some(clause) = self.parent_clause(params, first_identity) {
            s.push_str(&clause);
        }
        s.push('\n');
        if own_fields {
            if first_identity {
                writeln!(s, "---@field value {}", params[0]).unwrap();
                if self.arity() == 2 {
                    writeln!(s, "---@field second {}", params[1]).unwrap();
                }
            } else {
                writeln!(s, "---@field extra {}", params[0]).unwrap();
            }
        }
        s
    }

    /// The declaration blocks in source order.
    fn decls(&self) -> Vec<String> {
        let first = self.decl(
            &self.first_params,
            matches!(self.fields, Fields::First | Fields::Both),
            true,
        );
        let second = self.decl(
            &self.second_params,
            matches!(self.fields, Fields::Second | Fields::Both),
            false,
        );
        if self.swapped {
            vec![second, first]
        } else {
            vec![first, second]
        }
    }

    /// A consumer that instantiates `Boxed` with concrete arguments and
    /// probes one field. `local x = ...` keeps every probe a standalone
    /// program.
    fn consumer(&self, probe: &str) -> String {
        let args = if self.arity() == 2 {
            "Boxed<string, number>"
        } else {
            "Boxed<string>"
        };
        let binding = if self.crosses_require {
            format!("---@type {args}\nlocal b = require(\"mod\")\n")
        } else {
            format!("---@type {args}\nlocal b\n")
        };
        format!(
            "{binding}\
             ---@param s string\nlocal function want_string(s) end\n\
             ---@param n number\nlocal function want_number(n) end\n\
             {probe}\n"
        )
    }

    /// Run one probe against this case's declarations.
    fn run(&self, probe: &str) -> Vec<String> {
        let consumer = self.consumer(probe);
        let preamble = if self.parent == ParentShape::None {
            ""
        } else {
            WRAPPED_PREAMBLE
        };
        if self.crosses_require {
            self.run_cross_require(preamble, &consumer)
        } else if self.cross_file {
            let files: Vec<String> = self
                .decls()
                .into_iter()
                .map(|d| format!("{preamble}{d}\n"))
                .collect();
            check_cross(&files, &consumer)
        } else {
            let src = format!("{preamble}{}\n{consumer}", self.decls().join("\n"));
            check(&src)
        }
    }

    /// Cross the export/`require` boundary (#56): `Boxed` is this case's
    /// module export, required by a separate consumer file, so the seam a
    /// merge bug can only reach once a type has been reified through
    /// `module_surface` has somewhere in the generated space to hide.
    fn run_cross_require(&self, preamble: &str, consumer_body: &str) -> Vec<String> {
        let base = stdlib_defs(Dialect::Lua54);
        let decls = self.decls();
        let module_src = if self.cross_file {
            format!("{preamble}{}\nlocal B = {{}}\nreturn B\n", decls[0])
        } else {
            format!(
                "{preamble}{}\n{}\nlocal B = {{}}\nreturn B\n",
                decls[0], decls[1]
            )
        };
        let parsed_module = parse(&module_src, Dialect::Lua54);
        assert_eq!(
            parsed_module.errors(),
            &[],
            "module fixture must parse cleanly:\n{module_src}"
        );
        let module = module_surface(&parsed_module, "mod.lua", Some(base));
        let export_ty = module.export.clone().expect("module returns a value");
        let mut types = vec![module.types];
        if self.cross_file {
            let extra_src = format!("{preamble}{}\n", decls[1]);
            types.push(surface(&extra_src, base));
        }
        let ambient = base.with_project_types(types.iter());
        let mut requires = HashMap::new();
        requires.insert("mod".to_string(), export_ty);
        let parsed_consumer = parse(consumer_body, Dialect::Lua54);
        assert_eq!(
            parsed_consumer.errors(),
            &[],
            "consumer must parse cleanly:\n{consumer_body}"
        );
        check_file_with_requires(
            &parsed_consumer,
            "consumer.lua",
            Strictness::Strict,
            Dialect::Lua54,
            Some(&ambient),
            &requires,
        )
        .iter()
        .map(|d| d.code.to_string())
        .collect()
    }

    /// `(field, concrete type is string)` for every field this case declares.
    fn declared_fields(&self) -> Vec<(&'static str, bool)> {
        let mut out = Vec::new();
        if matches!(self.fields, Fields::First | Fields::Both) {
            out.push(("value", true));
            if self.arity() == 2 {
                out.push(("second", false));
            }
        }
        if matches!(self.fields, Fields::Second | Fields::Both) {
            out.push(("extra", true));
        }
        if matches!(self.parent, ParentShape::OwnParam | ParentShape::Concrete) {
            out.push(("slot", true));
        }
        out
    }
}

prop_compose! {
    fn arb_case()(
        arity in 1..=2usize,
        first_pick in 0..3usize,
        second_pick in 0..3usize,
        fields in prop_oneof![Just(Fields::First), Just(Fields::Second), Just(Fields::Both)],
        swapped in any::<bool>(),
        cross_file in any::<bool>(),
        parent in prop_oneof![
            Just(ParentShape::None),
            Just(ParentShape::Bare),
            Just(ParentShape::OwnParam),
            Just(ParentShape::Concrete),
        ],
        parent_on_second in any::<bool>(),
        crosses_require in any::<bool>(),
    ) -> Case {
        // Three disjoint spellings per arity; picking the same index for both
        // declarations is the same-name control, different indices the #46
        // renamed-duplicate shape.
        let pool: [&[&'static str]; 3] = if arity == 2 {
            [&["T", "U"], &["V", "W"], &["K", "V2"]]
        } else {
            [&["T"], &["U"], &["V"]]
        };
        Case {
            first_params: pool[first_pick].to_vec(),
            second_params: pool[second_pick].to_vec(),
            fields,
            swapped,
            cross_file,
            parent,
            parent_on_second,
            crosses_require,
        }
    }
}

proptest! {
    // F78 (round 3 review): explicit, so a CI failure's case count is
    // recorded here rather than left to proptest's default, and (default
    // persistence, left enabled) a failure ships a reproducible seed in
    // `proptest-regressions/` — the reproducibility a generator standing in
    // for enumerated shapes needs most.
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every declared field monomorphises through the canonical parameter
    /// list: the correctly-typed probe is clean, and the wrongly-typed probe
    /// fires exactly LB0300 — which a leaked parameter or an `unknown` field
    /// would NOT, so silent leniency cannot pass.
    #[test]
    fn merged_fields_monomorphise_positionally(case in arb_case()) {
        for (field, is_string) in case.declared_fields() {
            let (good, bad) = if is_string {
                ("want_string", "want_number")
            } else {
                ("want_number", "want_string")
            };
            let clean = case.run(&format!("local x = {good}(b.{field})"));
            prop_assert_eq!(
                clean, Vec::<String>::new(),
                "positive probe on `{}` must be clean for {:?}", field, case
            );
            let caught = case.run(&format!("local x = {bad}(b.{field})"));
            prop_assert_eq!(
                caught, vec!["LB0300".to_string()],
                "negative probe on `{}` must fire LB0300 for {:?} — \
                 leniency here means the parameter leaked or collapsed to unknown", field, case
            );
        }
        // A bare parent reference (`: Wrapped`, no argument) has nothing to
        // monomorphise the inherited field with, in EITHER direction — a
        // leaked parameter name would silently satisfy one of the two
        // probes below exactly as an ordinary field would, so both
        // directions are probed here too (round 3 review: "properties that
        // only assert 'no diagnostic' pass under every leniency bug").
        if case.parent == ParentShape::Bare {
            let string_probe = case.run("local x = want_string(b.slot)");
            prop_assert_eq!(
                string_probe, vec!["LB0300".to_string()],
                "an unbound parent parameter must not silently satisfy `want_string` for {:?}", case
            );
            let number_probe = case.run("local x = want_number(b.slot)");
            prop_assert_eq!(
                number_probe, vec!["LB0300".to_string()],
                "an unbound parent parameter must not silently satisfy `want_number` for {:?}", case
            );
        }
    }

    /// Pinned limitation, not a passing invariant: an undeclared member on
    /// `Boxed<Args>` is LB0300 (`found unknown`), never LB0306
    /// (`undefined field`) — whichever declaration order, spelling, or file
    /// layout produced the merge. This is what F76 (round 3 review) asked
    /// this property to assert `LB0306` for; running it found the assertion
    /// cannot hold, for a reason this PR does not touch.
    ///
    /// **Root cause.** `---@type Name<Args>` lowers a generic class ENTIRELY
    /// through the reference-site monomorphised-table template
    /// (`lower.rs:194-219`, `env.rs`'s `collect_generic_classes`), never
    /// through `Ty::Named` — `Ty::Named` carries no type arguments, so a
    /// generic reference has nowhere to put them except by monomorphising
    /// immediately into a plain `Ty::Table`. The `undefined-field`
    /// diagnostic (`infer.rs`'s `lookup_ty_field`) only fires for a
    /// `Ty::Named` receiver; a structural-table miss is lenient by design
    /// (`infer.rs:961-967`, "un-annotated code invents no undefined-field
    /// obligation") — correct for an inferred table literal, but it is also
    /// what every generic-class reference collapses to, so `Boxed<Args>`
    /// gets that same leniency regardless of how well-declared `Boxed` is.
    ///
    /// **Measured.** `first_pick == second_pick, fields: First, parent:
    /// None` — the simplest possible case, no duplication-specific shape
    /// involved at all — already gives `LB0300 "found unknown"` for
    /// `b.nope`. Bisected against a NON-generic `---@class Plain` with the
    /// identical shape, which correctly gives LB0306 — the difference is
    /// `<T>` alone. Uniform across every case this generator produces
    /// (`OwnParam`/`Concrete`/`crosses_require` included): the limitation is
    /// in the receiver's type representation, not in any merge seam this
    /// property's shape space varies.
    ///
    /// **What would flip this.** `Ty::Named` would need to carry an
    /// optional argument list, so a bound generic reference could resolve
    /// to a named-with-args identity instead of an anonymous table —
    /// `Ty::Named` is matched or constructed 125 times across 18 files, so
    /// that is a `Ty`-representation redesign, ruled out of scope for this
    /// PR. Pinned here so a future redesign has a red-to-green signal
    /// instead of a silent behaviour change.
    #[test]
    fn undeclared_members_on_a_generic_reference_stay_lenient_lb0300_not_lb0306(case in arb_case()) {
        let diags = case.run("local x = want_string(b.nope)");
        prop_assert_eq!(
            diags, vec!["LB0300".to_string()],
            "the CURRENT (limited) behaviour for {:?}: an undeclared member on a \
             generic reference is `LB0300 found unknown`, not `LB0306` — see the \
             doc comment for why, and what would have to change", case
        );
    }
}

/// Deterministic pin for the surplus-parameter bound the generator leaves
/// alone (its declarations always share an arity): a second declaration with
/// MORE parameters than the canonical list maps its surplus to `unknown`,
/// never a leaked name — no LB0305 `unknown type name 'B'` fires anywhere.
///
/// Measured, not assumed: strict mode rejects `unknown` at a `string` call
/// site (`LB0300 … found unknown`), so the surplus-typed field errs on the
/// strict side at its use site rather than passing silently. The slot that
/// HAS a canonical position (`first`) still monomorphises clean.
#[test]
fn surplus_parameter_collapses_to_unknown_not_a_leak() {
    let common = "\
---@class Pair<T>
---@field first T

---@class Pair<A, B>
---@field rest B

---@type Pair<string>
local p
---@param s string
local function want(s) end
";
    assert_eq!(
        check(&format!("{common}local x = want(p.first)\n")),
        Vec::<String>::new(),
        "the slot with a canonical position must monomorphise to string"
    );
    assert_eq!(
        check(&format!("{common}local y = want(p.rest)\n")),
        vec!["LB0300".to_string()],
        "the surplus slot is unknown — strict at the use site, not a leak \
         (a leak would be LB0305 at the declaration, which must not fire)"
    );
}

/// Template field ownership is **positional**, not name-keyed (#58 mutation
/// audit): the fields between one `---@class` tag and the next belong to
/// the first, full stop. Deleting the `Tag::Class => break` in
/// `lower_class_template` folds a following class's fields into the
/// preceding template — invisible to every fixture whose blocks declare one
/// class each, which is all the suite had.
#[test]
fn a_following_class_in_the_same_block_does_not_leak_its_fields() {
    // Second deliberately spells its parameter `T` — the same name as
    // First's — so a leak MONOMORPHISES: leaked `y: T` becomes `string`
    // under `First<string>` and the probe goes silently clean. Without the
    // leak, `a.y` is a missing member (lenient `unknown` on a generic
    // instance, measured), which strict mode rejects at the probe with
    // LB0300 `found unknown`. The diagnostic's *presence* is what separates
    // the two worlds.
    let src = "\
---@class First<T>
---@field x T
---@class Second<T>
---@field y T

---@type First<string>
local a
---@param s string
local function want_string(s) end
local probe = want_string(a.y)
";
    assert_eq!(check(src), vec!["LB0300".to_string()]);
}

/// A duplicate declaration's INDEXER unions into the merged template too
/// (#58 mutation audit): the generator and every deterministic fixture
/// above carry named fields only, so the indexer arm of the template merge
/// could be inverted without a test noticing.
#[test]
fn a_duplicate_declarations_indexer_unions_into_the_template() {
    let src = "\
---@class Bag<T>
---@field first T

---@class Bag<T>
---@field [string] T

---@type Bag<number>
local b
---@param n number
local function want_number(n) end
local probe = want_number(b.anything)
";
    // The second declaration's `[string] T` indexer monomorphises to
    // `number` under `Bag<number>`, so an arbitrary member read type-checks
    // clean. Dropped, `b.anything` is `unknown`, which strict mode rejects
    // at the probe.
    assert_eq!(check(src), Vec::<String>::new());
}

/// The generator never produces a single bare declaration alone (its cases
/// always have two); pin that control here so the suite covers it.
#[test]
fn single_declaration_control_is_unaffected() {
    let src = "\
---@class Boxed<U>
---@field value U

---@type Boxed<string>
local b
---@param s string
local function want_string(s) end
---@param n number
local function want_number(n) end
local x = want_string(b.value)
local y = want_number(b.value)
";
    assert_eq!(check(src), vec!["LB0300".to_string()]);
}

// --- hand-written regressions for the two shapes the generator's original
// space could not reach (round 3 review F77) --------------------------------

/// F38, independently confirmed — and confirmed fixed.
///
/// `env.rs:784-788` used to push a re-declaration's parent reference into
/// `existing.parents` BEFORE `class_param_unification` (`:806-809`)
/// computed the positional rename — unlike fields, methods and indexers,
/// which all substituted through it (`:831-834`). This fixture is the shape
/// F38 named: a duplicate-declared GENERIC class (`Cell`) whose second
/// declaration both renames its own parameter and writes that parameter
/// into a parent's argument list.
///
/// It first failed for an unrelated reason: `collect_generic_classes`
/// (`env.rs:1830-1929`, the table `---@type Name<Args>` monomorphises
/// against at the reference site, per `lower.rs:194-219`) built its
/// template from one declaration's own `---@field`s only and never walked
/// `def.parents` at all, so `c.slot` read `unknown` regardless of F38's fix
/// state — confirmed by bisection against a single, non-duplicated
/// `---@class Child<T> : Parent<Args>`, and again with `Child`'s own `<T>`
/// removed (a non-generic child correctly inherited, since `class_shape`,
/// unlike the old `collect_generic_classes`, does walk parents). That
/// masking bug is now closed too: `collect_generic_classes` was rewritten
/// to run a discovery pass through `TypeEnv::absorb_block` (the SAME
/// duplicate-merge machinery every other seam uses) and read the template
/// back off `class_shape`, rather than hand-rolling its own
/// parent-blind, per-declaration walk.
///
/// Both fixes measured together: this now passes.
#[test]
fn a_renamed_duplicates_parent_argument_still_monomorphises() {
    let src = "\
---@class Slot<S>
---@field slot S

---@class Cell<T>
---@field x T

---@class Cell<U> : Slot<U>

---@type Cell<string>
local c
---@param s string
local function want(s) end
local ok = want(c.slot)
";
    assert_eq!(
        check(src),
        Vec::<String>::new(),
        "the renamed declaration's parent argument must monomorphise through \
         the canonical parameter, exactly as its fields do"
    );
}

/// F42 (round 3 review), confirmed fixed via the two-pass pattern the fix
/// actually uses.
///
/// The round-2 fix for the export seam asked `class_params_in_scope`
/// whether the exported class still had an unbound parameter, but a
/// single-pass fixture — reify each file's export once, against a defs-only
/// ambient — cannot observe the fix at all: `check_cmd.rs`'s `run_passes`
/// and `luabox-db`'s `module_export_checked` both compute a file's export
/// TWICE. Pass 1 (`check_cmd.rs:267-277`, `query.rs::module_surface_checked`
/// `:166-176`) reifies every file alone, ambient = defs only, and keeps only
/// `.types` — the workspace-global class declarations, which do not depend
/// on cross-file visibility. Those merge into the project-wide ambient
/// (`check_cmd.rs:300-303`, `query.rs::project_types_checked` `:244-251`).
/// Pass 2 (`check_cmd.rs:322-333`, `query.rs::module_export_checked`
/// `:198-210`) re-reifies EACH FILE'S EXPORT against that merged ambient —
/// this is the step the original single-pass fixture omitted, and the step
/// that makes `Base`, declared in a different file than `Sub`, visible to
/// `Sub`'s own `reify_export` call the second time around.
///
/// A same-file generic ancestor was already pinned
/// (`cross_file_require.rs::a_parent_written_bare_leaves_its_parameter_unbound_and_erased`);
/// this is the shape where the ancestor and the child are split across two
/// files, which is what "crosses the export seam" actually means for a real
/// project — and, with the two-pass reification the fixture now performs,
/// it is clean.
#[test]
fn a_cross_file_generic_ancestor_is_invisible_at_export_reification_time() {
    let base_src = "\
---@class Base<U>
---@field item U
local M = {}
return M
";
    let sub_src = "\
---@class Sub : Base
local S = {}
return S
";
    let stdlib = stdlib_defs(Dialect::Lua54);

    // Pass 1: each file's own surface against the defs-only ambient. Only
    // `.types` survives into the merge — a file's own declarations do not
    // depend on what any other file names.
    let base_parsed = parse(base_src, Dialect::Lua54);
    assert_eq!(base_parsed.errors(), &[], "base fixture must parse cleanly");
    let base_pass1 = module_surface(&base_parsed, "base.lua", Some(stdlib));

    let sub_parsed = parse(sub_src, Dialect::Lua54);
    assert_eq!(sub_parsed.errors(), &[], "sub fixture must parse cleanly");
    let sub_pass1 = module_surface(&sub_parsed, "sub.lua", Some(stdlib));

    // Merge: the project-wide ambient every file's export re-reifies
    // against.
    let merged = stdlib.with_project_types([&base_pass1.types, &sub_pass1.types]);

    // Pass 2: re-reify `sub`'s EXPORT against the merged ambient — `Base` is
    // visible this time, so `reify_export`'s unbound-parameter check sees
    // `Sub`'s inherited `<U>` and erases it, instead of crossing `require`
    // as the unerased `Ty::Named("Sub")` the round-2 fix left behind for
    // this cross-file shape.
    let sub_pass2 = module_surface(&sub_parsed, "sub.lua", Some(&merged));
    let sub_export = sub_pass2.export.expect("module returns a value");

    let mut requires = HashMap::new();
    requires.insert("sub".to_string(), sub_export);

    let consumer = "\
---@param n number
local function want(n) end
local s = require(\"sub\")
want(s.item)
";
    let parsed_consumer = parse(consumer, Dialect::Lua54);
    assert_eq!(parsed_consumer.errors(), &[], "consumer must parse cleanly");
    let diags = check_file_with_requires(
        &parsed_consumer,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&merged),
        &requires,
    );
    let diag_codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(
        diag_codes,
        vec!["LB0300".to_string()],
        "the inherited parameter must erase to unknown, not leak its bare name, \
         across a cross-file export"
    );
    assert!(
        diags[0].message.contains("found `unknown`"),
        "{}",
        diags[0].message
    );
}
