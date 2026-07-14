//! Statistical benchmarking across runtimes (SPEC.md §11, ticket #26).
//!
//! Self-contained, additive sibling of the test-runner modules
//! ([`crate::discovery`], [`crate::protocol`], [`crate::runner`],
//! [`crate::report`]): discovery, the harness line protocol, batch
//! statistics, process fan-out and human rendering all live here rather
//! than being threaded through those modules, so `luabox bench` can land
//! without touching `luabox test`'s code paths.
//!
//! `luabox bench` always compares every runtime found on `PATH` (plus
//! `LUABOX_LUA`, if set) — see [`crate::runtime::resolve_matrix`] — because
//! cross-runtime comparison *is* the feature (SPEC.md §11: "criterion-style
//! statistical benchmarking across runtimes").
//!
//! ## Discovery
//!
//! A `.lua` file is a bench file if its name ends with `_bench.lua` or
//! `.bench.lua`, or it lives anywhere under a directory named `bench/`
//! (searched recursively, mirroring `tests/` in [`crate::discovery`]).
//! `*.d.lua` definition files are never benches.
//!
//! ## Lua-side API
//!
//! ```lua
//! bench("name", function() ... end)
//! bench("name", { setup = fn, iters = n }, function(state) ... end)
//! ```
//!
//! `setup`, if given, runs once (before warmup) to build a fixture; its
//! return value is passed as the sole argument to the benched function on
//! every call, warmup and timed alike. `iters`, if given, fixes the batch
//! size and skips adaptive calibration — useful when a benchmark's cost is
//! already known or must be pinned for reproducibility.
//!
//! ## Harness protocol ("criterion-lite")
//!
//! Per bench, in [`BENCH_HARNESS_SOURCE`]:
//!
//!  1. **warmup** — run the function until ~50ms elapsed or 10 iterations,
//!     whichever comes first (JIT/cache warmup; not measured).
//!  2. **calibrate** (skipped when `iters` is given) — double the batch
//!     size from 1 until one batch takes >= 10ms via `os.clock()`, so
//!     per-call timer overhead is negligible relative to the batch.
//!  3. **measure** — run timed batches of that size, each one reported as
//!     a `LUABOX_BENCH_RESULT` line (ns/iter for that batch), until >= 10
//!     batches have run or ~1s total has elapsed, whichever comes first.
//!
//! Lines (TAB-separated, same escaping as [`crate::protocol`]):
//!
//! ```text
//! LUABOX_BENCH_RUNTIME <version>
//! LUABOX_BENCH_BEGIN   <name>
//! LUABOX_BENCH_RESULT  <name> <ns_per_iter>
//! LUABOX_BENCH_ERROR   <name> <message>
//! LUABOX_BENCH_DONE    <count>
//! ```
//!
//! `parse` turns a process's stdout into a [`ParsedBenchRun`]; [`stats`]
//! reduces one bench's batch samples to [`BenchStats`] (median, mean,
//! stddev, min, and a >3-sigma outlier count); [`run_suite`] fans a bench
//! suite out across a runtime (one process per file, rayon-parallel,
//! mirroring [`crate::runner::run_suite`]); [`render`] renders a
//! cross-runtime comparison table.

use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;

use crate::runtime::RuntimeSpec;

/// The embedded Lua bench harness, written to a temp file per run and
/// passed the bench files as arguments — see the module docs for the
/// protocol it speaks.
pub const BENCH_HARNESS_SOURCE: &str = include_str!("bench_harness.lua");

// ---------------------------------------------------------------------- --
// Discovery.
// ---------------------------------------------------------------------- --

/// Discover every bench file under `root`, excluding `out_dir` (the build
/// output directory, if any). Deterministically ordered (sorted by entry
/// name at every level, mirroring [`crate::discovery::discover`]).
#[must_use]
pub fn discover(root: &Path, out_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, out_dir, false, &mut found);
    found
}

fn walk(dir: &Path, out_dir: Option<&Path>, in_bench_dir: bool, found: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = read.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            if out_dir == Some(path.as_path()) {
                continue;
            }
            let child_in_bench_dir = in_bench_dir || name == "bench";
            walk(&path, out_dir, child_in_bench_dir, found);
        } else if is_bench_file(&name, in_bench_dir) {
            found.push(path);
        }
    }
}

