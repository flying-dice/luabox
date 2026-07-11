//! Integration tests for the bench runner, driven two ways (mirrors
//! `tests/runner.rs`):
//!
//!   * a **fake runtime** — a `.bat`/`sh` shim that echoes each bench file's
//!     contents (authored as raw `LUABOX_BENCH_*` protocol) and always
//!     exits 0. Proves discovery/aggregation/stats **hermetically**, with
//!     no Lua required.
//!   * the **real runtime**, if a Lua is on PATH — a genuine `bench()`
//!     suite measuring a real loop, run end-to-end. Skipped (with a
//!     printed note) when no Lua is installed.

use std::path::{Path, PathBuf};

use luabox_test::bench::{FileOutcome, RuntimeReport, SuiteOptions, discover, render, run_suite};
use luabox_test::runtime::{RuntimeSpec, find_on_path};

/// Write a fake runtime shim into `dir` and return a `RuntimeSpec` for it —
/// a `.bat` on Windows, a `#!/bin/sh` script elsewhere. The runner appends
/// `<harness> <benchfile>`; the shim ignores the harness (arg 1), echoes the
/// bench file (arg 2, already containing protocol lines) and always exits
/// 0 — benches never fail the build.
fn fake_runtime(dir: &Path) -> RuntimeSpec {
    let shim = if cfg!(windows) {
        let bat = dir.join("fake_bench_runtime.bat");
        std::fs::write(&bat, "@echo off\r\ntype \"%~2\"\r\nexit /b 0\r\n").unwrap();
        bat
    } else {
        let sh = dir.join("fake_bench_runtime.sh");
        std::fs::write(&sh, "#!/bin/sh\ncat \"$2\"\nexit 0\n").unwrap();
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

/// A protocol bench file with one bench reporting the given ns/iter
/// samples.
fn bench_file(name: &str, samples: &[f64]) -> String {
    use std::fmt::Write as _;
    let mut out = format!("LUABOX_BENCH_BEGIN\t{name}\n");
    for s in samples {
        let _ = writeln!(out, "LUABOX_BENCH_RESULT\t{name}\t{s}");
    }
    out.push_str("LUABOX_BENCH_DONE\t1\n");
    out
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
fn fake_runtime_reports_stats_for_a_single_bench() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let files = vec![write(
        root,
        "fib_bench.lua",
        &bench_file("fib(20)", &[10.0, 12.0, 11.0, 13.0, 9.0]),
    )];
    let opts = SuiteOptions {
        files: &files,
        root,
    };

    let report = run_suite(&runtime, &opts);
    assert_eq!(report.files.len(), 1);
    let file = &report.files[0];
    assert!(
        file.error.is_none(),
        "unexpected file error: {:?}",
        file.error
    );
    assert_eq!(file.benches.len(), 1);
    let s = &file.benches[0];
    assert_eq!(s.name, "fib(20)");
    assert_eq!(s.samples, 5);
    assert!((s.mean_ns - 11.0).abs() < 1e-9);
    assert!((s.min_ns - 9.0).abs() < 1e-9);

    let text = render(&[report]);
    assert!(text.contains("fib(20)"));
    assert!(text.contains("fake"));
}

#[test]
fn fake_runtime_aggregates_multiple_files_in_parallel() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let mut files = Vec::new();
    for i in 0..10 {
        files.push(write(
            root,
            &format!("b{i}_bench.lua"),
            &bench_file(&format!("case {i}"), &[100.0, 110.0, 105.0]),
        ));
    }
    let opts = SuiteOptions {
        files: &files,
        root,
    };
    let report = run_suite(&runtime, &opts);
    assert_eq!(report.files.len(), 10);
    let total_benches: usize = report.files.iter().map(|f| f.benches.len()).sum();
    assert_eq!(total_benches, 10);
}

#[test]
fn fake_runtime_surfaces_reported_bench_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let runtime = fake_runtime(root);

    let files = vec![write(
        root,
        "broken_bench.lua",
        "LUABOX_BENCH_BEGIN\tsad\nLUABOX_BENCH_ERROR\tsad\tsetup exploded\nLUABOX_BENCH_DONE\t0\n",
    )];
    let opts = SuiteOptions {
        files: &files,
        root,
    };
    let report = run_suite(&runtime, &opts);
    let file = &report.files[0];
    assert!(file.error.is_none());
    assert_eq!(file.errors.len(), 1);
    assert_eq!(file.errors[0].name, "sad");
    assert_eq!(file.errors[0].message, "setup exploded");

    let text = render(&[report]);
    assert!(text.contains("errors:"));
    assert!(text.contains("setup exploded"));
}

