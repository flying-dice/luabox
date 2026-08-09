// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! A `---@class` single-parent ancestry deep enough to trip `DiamondGuard`'s
//! depth cap is reported as `LB0317`, naming the file and the class — not a
//! crash, and not a silently truncated shape nobody is told about (round 5
//! review N2's durable fix, `luabox_types::env::MAX_ANCESTRY_DEPTH`).
//!
//! The class-shape/operator resolution walk (`TypeEnv::collect_class`/
//! `collect_operators`) is the recursive merge behind every `---@class`
//! reference. Unlike `luabox-cli`'s own syntactic pre-check
//! (`check_cmd::class_ancestry_precheck`, which imports and checks
//! against this same `MAX_ANCESTRY_DEPTH` constant — round 6 review M52: an
//! earlier revision of this comment named a separate `MAX_CLASS_CHAIN_DEPTH`
//! constant that no longer exists; the CLI pre-check stopped keeping its own
//! figure once it had this crate's to import instead — and which only runs
//! project-wide, once, before the CLI's `check` command starts resolving
//! anything), this guard lives inside `luabox-types` itself, so it protects
//! every caller that
//! reaches a `TypeEnv` directly — including the LSP request path and an
//! embedder calling `luabox_lsp::run_stdio`, neither of which goes through
//! the CLI's pre-check at all. This suite exercises `luabox_types::check_file`
//! directly, bypassing the CLI entirely, so it is this guard — not the
//! CLI's — being proven here.
//!
//! Crash-prevention itself (the walk terminates instead of aborting the
//! process) is proven at the `luabox-types` unit level, on a deliberately
//! small pinned stack:
//! `env::tests::a_class_chain_past_the_ancestry_limit_does_not_crash`. This
//! suite proves the other half — what the user actually sees at the
//! boundary.

use std::collections::HashMap;
use std::fmt::Write as _;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::ty::Ty;
use luabox_types::{
    FileArtifacts, MAX_ANCESTRY_DEPTH, Strictness, build_ambient, check_file,
    check_file_with_artifacts_and_sources, module_surface_with_artifacts,
};

/// A single-file `---@class C0`, `---@class C1 : C0`, ..., `Cn : C(n-1)`
/// chain of length `n`, matching `luabox-cli::check_cmd`'s own fixture shape.
///
/// `item` is *optional* — deliberately, unlike the `env.rs` unit tests that
/// probe whether it survives a truncated walk: an empty `x = {}` conforming
/// to `Cn` regardless of chain depth or truncation isolates this suite's
/// question ("is the ancestry-depth diagnostic reported?") from an unrelated
/// one ("is a required inherited field satisfied?", already covered by
/// `table_literals.rs`).
fn chain_source(n: usize) -> String {
    let mut src = String::from("---@class C0\n---@field item? number\n");
    for i in 1..=n {
        let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
    }
    src
}

fn check(source: &str) -> Vec<Diagnostic> {
    let parsed = parse(source, Dialect::Lua54);
    assert_eq!(
        parsed.errors(),
        &[],
        "fixture must parse cleanly:\n{source}"
    );
    check_file(&parsed, "test.lua", Strictness::Strict, Dialect::Lua54)
}

fn codes(source: &str) -> Vec<String> {
    check(source).iter().map(|d| d.code.to_string()).collect()
}

#[test]
fn a_pathological_ancestry_is_reported_not_crashed() {
    // 500 classes: well above `MAX_ANCESTRY_DEPTH` (200, documented on the
    // constant itself), and an order of magnitude past any depth a real
    // hierarchy reaches — but small enough that a REGRESSION back to the
    // old unbounded walk would still resolve fine on this test's ordinary
    // (un-pinned) thread rather than aborting the whole binary, so a
    // regression here is a clean, readable assertion failure, not a lost
    // test run.
    //
    // Declaring the chain alone is not enough to force resolution — nothing
    // walks a declared-but-unreferenced class's shape — so a `---@type`
    // local on the deepest class is what actually asks `TypeEnv` to resolve
    // it, exactly as `luabox-types/src/env.rs`'s own
    // `a_class_chain_past_the_ancestry_limit_does_not_crash` does via
    // `class_shape` directly.
    let n = 500;
    let mut src = chain_source(n);
    let _ = writeln!(src, "\n---@type C{n}\nlocal x = {{}}");

    let diags = check(&src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0317"],
        "exactly one depth-limit diagnostic, nothing else: {diags:?}"
    );
    let diag = &diags[0];
    assert!(
        diag.message.contains(&format!("C{n}")),
        "the message must name the pathological class: {}",
        diag.message
    );
    let label = diag
        .primary_label()
        .expect("the diagnostic must carry a primary label pointing at a location");
    assert_eq!(
        label.span.file, "test.lua",
        "the diagnostic must name the file the pathological class was declared in"
    );
}