/// Whether a file (given its name and whether it sits under `bench/`) is a
/// bench file. Pure — unit-tested directly.
#[must_use]
pub fn is_bench_file(name: &str, in_bench_dir: bool) -> bool {
    let is_lua = Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lua"));
    if !is_lua || name.ends_with(".d.lua") {
        return false;
    }
    in_bench_dir || name.ends_with("_bench.lua") || name.ends_with(".bench.lua")
}

// ---------------------------------------------------------------------- --
// Protocol.
// ---------------------------------------------------------------------- --

/// One bench's raw ns/iter samples, one per reported batch, in emission
/// order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BenchSamples {
    pub name: String,
    pub batches_ns: Vec<f64>,
}

/// A bench that raised an error (from `setup` or the timed function).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchError {
    pub name: String,
    pub message: String,
}

/// Everything parsed out of one harness process's stdout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedBenchRun {
    /// The runtime's `_VERSION`, if it announced one.
    pub version: Option<String>,
    /// Per-bench batch samples, in first-seen order.
    pub benches: Vec<BenchSamples>,
    /// Benches that errored, in emission order.
    pub errors: Vec<BenchError>,
    /// The count from the terminating `DONE` line, if the harness ran to
    /// completion. `None` means the process died before finishing.
    pub done: Option<usize>,
}

const RUNTIME: &str = "LUABOX_BENCH_RUNTIME";
const RESULT: &str = "LUABOX_BENCH_RESULT";
const ERROR: &str = "LUABOX_BENCH_ERROR";
const DONE: &str = "LUABOX_BENCH_DONE";
// `LUABOX_BENCH_BEGIN` is a progress-only marker the parser deliberately
// ignores, so it has no constant here (mirrors `crate::protocol`).

/// Parse a harness process's full stdout into a [`ParsedBenchRun`]. Unknown
/// or malformed lines are skipped rather than erroring.
#[must_use]
pub fn parse(stdout: &str) -> ParsedBenchRun {
    let mut run = ParsedBenchRun::default();
    for line in stdout.lines() {
        let mut fields = line.split('\t');
        let Some(tag) = fields.next() else {
            continue;
        };
        match tag {
            RUNTIME => {
                if let Some(v) = fields.next() {
                    run.version = Some(unescape(v));
                }
            }
            RESULT => {
                let (Some(name), Some(ns)) = (fields.next(), fields.next()) else {
                    continue;
                };
                let Ok(ns) = unescape(ns).parse::<f64>() else {
                    continue;
                };
                let name = unescape(name);
                match run.benches.iter_mut().find(|b| b.name == name) {
                    Some(entry) => entry.batches_ns.push(ns),
                    None => run.benches.push(BenchSamples {
                        name,
                        batches_ns: vec![ns],
                    }),
                }
            }
            ERROR => {
                if let Some(name) = fields.next() {
                    let message = fields.next().map(unescape).unwrap_or_default();
                    run.errors.push(BenchError {
                        name: unescape(name),
                        message,
                    });
                }
            }
            DONE => {
                if let Some(count) = fields.next().and_then(|s| s.parse().ok()) {
                    run.done = Some(count);
                }
            }
            _ => {}
        }
    }
    run
}

/// Reverse the harness's field escaping (`\\`, `\n`, `\r`, `\t`) — same
/// scheme as `crate::protocol`, duplicated here since that helper is
/// module-private and `bench` is a standalone sibling module.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') | None => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------- --
// Statistics.
// ---------------------------------------------------------------------- --

/// Summary statistics for one bench's batch samples (ns/iter).
#[derive(Debug, Clone, PartialEq)]
pub struct BenchStats {
    pub name: String,
    /// Number of batches the stats were computed from.
    pub samples: usize,
    pub mean_ns: f64,
    pub median_ns: f64,
    /// Sample standard deviation (Bessel's correction, `n - 1`); `0.0` for
    /// fewer than two samples.
    pub stddev_ns: f64,
    pub min_ns: f64,
    /// Count of batches more than 3 standard deviations from the mean.
    pub outliers: usize,
}

