//! The runner: turn discovered files + a runtime into results.
//!
//! One OS process per test **file** (SPEC.md §11), fanned out with rayon.
//! Each process runs `<runtime> <harness.lua> <test_file>`; the harness
//! (written once to a temp file) `dofile`s the test file and speaks the
//! line protocol parsed by [`crate::protocol`].
//!
//! ## Pattern filtering
//!
//! The optional `pattern` is matched, as a plain substring, first against
//! each test file's project-relative path (forward-slashed). If it matches
//! any file, only those files run. If it matches **no** file path, it is
//! instead treated as a test-*name* filter applied across every file (a
//! case runs iff its name contains the pattern) — handed to the harness via
//! `LUABOX_TEST_FILTER`. This "path first, else name" rule keeps the common
//! `luabox test foo/bar` (a path) and `luabox test "adds numbers"` (a name)
//! both doing the obvious thing.

use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;

use crate::protocol::{self, CaseResult};
use crate::runtime::RuntimeSpec;

/// Options for one suite run against one runtime.
pub struct SuiteOptions<'a> {
    /// All discovered test files.
    pub files: &'a [PathBuf],
    /// The `[pattern]` CLI argument, if any.
    pub pattern: Option<&'a str>,
    /// Project root, used to compute display-relative paths for matching
    /// and reporting.
    pub root: &'a Path,
}

/// One test file's outcome under one runtime.
#[derive(Debug, Clone)]
pub struct FileOutcome {
    /// Project-relative, forward-slashed path (stable across platforms).
    pub rel_path: String,
    /// The runtime's announced `_VERSION`, if any.
    pub version: Option<String>,
    /// Per-case results parsed from the harness.
    pub cases: Vec<CaseResult>,
    /// Set if the process couldn't be launched or died before finishing —
    /// counts as a failure.
    pub error: Option<String>,
}

/// A whole suite's results under one runtime.
#[derive(Debug, Clone)]
pub struct RuntimeReport {
    pub runtime: RuntimeSpec,
    pub files: Vec<FileOutcome>,
}

impl RuntimeReport {
    #[must_use]
    pub fn passed(&self) -> usize {
        self.files
            .iter()
            .flat_map(|f| &f.cases)
            .filter(|c| matches!(c.outcome, protocol::Outcome::Pass))
            .count()
    }

    /// Failed cases plus files that errored (each counts as one failure so
    /// a crash can never be mistaken for success).
    #[must_use]
    pub fn failed(&self) -> usize {
        let case_failures = self
            .files
            .iter()
            .flat_map(|f| &f.cases)
            .filter(|c| matches!(c.outcome, protocol::Outcome::Fail(_)))
            .count();
        let file_errors = self.files.iter().filter(|f| f.error.is_some()).count();
        case_failures + file_errors
    }
}

/// A resolved run plan: which files to spawn and an optional name filter to
/// pass to every one of them.
struct Plan {
    selected: Vec<PathBuf>,
    name_filter: Option<String>,
}

/// The forward-slashed, root-relative display path for a file.
fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn plan(opts: &SuiteOptions) -> Plan {
    let Some(pattern) = opts.pattern.filter(|p| !p.is_empty()) else {
        return Plan {
            selected: opts.files.to_vec(),
            name_filter: None,
        };
    };

    let path_matches: Vec<PathBuf> = opts
        .files
        .iter()
        .filter(|f| rel(f, opts.root).contains(pattern))
        .cloned()
        .collect();

    if path_matches.is_empty() {
        // No path matched: treat the pattern as a test-name filter across
        // every file.
        Plan {
            selected: opts.files.to_vec(),
            name_filter: Some(pattern.to_string()),
        }
    } else {
        Plan {
            selected: path_matches,
            name_filter: None,
        }
    }
}

