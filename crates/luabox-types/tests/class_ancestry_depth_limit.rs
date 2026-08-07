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
//! (`check_cmd::MAX_CLASS_CHAIN_DEPTH`, which only runs project-wide, once,
//! before the CLI's `check` command starts resolving anything), this guard
//! lives inside `luabox-types` itself, so it protects every caller that
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

use std::fmt::Write as _;

use luabox_diag::Diagnostic;
use luabox_syntax::lua::{Dialect, parse};
use luabox_types::{MAX_ANCESTRY_DEPTH, Strictness, check_file};

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