/// Reduce one bench's raw batch samples to [`BenchStats`]. Samples are not
/// mutated in place; a sorted copy is used for the median.
#[must_use]
pub fn stats(samples: &BenchSamples) -> BenchStats {
    let n = samples.batches_ns.len();
    if n == 0 {
        return BenchStats {
            name: samples.name.clone(),
            samples: 0,
            mean_ns: 0.0,
            median_ns: 0.0,
            stddev_ns: 0.0,
            min_ns: 0.0,
            outliers: 0,
        };
    }

    let mut sorted = samples.batches_ns.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    #[allow(
        clippy::cast_precision_loss,
        reason = "n is a small sample count; f64 represents it exactly"
    )]
    let mean = sorted.iter().sum::<f64>() / n as f64;

    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        f64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
    };

    let stddev = if n > 1 {
        #[allow(
            clippy::cast_precision_loss,
            reason = "n is a small sample count; f64 represents it exactly"
        )]
        let variance = sorted.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        variance.sqrt()
    } else {
        0.0
    };

    let outliers = if stddev > 0.0 {
        let threshold = 3.0 * stddev;
        sorted
            .iter()
            .filter(|x| (*x - mean).abs() > threshold)
            .count()
    } else {
        0
    };

    BenchStats {
        name: samples.name.clone(),
        samples: n,
        mean_ns: mean,
        median_ns: median,
        stddev_ns: stddev,
        min_ns: sorted[0],
        outliers,
    }
}

// ---------------------------------------------------------------------- --
// Runner: one process per bench file, rayon-parallel across files.
// ---------------------------------------------------------------------- --

/// Options for one bench-suite run against one runtime.
pub struct SuiteOptions<'a> {
    /// All discovered bench files.
    pub files: &'a [PathBuf],
    /// Project root, used to compute display-relative paths.
    pub root: &'a Path,
}

/// One bench file's outcome under one runtime.
#[derive(Debug, Clone)]
pub struct FileOutcome {
    /// Project-relative, forward-slashed path (stable across platforms).
    pub rel_path: String,
    /// The runtime's announced `_VERSION`, if any.
    pub version: Option<String>,
    /// Per-bench statistics, in the order the harness reported them.
    pub benches: Vec<BenchStats>,
    /// Benches that errored (from `setup` or the timed function).
    pub errors: Vec<BenchError>,
    /// Set if the process couldn't be launched or died before finishing.
    pub error: Option<String>,
}

/// A whole suite's results under one runtime.
#[derive(Debug, Clone)]
pub struct RuntimeReport {
    pub runtime: RuntimeSpec,
    pub files: Vec<FileOutcome>,
}

/// The forward-slashed, root-relative display path for a file.
fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Run the bench suite once against `runtime`. Never returns `Err`:
/// per-file launch/crash problems are captured as [`FileOutcome::error`],
/// since benches never fail the build (SPEC.md §11).
#[must_use]
pub fn run_suite(runtime: &RuntimeSpec, opts: &SuiteOptions) -> RuntimeReport {
    let harness = match write_harness() {
        Ok(h) => h,
        Err(err) => {
            let files = opts
                .files
                .iter()
                .map(|f| FileOutcome {
                    rel_path: rel(f, opts.root),
                    version: None,
                    benches: Vec::new(),
                    errors: Vec::new(),
                    error: Some(format!("cannot stage bench harness: {err}")),
                })
                .collect();
            return RuntimeReport {
                runtime: runtime.clone(),
                files,
            };
        }
    };

    let files = opts
        .files
        .par_iter()
        .map(|file| run_file(runtime, harness.path(), file, opts.root))
        .collect();

    RuntimeReport {
        runtime: runtime.clone(),
        files,
    }
}

/// Spawn one runtime process for one bench file and parse its output.
fn run_file(runtime: &RuntimeSpec, harness: &Path, file: &Path, root: &Path) -> FileOutcome {
    let rel_path = rel(file, root);
    let mut command = Command::new(&runtime.program);
    command.args(&runtime.args).arg(harness).arg(file);

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            return FileOutcome {
                rel_path,
                version: None,
                benches: Vec::new(),
                errors: Vec::new(),
                error: Some(format!(
                    "failed to launch runtime `{}`: {err}",
                    runtime.program
                )),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let run = parse(&stdout);

    if run.done.is_none() && run.benches.is_empty() && run.errors.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let error = if detail.is_empty() {
            format!(
                "runtime `{}` produced no bench output (exit {})",
                runtime.program,
                output.status.code().unwrap_or(-1)
            )
        } else {
            detail.to_string()
        };
        return FileOutcome {
            rel_path,
            version: run.version,
            benches: Vec::new(),
            errors: Vec::new(),
            error: Some(error),
        };
    }

    FileOutcome {
        rel_path,
        version: run.version,
        benches: run.benches.iter().map(stats).collect(),
        errors: run.errors,
        error: None,
    }
}