#[test]
fn an_ordinary_deep_hierarchy_checks_cleanly() {
    // The floor side, at the integration level (not just the unit level):
    // 150 sits under `MAX_ANCESTRY_DEPTH` (200) and is already far deeper
    // than any real single-inheritance hierarchy this codebase's own
    // reasoning (`env::MAX_ANCESTRY_DEPTH`'s doc comment) argues exists — a
    // limit that fires on plausible real code would be a worse bug than the
    // crash it guards against, so this must stay clean.
    let n = 150;
    let mut src = chain_source(n);
    let _ = writeln!(src, "\n---@type C{n}\nlocal x = {{}}");
    assert_eq!(codes(&src), Vec::<String>::new());
}

#[test]
fn a_truncated_ancestry_does_not_manufacture_a_false_undefined_field() {
    // Production readiness review, finding 1 (the serious one): before this
    // fix, a class whose ancestry `DiamondGuard` truncated still had its
    // now-incomplete shape consulted for a field READ exactly as if it were
    // complete — so `item`, genuinely declared by `C0` (the root, well
    // above the cutoff for a 400-deep chain), came back `LB0306` "undefined
    // field" on top of the honest `LB0317`, and in strict mode that fails
    // the build over a field the tool's own diagnostic says it could not
    // resolve, not one that is actually missing. Repro matches the finding
    // exactly: 400 classes, `C0` declaring the field, the leaf reading it.
    //
    // `LB0317` alone is correct: a shape `DiamondGuard` admits it truncated
    // must go lenient on member reads, not confidently wrong.
    let n = 400;
    let mut src = chain_source(n);
    let _ = writeln!(src, "\n---@type C{n}\nlocal x = {{}}\nlocal y = x.item");

    let diags = check(&src);
    assert_eq!(
        diags.iter().map(|d| d.code.to_string()).collect::<Vec<_>>(),
        vec!["LB0317"],
        "exactly the depth-limit diagnostic, no false LB0306 for `item` \
         (declared by `C0`, just past the resolvable cutoff): {diags:?}"
    );
}

#[test]
fn a_chain_exactly_at_the_ancestry_limit_reads_the_root_field_with_no_diagnostics() {
    // The boundary's other direction, with an actual field READ rather than
    // just table-literal conformance (`an_ordinary_deep_hierarchy_checks_cleanly`
    // above only proves the latter): a chain of EXACTLY `MAX_ANCESTRY_DEPTH`
    // classes total (`C0` through `C(MAX_ANCESTRY_DEPTH - 1)`) must still
    // carry `C0`'s field all the way up to the leaf, with nothing reported —
    // not `LB0317` (the guard trips only on the class that would push
    // `on_path` PAST the limit, not the one that reaches it exactly — see
    // `env::MAX_ANCESTRY_DEPTH`'s doc comment) and not a false `LB0306`
    // either.
    let n = MAX_ANCESTRY_DEPTH - 1;
    let mut src = chain_source(n);
    let _ = writeln!(src, "\n---@type C{n}\nlocal x = {{}}\nlocal y = x.item");
    assert_eq!(codes(&src), Vec::<String>::new());
}

#[test]
fn a_chain_one_class_past_the_ancestry_limit_is_reported_at_the_integration_level() {
    // Round 6 review M64: the two boundary tests above pin only the FLOOR
    // (`MAX_ANCESTRY_DEPTH - 1` classes, must stay clean) — neither one
    // exercises the adjacent class that must trip `LB0317`, so a
    // `env.rs:482`-level regression that widens the guard from
    // `on_path.len() >= MAX_ANCESTRY_DEPTH` to `on_path.len() >
    // MAX_ANCESTRY_DEPTH` (silently permitting one extra ancestry link
    // before refusing) left every integration-level suite green here and
    // was only caught by three `luabox-types` lib-level unit tests nobody
    // outside that crate exercises. This is the missing adjacent case: a
    // chain of EXACTLY `MAX_ANCESTRY_DEPTH` classes total (`C0` through
    // `C(MAX_ANCESTRY_DEPTH)`, i.e. `chain_source(MAX_ANCESTRY_DEPTH)`) is
    // the SMALLEST chain the guard must refuse
    // (`env::MAX_ANCESTRY_DEPTH`'s own doc comment: the smallest tripping
    // chain has `MAX_ANCESTRY_DEPTH + 1` classes). Paired with the test
    // above (whose `n = MAX_ANCESTRY_DEPTH - 1` chain — one class shorter —
    // must stay clean), the exact transition point is now pinned by two
    // adjacent integration-level assertions, not one boundary and a gap:
    // the `>=`-to-`>` weakening this test is named for moves that
    // transition by exactly one class, which is exactly the distance
    // between these two tests' fixtures.
    let n = MAX_ANCESTRY_DEPTH;
    let mut src = chain_source(n);
    let _ = writeln!(src, "\n---@type C{n}\nlocal x = {{}}");
    assert_eq!(
        codes(&src),
        vec!["LB0317".to_string()],
        "the smallest ancestry chain the guard must refuse (MAX_ANCESTRY_DEPTH + 1 \
         classes total) did not trip LB0317 — the guard's boundary moved"
    );
}