#[test]
fn discover_finds_bench_files_in_a_real_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/math_bench.lua", "");
    write(root, "bench/deep/thing.lua", "");
    write(root, "src/math.lua", "");

    let mut found: Vec<String> = discover(root, None)
        .iter()
        .map(|f| {
            f.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();
    assert_eq!(found, vec!["bench/deep/thing.lua", "src/math_bench.lua"]);
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
                program: resolved.to_string_lossy().into_owned(),
                args: Vec::new(),
            });
        }
    }
    None
}

fn find_bench<'a>(
    report: &'a RuntimeReport,
    name: &str,
) -> Option<&'a luabox_test::bench::BenchStats> {
    report
        .files
        .iter()
        .flat_map(|f: &FileOutcome| &f.benches)
        .find(|b| b.name == name)
}

#[test]
fn real_lua_measures_a_real_loop_with_plausible_positive_ns() {
    let Some(runtime) = real_lua() else {
        eprintln!("SKIP: no Lua runtime on PATH; real bench end-to-end test skipped");
        return;
    };
    eprintln!("running real bench end-to-end on `{}`", runtime.program);

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // A plain loop bench, plus one using `setup` to build a fixture and
    // `iters` to pin the batch size, exercising the full Lua-side API.
    write(
        root,
        "loop_bench.lua",
        r#"
bench("sum loop", function()
  local x = 0
  for i = 1, 1000 do
    x = x + i
  end
end)

bench("sum with fixture", { setup = function() return { 1, 2, 3, 4, 5 } end, iters = 200000 },
  function(state)
    local total = 0
    for i = 1, #state do
      total = total + state[i]
    end
  end)
"#,
    );

    let files = vec![root.join("loop_bench.lua")];
    let opts = SuiteOptions {
        files: &files,
        root,
    };
    let report = run_suite(&runtime, &opts);
    let file = &report.files[0];
    assert!(
        file.error.is_none(),
        "bench file should not error: {:?}",
        file.error
    );
    assert!(
        file.errors.is_empty(),
        "no bench should error: {:?}",
        file.errors
    );

    let sum_loop = find_bench(&report, "sum loop").expect("sum loop bench ran");
    assert!(sum_loop.samples >= 1, "expected at least one batch");
    assert!(
        sum_loop.mean_ns > 0.0 && sum_loop.mean_ns.is_finite(),
        "expected a plausible positive ns/iter, got {}",
        sum_loop.mean_ns
    );
    // Sanity ceiling: a 1000-iteration integer-add loop should never
    // average anywhere near a second per call.
    assert!(
        sum_loop.mean_ns < 1_000_000_000.0,
        "ns/iter implausibly large: {}",
        sum_loop.mean_ns
    );

    let fixture_bench = find_bench(&report, "sum with fixture").expect("fixture bench ran");
    assert!(fixture_bench.mean_ns > 0.0);
    assert_eq!(
        fixture_bench.samples, 10,
        "iters override still runs the default batch-count floor"
    );

    assert!(report.files.iter().any(|f| f.version.is_some()));

    let text = render(&[report]);
    assert!(text.contains("sum loop"));
    assert!(text.contains("sum with fixture"));
}

#[test]
fn real_lua_reports_a_bench_error_without_failing_the_file() {
    let Some(runtime) = real_lua() else {
        eprintln!("SKIP: no Lua runtime on PATH; bench-error test skipped");
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "erroring_bench.lua",
        r#"
bench("boom", function()
  error("kaboom")
end)

bench("fine", function()
  local x = 1 + 1
end)
"#,
    );

    let files = vec![root.join("erroring_bench.lua")];
    let opts = SuiteOptions {
        files: &files,
        root,
    };
    let report = run_suite(&runtime, &opts);
    let file = &report.files[0];
    assert!(file.error.is_none(), "file itself must not error");
    assert_eq!(file.errors.len(), 1);
    assert!(file.errors[0].message.contains("kaboom"));
    assert!(
        find_bench(&report, "fine").is_some(),
        "a later bench must still run after an earlier one errors"
    );
}