/// Write the embedded harness to a fresh temp file. Kept alive by the
/// returned handle for the duration of the run.
fn write_harness() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("luabox-bench-harness-")
        .suffix(".lua")
        .tempfile()?;
    file.write_all(BENCH_HARNESS_SOURCE.as_bytes())?;
    file.flush()?;
    Ok(file)
}

// ---------------------------------------------------------------------- --
// Human rendering: a cross-runtime comparison table.
// ---------------------------------------------------------------------- --

/// Render `reports` (one per runtime, from [`run_suite`]) to a human report:
/// a `BENCH | RUNTIME | MEDIAN | ±STDDEV | MEAN | MIN | N | OUTLIERS` table
/// grouped by bench name so every runtime's numbers for a given bench sit
/// together, plus a trailing errors/notes section. Benches never fail the
/// build, so there is no pass/fail verdict here.
#[must_use]
pub fn render(reports: &[RuntimeReport]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();

    // Bench names in first-seen order, scanning runtimes/files in order.
    let mut names: Vec<String> = Vec::new();
    for report in reports {
        for file in &report.files {
            for b in &file.benches {
                if !names.contains(&b.name) {
                    names.push(b.name.clone());
                }
            }
        }
    }

    if !names.is_empty() {
        let _ = writeln!(
            out,
            "{:<28} {:<10} {:>12} {:>12} {:>12} {:>12} {:>5} {:>9}",
            "BENCH", "RUNTIME", "MEDIAN", "±STDDEV", "MEAN", "MIN", "N", "OUTLIERS"
        );
        for name in &names {
            for report in reports {
                for file in &report.files {
                    for b in &file.benches {
                        if &b.name != name {
                            continue;
                        }
                        let _ = writeln!(
                            out,
                            "{:<28} {:<10} {:>12} {:>12} {:>12} {:>12} {:>5} {:>9}",
                            b.name,
                            report.runtime.label,
                            format_ns(b.median_ns),
                            format!("±{}", format_ns(b.stddev_ns)),
                            format_ns(b.mean_ns),
                            format_ns(b.min_ns),
                            b.samples,
                            b.outliers,
                        );
                    }
                }
            }
        }
    }

    let mut errors: Vec<(&str, &str, &str)> = Vec::new();
    for report in reports {
        for file in &report.files {
            if let Some(err) = &file.error {
                errors.push((
                    report.runtime.label.as_str(),
                    file.rel_path.as_str(),
                    err.as_str(),
                ));
            }
            for e in &file.errors {
                errors.push((
                    report.runtime.label.as_str(),
                    e.name.as_str(),
                    e.message.as_str(),
                ));
            }
        }
    }
    if !errors.is_empty() {
        out.push('\n');
        let _ = writeln!(out, "errors:");
        for (runtime, what, message) in &errors {
            let _ = writeln!(out, "  [{runtime}] {what}: {message}");
        }
    }

    if names.is_empty() && errors.is_empty() {
        let _ = writeln!(out, "no benches ran");
    }

    out
}

