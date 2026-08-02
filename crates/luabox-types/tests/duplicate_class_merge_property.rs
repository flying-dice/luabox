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

use luabox_syntax::lua::{self, Dialect, parse};
use luabox_types::{
    Ambient, FileTypes, Strictness, check_file_with_ambient, module_surface, stdlib_defs,
};
use proptest::prelude::*;

fn check(src: &str) -> Vec<String> {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    check_file_with_ambient(
        &parsed,
        "test.lua",
        Strictness::Strict,
        lua::Dialect::Lua54,
        Some(stdlib_defs(Dialect::Lua54)),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
}

/// The workspace surface one project file contributes.
fn surface(src: &str, base: &Ambient) -> FileTypes {
    let parsed = parse(src, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "fixture must parse cleanly:\n{src}");
    module_surface(&parsed, "m.lua", Some(base)).types
}

/// Check `consumer` against the merged surface of every `file`.
fn check_cross(files: &[String], consumer: &str) -> Vec<String> {
    let base = stdlib_defs(Dialect::Lua54);
    let types: Vec<FileTypes> = files.iter().map(|f| surface(f, base)).collect();
    let ambient = base.with_project_types(types.iter());
    let parsed = parse(consumer, Dialect::Lua54);
    assert_eq!(parsed.errors(), &[], "consumer must parse cleanly");
    check_file_with_ambient(
        &parsed,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
    )
    .iter()
    .map(|d| d.code.to_string())
    .collect()
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
}

impl Case {
    fn arity(&self) -> usize {
        self.first_params.len()
    }

    /// The `---@class` block for one declaration. `value`/`second` belong to
    /// the "first" declaration's identity, `extra` to the "second"'s, so a
    /// swap moves the whole block, never re-attributes fields.
    fn decl(&self, params: &[&str], own_fields: bool, first_identity: bool) -> String {
        use std::fmt::Write as _;
        let mut s = format!("---@class Boxed<{}>\n", params.join(", "));
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
        format!(
            "---@type {args}\nlocal b\n\
             ---@param s string\nlocal function want_string(s) end\n\
             ---@param n number\nlocal function want_number(n) end\n\
             {probe}\n"
        )
    }

    /// Run one probe against this case's declarations.
    fn run(&self, probe: &str) -> Vec<String> {
        let consumer = self.consumer(probe);
        if self.cross_file {
            let files: Vec<String> = self.decls().into_iter().map(|d| format!("{d}\n")).collect();
            check_cross(&files, &consumer)
        } else {
            let src = format!("{}\n{consumer}", self.decls().join("\n"));
            check(&src)
        }
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
        }
    }
}

proptest! {
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
    }

    /// A misspelt member on the merged class still reports LB0306 whichever
    /// declaration order, spelling, or file layout produced the merge — the
    /// merge must not manufacture leniency for names neither declaration has.
    #[test]
    fn merged_class_still_rejects_unknown_members(case in arb_case()) {
        let diags = case.run("local x = want_string(b.nope)");
        prop_assert!(
            !diags.is_empty(),
            "unknown member `nope` must be diagnosed for {:?}, got none", case
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
