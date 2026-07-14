//! Integration tests for the runner, driven two ways:
//!
//!   * a **fake runtime** — a `.bat`/`sh` shim that just echoes each test file's
//!     contents (which we author as raw protocol) and exits nonzero when it
//!     sees a FAIL line. This proves discovery/aggregation/exit-code/
//!     parallel handling **hermetically**, with no Lua required.
//!   * the **real runtime**, if a Lua is on PATH — a genuine `describe/it`
//!     suite and a flat `test()` suite run end-to-end. Skipped (with a
//!     printed note) when no Lua is installed.
//!
//! The fake runtime works because `RuntimeSpec` is fully injectable: the
//! runner only ever spawns `<program> <args…> <harness> <file>`.

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::path::{Path, PathBuf};

use luabox_test::protocol::Outcome;
use luabox_test::run_suite;
use luabox_test::runner::{RuntimeReport, SuiteOptions};
use luabox_test::runtime::{RuntimeSpec, find_on_path};

/// Write a fake runtime shim into `dir` and return a `RuntimeSpec` for it —
/// a `.bat` on Windows, a `#!/bin/sh` script elsewhere. The runner appends
/// `<harness> <testfile>`; the shim ignores the harness (arg 1), echoes the
/// test file (arg 2, already containing protocol lines), and exits 1 if that
/// file contains a FAIL line.
fn fake_runtime(dir: &Path) -> RuntimeSpec {
    let shim = if cfg!(windows) {
        let bat = dir.join("fake_runtime.bat");
        let script = "@echo off\r\n\
            type \"%~2\"\r\n\
            findstr /C:\"LUABOX_TEST_FAIL\" \"%~2\" >nul\r\n\
            if not errorlevel 1 exit /b 1\r\n\
            exit /b 0\r\n";
        std::fs::write(&bat, script).unwrap();
        bat
    } else {
        let sh = dir.join("fake_runtime.sh");
        let script =
            "#!/bin/sh\ncat \"$2\"\nif grep -q LUABOX_TEST_FAIL \"$2\"; then exit 1; fi\nexit 0\n";
        std::fs::write(&sh, script).unwrap();
        make_executable(&sh);
        sh
    };
    RuntimeSpec {
        label: "fake".to_string(),
        program: shim.to_string_lossy().into_owned(),
        args: Vec::new(),
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// A protocol test file with a single passing case.
fn passing_file(name: &str) -> String {
    format!("LUABOX_TEST_BEGIN\t{name}\nLUABOX_TEST_PASS\t{name}\nLUABOX_TEST_DONE\t1\t0\n")
}

/// A protocol test file with a single failing case carrying a message.
fn failing_file(name: &str, message: &str) -> String {
    format!(
        "LUABOX_TEST_BEGIN\t{name}\nLUABOX_TEST_FAIL\t{name}\t{message}\nLUABOX_TEST_DONE\t0\t1\n"
    )
}

fn write(root: &Path, rel: &str, content: &str) -> PathBuf {
    let full = root.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, content).unwrap();
    full
}

#[test]
fn fake_runtime_aggregates_pass_and_fail() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let files = vec![
        write(root, "alpha_test.lua", &passing_file("alpha ok")),
        write(
            root,
            "beta_test.lua",
            &failing_file("beta broke", "expected 1 but got 2"),
        ),
    ];
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root,
    };

    let report = run_suite(&runtime, &opts);
    assert_eq!(report.passed(), 1, "one case should pass");
    assert_eq!(report.failed(), 1, "one case should fail");

    // The failure message must survive to the report.
    let (text, summary) = luabox_test::render(&[report], luabox_test::Layout::Flat);
    assert!(text.contains("PASS alpha ok"), "text:\n{text}");
    assert!(text.contains("FAIL beta broke"), "text:\n{text}");
    assert!(text.contains("expected 1 but got 2"), "text:\n{text}");
    assert!(!summary.is_ok());
}

#[test]
fn fake_runtime_all_passing_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let files = vec![
        write(root, "a_test.lua", &passing_file("a")),
        write(root, "b_test.lua", &passing_file("b")),
    ];
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.passed(), 2);
    assert_eq!(report.failed(), 0);
    let (_text, summary) = luabox_test::render(&[report], luabox_test::Layout::Flat);
    assert!(summary.is_ok());
}

#[test]
fn fake_runtime_handles_many_files_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    // Enough files to exercise the rayon fan-out; half fail.
    let mut files = Vec::new();
    for i in 0..20 {
        if i % 2 == 0 {
            files.push(write(
                root,
                &format!("pass_{i}_test.lua"),
                &passing_file(&format!("case {i}")),
            ));
        } else {
            files.push(write(
                root,
                &format!("fail_{i}_test.lua"),
                &failing_file(&format!("case {i}"), "boom"),
            ));
        }
    }
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.passed(), 10);
    assert_eq!(report.failed(), 10);
}

