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
