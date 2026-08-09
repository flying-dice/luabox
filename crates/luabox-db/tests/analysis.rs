//! Boundary + incrementality tests for the analysis database.
//!
//! These drive the crate exactly as the LSP will: build an [`AnalysisHost`],
//! [`apply_change`](AnalysisHost::apply_change), take a
//! [`snapshot`](AnalysisHost::snapshot), and read results. The execution trace
//! ([`AnalysisHost::take_execution_log`]) is used to prove *which* queries
//! actually recompute after an edit.

use std::path::{Path, PathBuf};

use luabox_db::{AnalysisHost, Change, Dialect, MAX_EXECUTION_LOG_ENTRIES, Strictness};
use luabox_syntax::lua;
use luabox_types::{check_file, check_file_with_requires, stdlib_defs};

/// Source with a call-argument type error (`LB0300`) that `check_file` reports.
const BAD: &str = "\
---@param n number
local function f(n) end
f(\"no\")
";

/// A clean annotated file — no diagnostics.
const GOOD: &str = "\
---@param n number
local function f(n) end
f(1)
";

fn host() -> AnalysisHost {
    AnalysisHost::new(Dialect::Lua54, Strictness::Strict)
}

fn set(path: &str, text: &str) -> Change {
    Change::SetFileText {
        path: PathBuf::from(path),
        dialect: Dialect::Lua54,
        text: text.to_owned(),
    }
}

/// Only the `parse(...)`/`diagnostics(...)` etc. lines mentioning `needle`.
fn mentioning<'a>(log: &'a [String], needle: &str) -> Vec<&'a String> {
    log.iter().filter(|l| l.contains(needle)).collect()
}

/// Assert `needle` never ran, per `log` — but refuse to answer at all when
/// `overflowed` is set (M46, round 6 review): once the trace has evicted an
/// entry, `needle`'s absence from `log` no longer proves it did not run —
/// it may simply have aged out — so an absence assertion taken over a lossy
/// trace is not proof of anything and must not be trusted the way it was
/// before the cap existed. Callers must read
/// [`luabox_db::AnalysisHost::execution_log_overflowed`] *before* draining
/// (draining resets it) and pass that here.
#[track_caller]
fn assert_absent(log: &[String], overflowed: bool, needle: &str) {
    assert!(
        !overflowed,
        "the execution trace overflowed (evicted at least one entry) since \
         the last drain — an absence assertion for {needle:?} cannot be \
         trusted against a lossy trace: {log:?}"
    );
    assert!(
        mentioning(log, needle).is_empty(),
        "expected {needle:?} not to have run, but it did: {log:?}"
    );
}

#[test]
fn diagnostics_parity_with_check_file() {
    let mut host = host();
    host.apply_change(set("a.lua", BAD));

    let got = host.snapshot().diagnostics(Path::new("a.lua")).unwrap();

    // Exactly what a direct check_file over the same source produces.
    let parse = lua::parse(BAD, Dialect::Lua54);
    let want = check_file(&parse, "a.lua", Strictness::Strict, Dialect::Lua54);

    assert_eq!(got, want);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].code.to_string(), "LB0300");
}