#[test]
fn path_pattern_selects_a_subset_of_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let files = vec![
        write(root, "alpha_test.lua", &passing_file("alpha")),
        write(root, "beta_test.lua", &failing_file("beta", "boom")),
    ];
    // Pattern matches only alpha's path → only alpha runs → the run is ok.
    let opts = SuiteOptions {
        files: &files,
        pattern: Some("alpha"),
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.passed(), 1);
    assert_eq!(report.failed(), 0);
    assert_eq!(report.files.len(), 1);
}

// ------------------------------------------------------------------ --
// Real-runtime end-to-end (skipped without a Lua on PATH).
// ------------------------------------------------------------------ --

fn real_lua() -> Option<RuntimeSpec> {
    for name in [
        "lua5.4", "lua54", "lua5.3", "lua5.1", "lua51", "luajit", "lua",
    ] {
        if let Some(resolved) = find_on_path(name) {
            return Some(RuntimeSpec {
                label: name.to_string(),
                // Resolved path, not the bare name — Windows CreateProcess
                // won't append `.exe` to `lua5.1` (looks pre-extensioned).
                program: resolved.to_string_lossy().into_owned(),
                args: Vec::new(),
            });
        }
    }
    None
}

fn count(report: &RuntimeReport, name: &str) -> Option<Outcome> {
    report
        .files
        .iter()
        .flat_map(|f| &f.cases)
        .find(|c| c.name == name)
        .map(|c| c.outcome.clone())
}

#[test]
fn real_lua_runs_describe_it_and_flat_suites() {
    let Some(runtime) = real_lua() else {
        eprintln!("SKIP: no Lua runtime on PATH; real end-to-end test skipped");
        return;
    };
    eprintln!("running real end-to-end suite on `{}`", runtime.program);

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // A busted-style suite: describe/it, before_each, and assert.* shims.
    write(
        root,
        "busted_test.lua",
        r#"
describe("math", function()
  local base
  before_each(function() base = 10 end)

  it("adds", function()
    assert.equal(12, base + 2)
  end)

  it("detects a bad sum", function()
    assert.equal(99, base + 2)
  end)
end)

describe("errors", function()
  it("raises", function()
    assert.has_error(function() error("kaboom") end, "kaboom")
  end)

  it("deep-compares tables", function()
    assert.same({ a = 1, b = { 2, 3 } }, { a = 1, b = { 2, 3 } })
  end)
end)
"#,
    );

    // A flat native-API suite.
    write(
        root,
        "flat_test.lua",
        r#"
test("truthy passes", function()
  assert.is_true(1 == 1)
end)

test("nil check", function()
  assert.is_nil(nil)
end)
"#,
    );

    let files = vec![root.join("busted_test.lua"), root.join("flat_test.lua")];
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root,
    };
    let report = run_suite(&runtime, &opts);

    // Exactly one case is designed to fail.
    assert_eq!(report.passed(), 5, "expected 5 passing cases");
    assert_eq!(report.failed(), 1, "expected 1 failing case");

    assert_eq!(count(&report, "math adds"), Some(Outcome::Pass));
    assert!(matches!(
        count(&report, "math detects a bad sum"),
        Some(Outcome::Fail(_))
    ));
    assert_eq!(count(&report, "errors raises"), Some(Outcome::Pass));
    assert_eq!(
        count(&report, "errors deep-compares tables"),
        Some(Outcome::Pass)
    );
    assert_eq!(count(&report, "truthy passes"), Some(Outcome::Pass));
    assert_eq!(count(&report, "nil check"), Some(Outcome::Pass));

    // The harness announced a runtime version.
    assert!(report.files.iter().any(|f| f.version.is_some()));
}

#[test]
fn real_lua_name_filter_runs_only_matching_cases() {
    let Some(runtime) = real_lua() else {
        eprintln!("SKIP: no Lua runtime on PATH; name-filter test skipped");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "filter_test.lua",
        r#"
test("keep me", function() assert.is_true(true) end)
test("skip me", function() assert.is_true(false) end)
"#,
    );

    let files = vec![root.join("filter_test.lua")];
    // "keep me" isn't a substring of the file path, so it becomes a name
    // filter; only the matching case runs (and the would-be failing one is
    // never executed).
    let opts = SuiteOptions {
        files: &files,
        pattern: Some("keep me"),
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.passed(), 1);
    assert_eq!(report.failed(), 0);
    assert_eq!(count(&report, "keep me"), Some(Outcome::Pass));
    assert!(
        count(&report, "skip me").is_none(),
        "skipped case must not run"
    );
}

#[test]
fn real_lua_reports_a_load_error_as_a_failure() {
    let Some(runtime) = real_lua() else {
        eprintln!("SKIP: no Lua runtime on PATH; load-error test skipped");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Syntax error at load time.
    write(root, "broken_test.lua", "this is not lua = = =\n");

    let files = vec![root.join("broken_test.lua")];
    let opts = SuiteOptions {
        files: &files,
        pattern: None,
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.failed(), 1, "a load error must count as a failure");
}
