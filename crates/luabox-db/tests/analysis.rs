//! Boundary + incrementality tests for the analysis database.
//!
//! These drive the crate exactly as the LSP will: build an [`AnalysisHost`],
//! [`apply_change`](AnalysisHost::apply_change), take a
//! [`snapshot`](AnalysisHost::snapshot), and read results. The execution trace
//! ([`AnalysisHost::take_execution_log`]) is used to prove *which* queries
//! actually recompute after an edit.

use std::path::{Path, PathBuf};

use luabox_db::{AnalysisHost, Change, Dialect, Strictness};
use luabox_syntax::lua;
use luabox_types::check_file;

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