#[test]
fn clean_file_has_no_diagnostics() {
    let mut host = host();
    host.apply_change(set("a.lua", GOOD));
    assert!(
        host.snapshot()
            .diagnostics(Path::new("a.lua"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn query_results_are_memoized() {
    let mut host = host();
    host.apply_change(set("a.lua", BAD));
    let a = Path::new("a.lua");

    // First read runs the queries.
    let _ = host.snapshot().diagnostics(a);
    let first = host.take_execution_log();
    assert!(
        !mentioning(&first, "diagnostics(a.lua)").is_empty(),
        "first read should execute diagnostics: {first:?}"
    );

    // Second read with no intervening change recomputes nothing.
    let _ = host.snapshot().diagnostics(a);
    let second = host.take_execution_log();
    assert!(
        second.is_empty(),
        "re-reading an unchanged file must hit the cache, ran: {second:?}"
    );
}

#[test]
fn editing_one_file_does_not_recompute_another() {
    let mut host = host();
    host.apply_changes([set("a.lua", BAD), set("b.lua", GOOD)]);

    // Warm both files' diagnostics.
    let _ = host.snapshot().project_diagnostics();
    let _ = host.take_execution_log();

    // Edit only A.
    host.apply_change(set("a.lua", GOOD));
    let _ = host.snapshot().project_diagnostics();
    let log = host.take_execution_log();

    // A re-parses and re-checks; B does neither.
    assert!(
        !mentioning(&log, "parse(a.lua)").is_empty(),
        "A should re-parse: {log:?}"
    );
    assert!(
        !mentioning(&log, "diagnostics(a.lua)").is_empty(),
        "A should re-check: {log:?}"
    );
    assert!(
        mentioning(&log, "parse(b.lua)").is_empty(),
        "B must NOT re-parse: {log:?}"
    );
    assert!(
        mentioning(&log, "diagnostics(b.lua)").is_empty(),
        "B must NOT re-check: {log:?}"
    );
}

#[test]
fn editing_a_file_updates_its_diagnostics() {
    let mut host = host();
    host.apply_change(set("a.lua", BAD));
    let a = Path::new("a.lua");
    assert_eq!(host.snapshot().diagnostics(a).unwrap().len(), 1);

    host.apply_change(set("a.lua", GOOD));
    assert!(host.snapshot().diagnostics(a).unwrap().is_empty());
}

#[test]
fn overlay_beats_disk_in_analysis() {
    let mut host = host();
    // On disk: clean.
    host.apply_change(set("a.lua", GOOD));
    let a = Path::new("a.lua");
    assert!(host.snapshot().diagnostics(a).unwrap().is_empty());

    // Editor buffer introduces an error — overlay wins.
    host.apply_change(Change::SetOverlay {
        path: PathBuf::from("a.lua"),
        text: BAD.to_owned(),
    });
    assert_eq!(host.snapshot().diagnostics(a).unwrap().len(), 1);

    // Closing the buffer reverts to the clean disk content.
    host.apply_change(Change::ClearOverlay {
        path: PathBuf::from("a.lua"),
    });
    assert!(host.snapshot().diagnostics(a).unwrap().is_empty());
}

#[test]
fn changing_strictness_rechecks_but_downgrades() {
    let mut host = host();
    host.apply_change(set("a.lua", BAD));
    let a = Path::new("a.lua");

    let strict = host.snapshot().diagnostics(a).unwrap();
    assert_eq!(strict[0].severity, luabox_diag::Severity::Error);

    host.apply_change(Change::SetStrictness(Strictness::Warn));
    let warn = host.snapshot().diagnostics(a).unwrap();
    assert_eq!(warn[0].severity, luabox_diag::Severity::Warning);

    host.apply_change(Change::SetStrictness(Strictness::None));
    assert!(host.snapshot().diagnostics(a).unwrap().is_empty());
}

#[test]
fn parse_tree_and_annotations_are_accessible() {
    let mut host = host();
    host.apply_change(set("a.lua", BAD));
    let snap = host.snapshot();
    let a = Path::new("a.lua");

    let parsed = snap.parse(a).unwrap();
    assert_eq!(parsed.syntax().text().to_string(), BAD);
    assert!(parsed.errors().is_empty());

    // The `---@param n number` block is harvested.
    let annotations = snap.annotations(a).unwrap();
    assert!(!annotations.items().is_empty());

    // Unknown path -> None.
    assert!(snap.parse(Path::new("missing.lua")).is_none());
}

#[test]
fn project_diagnostics_aggregate_all_files() {
    let mut host = host();
    host.apply_changes([set("a.lua", BAD), set("b.lua", BAD), set("c.lua", GOOD)]);
    // Two bad files, one clean -> two diagnostics total.
    assert_eq!(host.snapshot().project_diagnostics().len(), 2);
}

#[test]
fn whitespace_only_edit_backdates_diagnostics() {
    // Firewall: a re-parse whose diagnostics are unchanged should not force
    // the project aggregator to recompute. We add trailing whitespace to A.
    let mut host = host();
    host.apply_changes([set("a.lua", GOOD), set("b.lua", GOOD)]);
    let _ = host.snapshot().project_diagnostics();
    let _ = host.take_execution_log();

    host.apply_change(set("a.lua", &format!("{GOOD}\n")));
    let _ = host.snapshot().project_diagnostics();
    let log = host.take_execution_log();

    // A re-parses (text changed) and re-checks, but since A's diagnostics are
    // still empty, project_diagnostics backdates: it should NOT re-run.
    assert!(!mentioning(&log, "parse(a.lua)").is_empty(), "{log:?}");
    assert!(
        mentioning(&log, "project_diagnostics()").is_empty(),
        "identical aggregated diagnostics must backdate the aggregator: {log:?}"
    );
}

#[test]
fn lower_exposes_name_resolution_and_is_memoized() {
    let mut host = host();
    host.apply_change(set("a.lua", GOOD));
    let a = Path::new("a.lua");

    let snap = host.snapshot();
    let lowered = snap.lower(a).unwrap();
    // `local function f` introduces one binding named `f`, plus its param.
    let names: Vec<&str> = lowered
        .file()
        .bindings()
        .map(|(_, b)| b.name.as_str())
        .collect();
    assert!(names.contains(&"f"), "bindings: {names:?}");
    let _ = host.take_execution_log();

    // Re-reading without an edit is served from cache.
    let _ = host.snapshot().lower(a);
    let log = host.take_execution_log();
    assert!(
        mentioning(&log, "lower(a.lua)").is_empty(),
        "unchanged file must not re-lower: {log:?}"
    );

    // Unknown path -> None.
    assert!(host.snapshot().lower(Path::new("missing.lua")).is_none());
}

#[test]
fn require_resolution_uses_bundle_path_mapping_not_suffix() {
    // The db resolves `require` with the bundler's SPEC.md §7 path-mapping
    // (project root and `src/` only), the *same* ordering `luabox check` uses
    // on disk — not by trailing-path suffix. A module correctly placed at
    // `src/geom.lua` resolves as `require("geom")`; one buried at
    // `lib/util/helper.lua` does NOT resolve as `require("helper")`. The old
    // suffix match wrongly resolved the buried file, so the editor saw a
    // require the CLI never did. Requires a project root (the LSP sets one).
    let module = "\
local M = {}
---@return number
function M.area() return 1 end
return M
";
    let main = "local geom = require(\"geom\")\nlocal helper = require(\"helper\")\n";
    let mut host = host();
    host.set_root(PathBuf::from("/proj"));
    host.apply_changes([
        set("/proj/src/geom.lua", module),
        set("/proj/lib/util/helper.lua", module),
        set("/proj/main.lua", main),
    ]);

    let reqs = host
        .snapshot()
        .require_exports(Path::new("/proj/main.lua"))
        .unwrap();
    assert!(
        reqs.contains_key("geom"),
        "src/geom.lua must resolve as require(\"geom\"): {reqs:?}"
    );
    assert!(
        !reqs.contains_key("helper"),
        "a deep lib/util/helper.lua must NOT resolve as require(\"helper\") \
         (bundle parity, not path-suffix): {reqs:?}"
    );
}

#[test]
fn file_text_and_files_reflect_the_effective_content() {
    let mut host = host();
    host.apply_change(set("a.lua", GOOD));
    let a = Path::new("a.lua");
    assert_eq!(host.snapshot().file_text(a).as_deref(), Some(GOOD));

    // The overlay becomes the effective text.
    host.apply_change(Change::SetOverlay {
        path: PathBuf::from("a.lua"),
        text: BAD.to_owned(),
    });
    assert_eq!(host.snapshot().file_text(a).as_deref(), Some(BAD));
    assert!(
        host.snapshot()
            .file_text(Path::new("missing.lua"))
            .is_none()
    );

    let files: Vec<_> = host.snapshot().files().map(Path::to_path_buf).collect();
    assert_eq!(files, vec![PathBuf::from("a.lua")]);
}

#[test]
fn every_analysis_accessor_answers_for_a_known_file_and_none_for_an_unknown_one() {
    const MODULE: &str = "\
---@class Point
---@field x number
local M = {}
---@return number
function M.zero() return 0 end
return M
";

    let mut host = host();
    host.apply_change(set("m.lua", MODULE));
    let snap = host.snapshot();
    let m = Path::new("m.lua");
    let missing = Path::new("nope.lua");

    // Syntax surface.
    assert_eq!(snap.syntax(m).unwrap().text().to_string(), MODULE);
    assert!(snap.parse(m).unwrap().errors().is_empty());

    // Annotation / type surfaces.
    assert!(
        !snap.annotations(m).unwrap().items().is_empty(),
        "the ---@class block is harvested"
    );
    assert!(snap.type_env(m).is_some());

    // HIR surface: one chunk body plus the `M.zero` function body.
    assert_eq!(snap.lower(m).unwrap().file().bodies().count(), 2);

    // Inference surfaces.
    assert!(
        !snap.binding_types(m).unwrap().bindings().is_empty(),
        "`M` at least is a typed binding"
    );
    assert!(
        snap.module_export(m).unwrap().ty().is_some(),
        "the chunk returns M, so the module exports a type"
    );
    assert!(
        snap.project_types()
            .iter()
            .any(|t| format!("{t:?}").contains("Point")),
        "the workspace-global class contribution is visible"
    );

    // Every path-keyed accessor declines an unknown file rather than panicking.
    assert!(snap.syntax(missing).is_none());
    assert!(snap.parse(missing).is_none());
    assert!(snap.annotations(missing).is_none());
    assert!(snap.type_env(missing).is_none());
    assert!(snap.lower(missing).is_none());
    assert!(snap.binding_types(missing).is_none());
    assert!(snap.module_export(missing).is_none());
    assert!(snap.require_exports(missing).is_none());
    assert!(snap.diagnostics(missing).is_none());
}

#[test]
fn display_mode_inference_flows_across_a_require_in_both_directions() {
    // `main.lua` requires `m.lua`, so:
    //   * downstream — `main.lua`'s binding types see `m.lua`'s export;
    //   * upstream    — `m.lua`'s exported `M.f` gets its parameter seeded
    //                   from the `number` argument `main.lua` passes.
    let mut called = host();
    called.apply_changes([
        set(
            "m.lua",
            "local M = {}\nfunction M.f(x) return x end\nreturn M\n",
        ),
        set("main.lua", "local m = require(\"m\")\nlocal r = m.f(1)\n"),
    ]);

    let snap = called.snapshot();

    // Downstream: `m` in main.lua is the required module's table, not unknown.
    let types = snap.binding_types(Path::new("main.lua")).unwrap();
    let m_binding = types
        .bindings()
        .iter()
        .find(|b| b.name == "m")
        .expect("`m` is a binding");
    let rendered = format!("{:?}", m_binding.ty);
    assert!(
        rendered.contains('f'),
        "the required module's `f` member is visible: {rendered}"
    );

    // Upstream: `M.f` is `function(x) return x end`, so its return type is
    // only knowable from a caller. The dependent file's `m.f(1)` seeds it.
    let seeded = format!(
        "{:?}",
        snap.module_export(Path::new("m.lua"))
            .unwrap()
            .ty()
            .expect("m.lua exports its table")
    );
    assert!(
        seeded.contains("returns: [Integer]"),
        "the observed call argument flows into the exported return type: {seeded}"
    );

    // Drop the call site and the seed disappears — proving it came from the
    // dependent file, not from `m.lua` alone.
    let mut uncalled = host();
    uncalled.apply_changes([
        set(
            "m.lua",
            "local M = {}\nfunction M.f(x) return x end\nreturn M\n",
        ),
        set("main.lua", "local m = require(\"m\")\n"),
    ]);
    let unseeded = format!(
        "{:?}",
        uncalled
            .snapshot()
            .module_export(Path::new("m.lua"))
            .unwrap()
            .ty()
            .expect("m.lua exports its table")
    );
    assert!(
        unseeded.contains("returns: [Unknown]"),
        "with no caller there is nothing to seed from: {unseeded}"
    );
}

#[test]
fn parameter_seeds_union_across_callers_and_ignore_unknown_arguments() {
    const MODULE: &str = "local M = {}\nfunction M.f(x) return x end\nreturn M\n";

    let mut host = host();
    host.apply_changes([
        set("m.lua", MODULE),
        set("a.lua", "local m = require(\"m\")\nm.f(1)\n"),
        set("b.lua", "local m = require(\"m\")\nm.f(\"s\")\n"),
        // A third caller passing an undeclared global must not widen the seed.
        set(
            "c.lua",
            "local m = require(\"m\")\nm.f(some_undeclared_global)\n",
        ),
    ]);

    let exported = format!(
        "{:?}",
        host.snapshot()
            .module_export(Path::new("m.lua"))
            .unwrap()
            .ty()
            .expect("m.lua exports its table")
    );
    // The call sites are folded into one union rather than the last one
    // winning, and the seed stays concrete.
    assert!(
        exported.contains("Union"),
        "two differently-typed callers union their seeds: {exported}"
    );
    assert!(
        exported.contains("Integer") && exported.contains("String"),
        "{exported}"
    );
}

#[test]
fn set_root_rebases_require_resolution_and_is_visible_through_the_vfs() {
    let mut host = host();
    host.set_root(PathBuf::from("/workspace"));
    host.apply_changes([
        set("/workspace/src/util.lua", "local M = {}\nreturn M\n"),
        set("/workspace/main.lua", "local u = require(\"util\")\n"),
    ]);

    let snap = host.snapshot();
    let reqs = snap
        .require_exports(Path::new("/workspace/main.lua"))
        .expect("main.lua is known");
    assert!(
        reqs.contains_key("util"),
        "`require(\"util\")` resolves under <root>/src: {reqs:?}"
    );

    // The read-only VFS reflects the same interned set.
    assert_eq!(host.vfs().ids().count(), 2);
    assert!(
        host.vfs()
            .file_id(Path::new("/workspace/main.lua"))
            .is_some()
    );
}

#[test]
fn set_root_bumps_the_revision_like_any_other_input_write() {
    // The revision is the cache key consumers key derived state on (the LSP's
    // merged ambient layer), so it must move whenever an input does. `set_root`
    // writes a salsa input; two snapshots either side of a re-root comparing
    // equal would hand a re-rooted host a stale derivation with no signal.
    let mut host = host();
    let start = host.snapshot().revision();

    host.set_root(PathBuf::from("/workspace"));
    let rooted = host.snapshot().revision();
    assert!(
        rooted > start,
        "set_root moved an input: {start} -> {rooted}"
    );

    // Reading, not writing, leaves it alone.
    assert_eq!(host.snapshot().revision(), rooted);

    host.apply_change(set("/workspace/main.lua", GOOD));
    assert!(host.snapshot().revision() > rooted);

    // A re-root is a re-root even when the path is one the host has seen.
    let before = host.snapshot().revision();
    host.set_root(PathBuf::from("/workspace"));
    assert!(host.snapshot().revision() > before);
}

#[test]
fn clearing_an_overlay_reverts_to_disk_and_ignores_unknown_paths() {
    let mut host = host();
    host.apply_change(set("a.lua", GOOD));
    host.apply_change(Change::SetOverlay {
        path: PathBuf::from("a.lua"),
        text: BAD.to_owned(),
    });
    assert_eq!(
        host.snapshot()
            .diagnostics(Path::new("a.lua"))
            .unwrap()
            .len(),
        1
    );

    host.apply_change(Change::ClearOverlay {
        path: PathBuf::from("a.lua"),
    });
    assert_eq!(
        host.snapshot().file_text(Path::new("a.lua")).as_deref(),
        Some(GOOD)
    );
    assert!(
        host.snapshot()
            .diagnostics(Path::new("a.lua"))
            .unwrap()
            .is_empty()
    );

    // Clearing an overlay on a path the VFS never interned is a no-op, not a
    // panic and not a new file.
    host.apply_change(Change::ClearOverlay {
        path: PathBuf::from("never-seen.lua"),
    });
    assert_eq!(host.snapshot().files().count(), 1);
}

#[test]
fn display_mode_resolves_a_required_carriers_class_across_files() {
    // F44 (round 3 review) — regression: `module_export`/`binding_types` build
    // their ambient as `stdlib_defs(dialect)` alone, with no
    // `with_project_types` merge, unlike the check-mode path. Since #56 a
    // declared `---@class` carrier crosses `require` as `Ty::Named(name)`
    // rather than a structural table, so resolving anything about it in the
    // *consumer's* display-mode inference (inlay hints) needs the carrier's
    // own class declaration in scope — which lives in a DIFFERENT file, and
    // was never merged in. Before #56 this never mattered: the export was
    // already fully structural, needing no further class lookup.
    //
    // `widget.lua` declares a carrier class; `main.lua` requires it and
    // calls its constructor. The constructor's return type is `Widget`
    // (`Infer::reify_shape`'s instance-identity rule), which the display
    // inference must resolve back to its `id: number` field to report
    // anything useful for `v` at all — with the merge missing, `Widget`
    // resolves to `Lookup::Opaque` (`infer.rs`) and the binding renders as
    // unknown.
    let widget = "\
---@class Widget
---@field id number
local W = {}
W.__index = W
---@return Widget
function W.make() return setmetatable({}, W) end
return W
";
    let main = "\
local m = require(\"widget\")
local v = m.make()
";
    let mut host = host();
    host.apply_changes([set("widget.lua", widget), set("main.lua", main)]);

    let snap = host.snapshot();
    let types = snap.binding_types(Path::new("main.lua")).unwrap();
    let v_binding = types
        .bindings()
        .iter()
        .find(|b| b.name == "v")
        .expect("`v` is a binding");
    let rendered = format!("{:?}", v_binding.ty);
    assert!(
        rendered.contains("Widget") || rendered.contains("\"id\""),
        "`v`'s type must resolve through the cross-file carrier class, not \
         stay opaque/unknown: {rendered}"
    );
}

#[test]
fn a_cross_file_generic_ancestor_does_not_leak_its_parameter_through_require_exports() {
    // F42/F43 (round 3 review): the export-seam fix (#56) erases an unbound
    // generic parameter at the `require` boundary by asking
    // `class_params_in_scope` whether the exported class still has one free
    // — but the round-2 fix computed that answer against a defs-only
    // ambient (`module_surface_checked`), which cannot see a *different*
    // project file's classes. `base.lua` declares the generic ancestor;
    // `sub.lua` inherits it bare (`: Base`, parameter left unbound) and
    // returns the carrier; `main.lua` requires `sub` and reads the
    // inherited field. Every existing fixture for this guard
    // (`cross_file_require.rs`) declares the ancestor and the child in ONE
    // file, where `class_params_in_scope` can already see the ancestor —
    // this is the shape that actually crosses the export seam.
    let base = "\
---@class Base<U>
---@field item U
local M = {}
return M
";
    let sub = "\
---@class Sub : Base
local S = {}
return S
";
    let main = "\
---@param n number
local function want(n) end
local s = require(\"sub\")
want(s.item)
";
    let mut host = host();
    host.set_root(PathBuf::from("/proj"));
    host.apply_changes([
        set("/proj/base.lua", base),
        set("/proj/sub.lua", sub),
        set("/proj/main.lua", main),
    ]);

    let snap = host.snapshot();
    let requires = snap
        .require_exports(Path::new("/proj/main.lua"))
        .expect("main.lua is a known file");
    let project_types = snap.project_types();
    let ambient = stdlib_defs(Dialect::Lua54).with_project_types(project_types.iter());

    let parsed = lua::parse(main, Dialect::Lua54);
    let diags = check_file_with_requires(
        &parsed,
        "main.lua",
        Strictness::Strict,
        Dialect::Lua54,
        Some(&ambient),
        &requires,
    );
    let codes: Vec<String> = diags.iter().map(|d| d.code.to_string()).collect();
    assert_eq!(
        codes,
        vec!["LB0300".to_string()],
        "the inherited, still-unbound parameter must erase to `unknown` — a \
         cross-file leak reports `found \\`U\\`` here instead: {diags:?}"
    );
    assert!(
        diags[0].message.contains("found `unknown`"),
        "the parameter name must never reach the consumer: {}",
        diags[0].message
    );
}

#[test]
fn a_watch_event_delivering_unchanged_text_does_not_bump_the_revision() {
    // R17 (round 4 review): `workspace/didChangeWatchedFiles` (`server.rs`)
    // calls `apply_change(SetFileText { .. })` for every non-deleted `.lua`
    // path named in a watch event, whether or not the on-disk text actually
    // differs from what the host already has — a save-without-edit, an
    // unrelated watcher coalescing multiple events, a `touch`. Every
    // revision-keyed cache downstream (the LSP's `MergedAmbient`,
    // `server.rs:1583`) treats any bump as "the world changed" and pays a
    // full `clone_surface` + `merge_file_types` over every project file.
    // Re-delivering identical text must not move the revision; text that
    // actually differs still must.
    let mut host = host();
    host.apply_change(set("a.lua", GOOD));
    let after_first_write = host.snapshot().revision();

    // The exact re-delivery the watcher performs: same path, same dialect,
    // byte-identical text.
    host.apply_change(set("a.lua", GOOD));
    assert_eq!(
        host.snapshot().revision(),
        after_first_write,
        "re-delivering unchanged text must not bump the revision"
    );

    // Text that actually differs still must.
    host.apply_change(set("a.lua", BAD));
    assert!(
        host.snapshot().revision() > after_first_write,
        "an actual edit must still bump the revision"
    );
}

#[test]
fn a_strictness_no_op_does_not_bump_the_revision() {
    // Same class of fix as the watch-event case above (R17): re-applying the
    // strictness the project already has must not move the revision either.
    let mut host = host();
    let start = host.snapshot().revision();

    host.apply_change(Change::SetStrictness(Strictness::Strict));
    assert_eq!(
        host.snapshot().revision(),
        start,
        "the host defaults to Strict; setting it to Strict again is a no-op"
    );

    host.apply_change(Change::SetStrictness(Strictness::Warn));
    assert!(
        host.snapshot().revision() > start,
        "an actual strictness change must still bump the revision"
    );
}

#[test]
fn project_types_checked_is_memoized_once_across_a_display_pass_over_many_files() {
    // R14 (round 4 review): `project_types_checked` merges every project
    // file's workspace-global class/enum contribution — the input
    // `Ambient::with_project_types` folds beneath each file's own
    // declarations. It has three per-file callers (`module_export`,
    // `binding_types`, `module_export_checked`), each itself a tracked
    // query keyed on `(file, project)`. Before it was a tracked query in
    // its own right, a display pass over N files (the LSP's inlay-hint
    // sweep on open) called it N times, and it rebuilt the whole
    // `Vec<FileTypes>` collection — and the merge each caller runs over it
    // — from scratch every single time: O(N) work per file it touched,
    // O(N²) total, even though every `module_surface_checked` it reads is
    // itself already memoized. Tracking it collapses that to one execution
    // per project revision, shared by every caller.
    const N: usize = 25;
    let mut host = host();
    let changes: Vec<Change> = (0..N)
        .map(|i| {
            set(
                &format!("f{i}.lua"),
                &format!("---@class C{i}\n---@field x number\nlocal M = {{}}\nreturn M\n"),
            )
        })
        .collect();
    host.apply_changes(changes);
    let _ = host.take_execution_log();

    // One display pass, touching every file's `binding_types` for the
    // first time at this revision — exactly what an LSP opening an N-file
    // workspace does.
    let snap = host.snapshot();
    for i in 0..N {
        let _ = snap.binding_types(Path::new(&format!("f{i}.lua")));
    }
    let log = host.take_execution_log();

    let types_runs = mentioning(&log, "project_types_checked()");
    assert_eq!(
        types_runs.len(),
        1,
        "the project-wide types merge must execute once per revision, not \
         once per file touched ({N} files, log: {log:?})"
    );
}

/// M18 (round 6 review): `project_types_checked_is_memoized_once_across_a_display_pass_over_many_files`
/// above only counts `project_types_checked()` — the *collection* of every
/// file's `FileTypes`. That collection was already memoized; the bug is the
/// `Ambient::with_project_types` *merge* built over it, which
/// `module_export`/`binding_types`/`module_export_checked` each used to run
/// again from scratch on every one of their N per-file calls — invisible to
/// a test that only ever counts the collection step. This counts the merge
/// itself (`project_ambient()`, `query.rs`'s newly-tracked query) across the
/// identical N-file display pass, and fails the same way the collection-only
/// test would have failed to catch: red before `project_ambient` existed
/// (the merge ran once per file, N times), green after (once per revision).
#[test]
fn project_ambient_merge_is_memoized_once_across_a_display_pass_over_many_files() {
    const N: usize = 25;
    let mut host = host();
    let changes: Vec<Change> = (0..N)
        .map(|i| {
            set(
                &format!("f{i}.lua"),
                &format!("---@class C{i}\n---@field x number\nlocal M = {{}}\nreturn M\n"),
            )
        })
        .collect();
    host.apply_changes(changes);
    let _ = host.take_execution_log();

    let snap = host.snapshot();
    for i in 0..N {
        let _ = snap.binding_types(Path::new(&format!("f{i}.lua")));
    }
    let log = host.take_execution_log();

    let merge_runs = mentioning(&log, "project_ambient()");
    assert_eq!(
        merge_runs.len(),
        1,
        "the `with_project_types` merge itself must execute once per \
         revision, not once per file touched ({N} files, log: {log:?})"
    );
}

/// M18's own real-numbers sweep: a full `binding_types` display pass (the
/// LSP's inlayHint sweep) at N=20/80/160, wall-clock. Manual — run
/// explicitly with `cargo test -p luabox-db --release -- --ignored \
/// --nocapture project_types_sweep_wall_time` — since a wall-clock
/// assertion in the default suite would be flaky across machines; the
/// round 6 report carries the numbers this prints. This probe backs no CI
/// claim (round 8 review F10): M18's FUNCTIONAL pin is the non-ignored
/// `project_ambient_merge_is_memoized_once_across_a_display_pass_over_many_files`
/// above — this one only puts wall-clock numbers on it by hand.
#[test]
#[ignore = "manual wall-clock measurement, see the doc comment"]
fn project_types_sweep_wall_time() {
    for n in [20usize, 80, 160] {
        let mut host = host();
        let changes: Vec<Change> = (0..n)
            .map(|i| {
                set(
                    &format!("f{i}.lua"),
                    &format!("---@class C{i}\n---@field x number\nlocal M = {{}}\nreturn M\n"),
                )
            })
            .collect();
        host.apply_changes(changes);
        let snap = host.snapshot();
        let start = std::time::Instant::now();
        for i in 0..n {
            let _ = snap.binding_types(Path::new(&format!("f{i}.lua")));
        }
        let elapsed = start.elapsed();
        eprintln!("N={n}: {elapsed:?}");
    }
}

#[test]
fn set_dialect_reparses_the_file_under_the_new_dialect() {
    // LuaJIT-only `0x10ULL` is a parse error under 5.4.
    const JIT_ONLY: &str = "local n = 10ULL\n";

    let mut host = host();
    host.apply_change(set("a.lua", JIT_ONLY));
    assert!(
        !host
            .snapshot()
            .parse(Path::new("a.lua"))
            .unwrap()
            .errors()
            .is_empty(),
        "the LuaJIT suffix does not parse as Lua 5.4"
    );

    host.apply_change(Change::SetDialect {
        path: PathBuf::from("a.lua"),
        dialect: Dialect::LuaJit,
    });
    assert!(
        host.snapshot()
            .parse(Path::new("a.lua"))
            .unwrap()
            .errors()
            .is_empty(),
        "under LuaJIT the same source is clean"
    );
}

#[test]
fn project_types_shares_its_allocation_across_calls_instead_of_deep_cloning() {
    // Round 4 review finding 2: `Host::project_types()` used to end in
    // `.types().to_vec()`, deep-cloning every project file's class/enum/
    // alias maps on every single call. Two calls against the *same*
    // snapshot revision now read the identical memoized `ProjectTypes`
    // (an `Arc`-backed wrapper) and hand back a cheap `Arc` clone rather
    // than a fresh `Vec`, so the two results' backing storage is the exact
    // same allocation — measured here by comparing the slice's data
    // pointer, not merely its contents (equal *contents* would pass even
    // with the old deep clone; equal *pointer* would not).
    const MODULE: &str = "\
---@class Point
---@field x number
local M = {}
return M
";
    let mut host = host();
    host.apply_change(set("m.lua", MODULE));
    let snap = host.snapshot();

    let first = snap.project_types();
    let second = snap.project_types();
    assert!(
        !first.is_empty(),
        "the fixture declares a class, so the contribution must be non-empty"
    );
    assert_eq!(
        first.as_ptr(),
        second.as_ptr(),
        "two calls at the same revision must share one allocation, not each \
         deep-clone their own"
    );
}

/// Round 5 review N23: the execution trace has no non-test drainer, so a
/// long-running session that never calls `take_execution_log` used to grow
/// it by roughly one entry per query invocation, per revision, forever —
/// measured, a linear ~0.84 KiB/keystroke RSS climb over 3000 edits with no
/// plateau. Simulate exactly that: many more revisions, each touching a
/// fresh file (so each one logs real entries), than the cap — without ever
/// draining in between — and assert the trace stayed at its ceiling instead
/// of growing past it.
#[test]
fn execution_log_never_grows_past_its_cap() {
    let mut host = host();
    let edits = MAX_EXECUTION_LOG_ENTRIES * 4;
    for i in 0..edits {
        host.apply_change(set(&format!("f{i}.lua"), GOOD));
        let _ = host.snapshot().diagnostics(Path::new(&format!("f{i}.lua")));
    }

    let log = host.take_execution_log();

    assert!(
        log.len() <= MAX_EXECUTION_LOG_ENTRIES,
        "execution log grew past its cap of {MAX_EXECUTION_LOG_ENTRIES}: {} entries \
         after {edits} undrained revisions — the bound regressed",
        log.len()
    );
}

/// M46 (round 6 review): the cap above silently broke the trace's original
/// "complete list of queries since the last drain" invariant — a query
/// whose entry was evicted to make room for newer ones is now
/// indistinguishable, from the drained `Vec<String>` alone, from a query
/// that genuinely never ran. This proves both halves: first, that the trap
/// is real (the probed query's own entry really is silently absent from the
/// raw log after enough undrained revisions to overflow the cap); second,
/// that `execution_log_overflowed` correctly flags it and `assert_absent`
/// refuses to certify the absence once it is set — where an un-guarded
/// `mentioning(&log, needle).is_empty()` check would have passed, wrongly,
/// exactly as it would for a query that truly never ran.
#[test]
fn an_absence_assertion_over_an_overflowed_trace_is_not_trusted() {
    let mut host = host();

    // One revision whose own entry we will probe for later.
    host.apply_change(set("probe.lua", GOOD));
    let _ = host.snapshot().diagnostics(Path::new("probe.lua"));
    assert!(
        !host.execution_log_overflowed(),
        "nothing has overflowed yet"
    );

    // Push far more undrained revisions than the cap holds, so the probe's
    // own entry — logged first, evicted first — ages out.
    for i in 0..(MAX_EXECUTION_LOG_ENTRIES * 2) {
        host.apply_change(set(&format!("filler{i}.lua"), GOOD));
        let _ = host
            .snapshot()
            .diagnostics(Path::new(&format!("filler{i}.lua")));
    }

    // Read the flag *before* draining — draining resets it.
    let overflowed = host.execution_log_overflowed();
    assert!(
        overflowed,
        "this many undrained pushes must overflow the cap"
    );

    let log = host.take_execution_log();
    assert!(
        mentioning(&log, "diagnostics(probe.lua)").is_empty(),
        "the probe's own entry really was evicted — this is the false-\
         absence trap M46 describes: {log:?}"
    );

    // The dedicated helper must refuse to certify that absence.
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_absent(&log, overflowed, "diagnostics(probe.lua)");
    }));
    assert!(
        refused.is_err(),
        "assert_absent must panic rather than certify an absence over an \
         overflowed trace"
    );
}