/// Scale a nanosecond duration to a human-friendly unit (ns/µs/ms/s), one
/// decimal place, e.g. `"123.4ns"`, `"1.23µs"`, `"4.56ms"`.
fn format_ns(ns: f64) -> String {
    if ns < 1_000.0 {
        format!("{ns:.1}ns")
    } else if ns < 1_000_000.0 {
        format!("{:.2}\u{b5}s", ns / 1_000.0)
    } else if ns < 1_000_000_000.0 {
        format!("{:.2}ms", ns / 1_000_000.0)
    } else {
        format!("{:.3}s", ns / 1_000_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BenchSamples, FileOutcome, RuntimeReport, discover, is_bench_file, parse, rel, render,
        stats,
    };
    use crate::runtime::RuntimeSpec;
    use std::fs;
    use std::path::Path;

    // -- discovery ---------------------------------------------------- --

    #[test]
    fn naming_rules() {
        assert!(is_bench_file("math_bench.lua", false));
        assert!(is_bench_file("math.bench.lua", false));
        assert!(!is_bench_file("math.lua", false));
        assert!(is_bench_file("math.lua", true));
        assert!(!is_bench_file("api.d.lua", true));
        assert!(!is_bench_file("README.md", true));
    }

    #[test]
    fn discovers_by_name_and_bench_dir_recursively() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path();
        fs::create_dir_all(p.join("src")).unwrap();
        fs::create_dir_all(p.join("bench/sub")).unwrap();
        fs::create_dir_all(p.join("dist")).unwrap();
        fs::create_dir_all(p.join(".git")).unwrap();

        fs::write(p.join("src/math_bench.lua"), "").unwrap();
        fs::write(p.join("src/math.bench.lua"), "").unwrap();
        fs::write(p.join("src/math.lua"), "").unwrap(); // not a bench
        fs::write(p.join("src/api.d.lua"), "").unwrap(); // def, not a bench
        fs::write(p.join("bench/basic.lua"), "").unwrap(); // under bench/
        fs::write(p.join("bench/sub/deep.lua"), "").unwrap(); // recursive
        fs::write(p.join("dist/built_bench.lua"), "").unwrap(); // in out dir
        fs::write(p.join(".git/hook_bench.lua"), "").unwrap(); // dot dir

        let out = p.join("dist");
        let mut got: Vec<String> = discover(p, Some(&out))
            .iter()
            .map(|f| {
                f.strip_prefix(p)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        got.sort();

        assert_eq!(
            got,
            vec![
                "bench/basic.lua",
                "bench/sub/deep.lua",
                "src/math.bench.lua",
                "src/math_bench.lua",
            ]
        );
    }

    // -- protocol ------------------------------------------------------ --

    #[test]
    fn parses_a_full_run_with_multiple_batches() {
        let out = "LUABOX_BENCH_RUNTIME\tLua 5.4\n\
                    LUABOX_BENCH_BEGIN\tfib(20)\n\
                    LUABOX_BENCH_RESULT\tfib(20)\t120.5\n\
                    LUABOX_BENCH_RESULT\tfib(20)\t118.2\n\
                    LUABOX_BENCH_DONE\t1\n";
        let run = parse(out);
        assert_eq!(run.version.as_deref(), Some("Lua 5.4"));
        assert_eq!(run.done, Some(1));
        assert_eq!(run.benches.len(), 1);
        assert_eq!(run.benches[0].name, "fib(20)");
        assert_eq!(run.benches[0].batches_ns, vec![120.5, 118.2]);
        assert!(run.errors.is_empty());
    }

    #[test]
    fn groups_batches_by_bench_name_across_interleaved_lines() {
        let out = "LUABOX_BENCH_RESULT\ta\t1.0\n\
                    LUABOX_BENCH_RESULT\tb\t2.0\n\
                    LUABOX_BENCH_RESULT\ta\t3.0\n\
                    LUABOX_BENCH_DONE\t2\n";
        let run = parse(out);
        assert_eq!(run.benches.len(), 2);
        assert_eq!(run.benches[0].name, "a");
        assert_eq!(run.benches[0].batches_ns, vec![1.0, 3.0]);
        assert_eq!(run.benches[1].name, "b");
        assert_eq!(run.benches[1].batches_ns, vec![2.0]);
    }

    #[test]
    fn parses_errors_with_escaped_message() {
        let out = "LUABOX_BENCH_ERROR\tbroken\tsetup failed: line one\\nline two\n\
                    LUABOX_BENCH_DONE\t0\n";
        let run = parse(out);
        assert_eq!(run.errors.len(), 1);
        assert_eq!(run.errors[0].name, "broken");
        assert_eq!(run.errors[0].message, "setup failed: line one\nline two");
    }

    #[test]
    fn ignores_unknown_lines_and_malformed_results() {
        let out = "hello from a print\n\
                    LUABOX_BENCH_RESULT\tnot-enough-fields\n\
                    LUABOX_BENCH_RESULT\tok\tnot-a-number\n\
                    LUABOX_BENCH_RESULT\tok\t5.0\n\
                    LUABOX_BENCH_DONE\t1\n";
        let run = parse(out);
        assert_eq!(run.benches.len(), 1);
        assert_eq!(run.benches[0].batches_ns, vec![5.0]);
    }

    #[test]
    fn missing_done_leaves_none() {
        let out = "LUABOX_BENCH_RESULT\ta\t1.0\n";
        let run = parse(out);
        assert_eq!(run.done, None);
    }

    // -- statistics ------------------------------------------------------ --

    #[test]
    fn stats_computes_mean_median_stddev_min() {
        let samples = BenchSamples {
            name: "x".to_string(),
            batches_ns: vec![1.0, 2.0, 3.0, 4.0, 5.0],
        };
        let s = stats(&samples);
        assert_eq!(s.samples, 5);
        assert!((s.mean_ns - 3.0).abs() < 1e-9);
        assert!((s.median_ns - 3.0).abs() < 1e-9);
        assert!((s.stddev_ns - 2.5_f64.sqrt()).abs() < 1e-9);
        assert!((s.min_ns - 1.0).abs() < 1e-9);
        assert_eq!(s.outliers, 0);
    }

    #[test]
    fn stats_median_of_even_count_averages_the_middle_pair() {
        let samples = BenchSamples {
            name: "x".to_string(),
            batches_ns: vec![10.0, 20.0, 30.0, 40.0],
        };
        let s = stats(&samples);
        assert!((s.median_ns - 25.0).abs() < 1e-9);
    }

    #[test]
    fn stats_flags_a_genuine_outlier_beyond_three_sigma() {
        // 29 samples at 10.0, one at 100.0: mean = 13.0, sample stddev =
        // sqrt(270) ~= 16.43, so the 100.0 sample sits at |100-13| = 87,
        // comfortably past 3 sigma (~49.3) — the base population is large
        // enough that a single extreme value doesn't inflate the stddev
        // past the outlier itself (unlike a small n).
        let mut batches = vec![10.0; 29];
        batches.push(100.0);
        let samples = BenchSamples {
            name: "x".to_string(),
            batches_ns: batches,
        };
        let s = stats(&samples);
        assert!((s.mean_ns - 13.0).abs() < 1e-9);
        assert!((s.stddev_ns - 270.0_f64.sqrt()).abs() < 1e-9);
        assert!((s.median_ns - 10.0).abs() < 1e-9);
        assert_eq!(s.outliers, 1);
    }

    #[test]
    fn stats_single_sample_has_zero_stddev_and_no_outliers() {
        let samples = BenchSamples {
            name: "x".to_string(),
            batches_ns: vec![42.0],
        };
        let s = stats(&samples);
        assert_eq!(s.samples, 1);
        assert!((s.mean_ns - 42.0).abs() < 1e-9);
        assert!((s.median_ns - 42.0).abs() < 1e-9);
        assert!(s.stddev_ns.abs() < 1e-9);
        assert_eq!(s.outliers, 0);
    }

    // -- rendering --------------------------------------------------------- --

    fn spec(label: &str) -> RuntimeSpec {
        RuntimeSpec {
            label: label.to_string(),
            program: "lua".to_string(),
            args: Vec::new(),
        }
    }

    #[test]
    fn render_groups_one_bench_across_two_runtimes() {
        let make = |runtime: &str, ns: f64| RuntimeReport {
            runtime: spec(runtime),
            files: vec![FileOutcome {
                rel_path: "bench/fib_bench.lua".to_string(),
                version: Some(format!("Lua {runtime}")),
                benches: vec![stats(&BenchSamples {
                    name: "fib(20)".to_string(),
                    batches_ns: vec![ns, ns, ns],
                })],
                errors: Vec::new(),
                error: None,
            }],
        };
        let reports = vec![make("5.4", 120.0), make("luajit", 12.0)];
        let text = render(&reports);
        assert!(text.contains("BENCH"));
        assert!(text.contains("RUNTIME"));
        assert!(text.contains("MEDIAN"));
        assert!(text.contains("\u{b1}STDDEV"));
        assert!(text.contains("fib(20)"));
        assert!(text.contains("5.4"));
        assert!(text.contains("luajit"));
        assert!(text.contains("120.0ns"));
        assert!(text.contains("12.0ns"));
    }

    #[test]
    fn render_reports_bench_errors() {
        let reports = vec![RuntimeReport {
            runtime: spec("5.4"),
            files: vec![FileOutcome {
                rel_path: "bench/broken_bench.lua".to_string(),
                version: None,
                benches: Vec::new(),
                errors: vec![super::BenchError {
                    name: "broken".to_string(),
                    message: "boom".to_string(),
                }],
                error: None,
            }],
        }];
        let text = render(&reports);
        assert!(text.contains("errors:"));
        assert!(text.contains("broken"));
        assert!(text.contains("boom"));
    }

    #[test]
    fn render_notes_when_nothing_ran() {
        let text = render(&[]);
        assert!(text.contains("no benches ran"));
    }

    #[test]
    fn rel_strips_root_and_normalizes_separators() {
        assert_eq!(
            rel(Path::new("/proj/bench/x_bench.lua"), Path::new("/proj")),
            "bench/x_bench.lua"
        );
    }
}