/// Run the suite once against `runtime`. Never returns `Err`: per-file
/// launch/crash problems are captured as [`FileOutcome::error`] so the
/// report is always complete.
#[must_use]
pub fn run_suite(runtime: &RuntimeSpec, opts: &SuiteOptions) -> RuntimeReport {
    let plan = plan(opts);
    let harness = match write_harness() {
        Ok(h) => h,
        Err(err) => {
            // Can't stage the harness: report every selected file as errored.
            let files = plan
                .selected
                .iter()
                .map(|f| FileOutcome {
                    rel_path: rel(f, opts.root),
                    version: None,
                    cases: Vec::new(),
                    error: Some(format!("cannot stage test harness: {err}")),
                })
                .collect();
            return RuntimeReport {
                runtime: runtime.clone(),
                files,
            };
        }
    };

    let files = plan
        .selected
        .par_iter()
        .map(|file| {
            run_file(
                runtime,
                harness.path(),
                file,
                plan.name_filter.as_deref(),
                opts.root,
            )
        })
        .collect();

    RuntimeReport {
        runtime: runtime.clone(),
        files,
    }
}

/// Spawn one runtime process for one test file and parse its output.
fn run_file(
    runtime: &RuntimeSpec,
    harness: &Path,
    file: &Path,
    name_filter: Option<&str>,
    root: &Path,
) -> FileOutcome {
    let rel_path = rel(file, root);
    let mut command = Command::new(&runtime.program);
    command.args(&runtime.args).arg(harness).arg(file);
    if let Some(filter) = name_filter {
        command.env("LUABOX_TEST_FILTER", filter);
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(err) => {
            return FileOutcome {
                rel_path,
                version: None,
                cases: Vec::new(),
                error: Some(format!(
                    "failed to launch runtime `{}`: {err}",
                    runtime.program
                )),
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let run = protocol::parse(&stdout);

    // No terminating DONE and no cases means the harness never really ran
    // (bad interpreter, panic, etc.) — surface stderr as the error.
    if run.done.is_none() && run.cases.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let error = if detail.is_empty() {
            format!(
                "runtime `{}` produced no test output (exit {})",
                runtime.program,
                output.status.code().unwrap_or(-1)
            )
        } else {
            detail.to_string()
        };
        return FileOutcome {
            rel_path,
            version: run.version,
            cases: Vec::new(),
            error: Some(error),
        };
    }

    FileOutcome {
        rel_path,
        version: run.version,
        cases: run.cases,
        error: None,
    }
}

/// Write the embedded harness to a fresh temp file. Kept alive by the
/// returned handle for the duration of the run.
fn write_harness() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::Builder::new()
        .prefix("luabox-harness-")
        .suffix(".lua")
        .tempfile()?;
    file.write_all(crate::HARNESS_SOURCE.as_bytes())?;
    file.flush()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::{SuiteOptions, plan};
    use std::path::{Path, PathBuf};

    fn files() -> Vec<PathBuf> {
        vec![
            PathBuf::from("/proj/src/math_test.lua"),
            PathBuf::from("/proj/tests/io_test.lua"),
        ]
    }

    #[test]
    fn no_pattern_selects_everything() {
        let files = files();
        let opts = SuiteOptions {
            files: &files,
            pattern: None,
            root: Path::new("/proj"),
        };
        let p = plan(&opts);
        assert_eq!(p.selected.len(), 2);
        assert!(p.name_filter.is_none());
    }

    #[test]
    fn path_pattern_narrows_to_matching_files() {
        let files = files();
        let opts = SuiteOptions {
            files: &files,
            pattern: Some("tests/"),
            root: Path::new("/proj"),
        };
        let p = plan(&opts);
        assert_eq!(p.selected, vec![PathBuf::from("/proj/tests/io_test.lua")]);
        assert!(p.name_filter.is_none());
    }

    #[test]
    fn name_pattern_when_no_path_matches_filters_all_files() {
        let files = files();
        let opts = SuiteOptions {
            files: &files,
            pattern: Some("adds numbers"),
            root: Path::new("/proj"),
        };
        let p = plan(&opts);
        assert_eq!(p.selected.len(), 2);
        assert_eq!(p.name_filter.as_deref(), Some("adds numbers"));
    }
}