/// Production readiness review G1: `LB0317`, exactly like `LB0318`, can
/// carry a primary label belonging to a file OTHER than the one whose check
/// pass tripped `DiamondGuard`'s depth cap — here, the pathological class
/// `Cn` is declared in `chain.lua`, but only `consumer.lua`'s `---@type Cn`
/// reference ever asks `TypeEnv` to resolve it, so `consumer.lua`'s own
/// check pass is the one that trips the guard and drains the hit
/// (`env::TypeEnv::cross_file_class_decl_span` attributes the resulting
/// diagnostic back to `chain.lua`, where `Cn` actually lives).
///
/// `chain.lua` carries the ONLY `---@diagnostic disable-next-line:
/// class-ancestry-too-deep` comment, directly above `Cn`'s own `---@class`
/// tag; `consumer.lua` has none at all. This proves both halves of
/// [`check_file_with_artifacts_and_sources`]'s contract: absent a resolver,
/// the diagnostic is never wrongly suppressed (survives, checked against
/// `test.lua`'s own directives would be nonsense against `chain.lua`'s
/// bytes); given one that can fetch `chain.lua`'s real source, it is
/// suppressed correctly.
#[test]
fn a_depth_limit_diagnostic_attributed_to_another_file_honors_that_files_own_disable_comment() {
    let n = MAX_ANCESTRY_DEPTH;
    let mut chain_src = chain_source(n - 1);
    chain_src.push_str("---@diagnostic disable-next-line: class-ancestry-too-deep\n");
    let _ = writeln!(chain_src, "---@class C{n} : C{}", n - 1);
    let chain_parse = parse(&chain_src, Dialect::Lua54);
    assert_eq!(
        chain_parse.errors(),
        &[],
        "fixture must parse cleanly:\n{chain_src}"
    );
    let chain_artifacts = FileArtifacts::new(&chain_parse);
    let chain_surface =
        module_surface_with_artifacts(&chain_parse, "chain.lua", None, &chain_artifacts);

    let consumer_src = format!("---@type C{n}\nlocal x = {{}}\n");
    let consumer_parse = parse(&consumer_src, Dialect::Lua54);
    assert_eq!(consumer_parse.errors(), &[]);
    let consumer_artifacts = FileArtifacts::new(&consumer_parse);

    let base_ambient = build_ambient(Dialect::Lua54, &[]);
    let ambient = base_ambient.with_project_types([&chain_surface.types]);
    let requires: HashMap<String, Ty> = HashMap::new();

    // No resolver: the diagnostic — attributed to `chain.lua` — must
    // survive rather than be checked against the wrong file.
    //
    // This arm is also the pin for a DOCUMENTED USER-VISIBLE LIMIT, not just
    // an internal safety property (round 8 review F8). `None` is exactly what
    // the language server passes — it checks one open document and has no
    // resolver — so "survives unsuppressed" is what an editor shows while
    // `luabox check`, which does pass a resolver (the arm below), goes green
    // on the same workspace. That divergence is now stated as a measured
    // exception in `luabox explain LB0317`/`LB0318`/`LB0319` and in
    // docs/03-reference/02-limitations.md's editor/CLI parity claim; the two
    // arms here are what those texts describe. Reversing this arm to
    // "suppressed" would make the docs wrong, not just the behaviour
    // different.
    let unsuppressed = check_file_with_artifacts_and_sources(
        &consumer_parse,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
        &requires,
        &consumer_artifacts,
        None,
    );
    assert_eq!(
        unsuppressed
            .iter()
            .map(|d| d.code.to_string())
            .collect::<Vec<_>>(),
        vec!["LB0317".to_string()],
        "with no cross-file resolver the depth-limit diagnostic must still be \
         reported, not silently (and wrongly) dropped: {unsuppressed:?}"
    );

    // A resolver that can fetch `chain.lua`'s real source: `chain.lua`'s own
    // `disable-next-line` comment, directly above `Cn`'s declaration, must
    // suppress it.
    let resolve =
        |name: &str| -> Option<String> { (name == "chain.lua").then(|| chain_src.clone()) };
    let suppressed = check_file_with_artifacts_and_sources(
        &consumer_parse,
        "consumer.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
        &requires,
        &consumer_artifacts,
        Some(&resolve),
    );
    assert_eq!(
        suppressed,
        Vec::<Diagnostic>::new(),
        "chain.lua's own disable-next-line comment, directly above Cn's declaration, \
         must suppress the LB0317 attributed to it: {suppressed:?}"
    );
}
