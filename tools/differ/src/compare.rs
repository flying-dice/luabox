//! Comparing a source run against its lowered run.
//!
//! # What is compared
//!
//! For a `(source-on-from-runtime, lowered-on-target-runtime)` pair we assert
//! three things, in this order:
//!
//! 1. **stdout — exact.** After normalizing line endings only (`\r\n` → `\n`,
//!    so a Windows source runtime and a Linux target runtime agree), the two
//!    stdout streams must be byte-identical. This is the primary signal: every
//!    corpus program prints deterministic lines, and *observable behaviour is
//!    the thing lowering must preserve*.
//! 2. **exit code — exact.** Success vs failure must agree (`0` vs non-zero),
//!    and the specific code must match. A timeout is treated as its own
//!    distinct outcome, so an infinite loop introduced by a bad lowering is a
//!    mismatch rather than a hang.
//! 3. **error class — normalized.** Only consulted when at least one side
//!    failed. Raw interpreter error text carries three kinds of noise that
//!    differ legitimately between two runs of *the same* logical error, so we
//!    strip them before comparing:
//!      - the **interpreter/program prefix** (`lua5.4:` vs `lua5.1:` — the two
//!        runtimes are different binaries), removed by stripping the known
//!        invoking program path and its basename;
//!      - **`chunk:line:` positions** — lowering injects a prelude and rewrites
//!        statements, so the *same* error surfaces at a *different* line, and
//!        the source file and the lowered temp file have different paths;
//!        every `…:<digits>:` marker collapses to `FILE:LINE:`;
//!      - **addresses** (`table: 0x55f…`) collapse to `0xADDR`;
//!      - the **`stack traceback:` tail** is dropped entirely (frame counts and
//!        positions differ structurally after a rewrite).
//!
//!    What survives is the human-readable error *class* (e.g. `attempt to
//!    perform arithmetic on a nil value`). This is deliberately coarse: error
//!    *wording* still varies across Lua versions (5.2+ appends the offending
//!    variable name in parentheses), so the error-class axis is a **safety
//!    net** for unexpected crashes. Corpus programs that mean to exercise an
//!    error *path* catch it with `pcall` and print a deterministic line, moving
//!    that assertion onto the exact-stdout axis where it is version-robust.

use std::sync::LazyLock;

use regex::Regex;

use crate::exec::ExecResult;

/// Which comparison axis diverged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Stdout,
    ExitCode,
    ErrorClass,
}

impl Axis {
    pub fn label(self) -> &'static str {
        match self {
            Axis::Stdout => "stdout",
            Axis::ExitCode => "exit-code",
            Axis::ErrorClass => "error-class",
        }
    }
}

/// The outcome of comparing one pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Match,
    Mismatch(Vec<Axis>),
}

/// Normalize stdout: line endings only. Everything else about stdout is
/// load-bearing and compared verbatim.
pub fn normalize_stdout(s: &str) -> String {
    s.replace("\r\n", "\n")
}

static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0[xX][0-9a-fA-F]+").unwrap());
static POS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S*:\d+:").unwrap());

/// Normalize an interpreter error stream into a comparable *class* string.
/// `program` is the path we invoked, whose name Lua echoes as the error
/// prefix; stripping it makes a `lua5.4` error comparable with a `lua5.1` one.
pub fn normalize_error(stderr: &str, program: &str) -> String {
    let mut s = stderr.replace("\r\n", "\n");

    // Drop the `stack traceback:` tail: frame positions/counts differ after a
    // rewrite and would otherwise dominate the class.
    if let Some(idx) = s.find("stack traceback:") {
        s.truncate(idx);
    }

    // Strip the invoking-program prefix Lua prints on each error line.
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(program);
    for prefix in [format!("{program}: "), format!("{basename}: ")] {
        s = s.replace(&prefix, "");
    }

    // Collapse addresses, then `chunk:line:` positions.
    let s = HEX.replace_all(&s, "0xADDR");
    let s = POS.replace_all(&s, "FILE:LINE:");
    s.trim().to_string()
}

/// Compare a source run against a lowered run. `orig_program` / `low_program`
/// are the interpreters they ran under (needed to strip each side's own error
/// prefix).
pub fn compare(
    orig: &ExecResult,
    low: &ExecResult,
    orig_program: &str,
    low_program: &str,
) -> Verdict {
    let mut axes = Vec::new();

    if normalize_stdout(&orig.stdout) != normalize_stdout(&low.stdout) {
        axes.push(Axis::Stdout);
    }

    // Exit outcome: a timeout is its own bucket, distinct from any real code.
    let orig_exit = (orig.timed_out, orig.code);
    let low_exit = (low.timed_out, low.code);
    if orig_exit != low_exit {
        axes.push(Axis::ExitCode);
    }

    // Error class only matters when something actually went wrong on either
    // side. (If both succeeded, stderr may carry benign warnings we ignore.)
    if orig.failed() || low.failed() {
        let orig_class = normalize_error(&orig.stderr, orig_program);
        let low_class = normalize_error(&low.stderr, low_program);
        if orig_class != low_class {
            axes.push(Axis::ErrorClass);
        }
    }

    if axes.is_empty() {
        Verdict::Match
    } else {
        Verdict::Mismatch(axes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Verdict {
        fn is_match(&self) -> bool {
            matches!(self, Verdict::Match)
        }
    }

    fn ok(stdout: &str) -> ExecResult {
        ExecResult {
            stdout: stdout.to_string(),
            stderr: String::new(),
            code: Some(0),
            timed_out: false,
        }
    }

    #[test]
    fn identical_success_matches() {
        let a = ok("1\n2\n3\n");
        let b = ok("1\n2\n3\n");
        assert!(compare(&a, &b, "lua5.4", "lua5.1").is_match());
    }

    #[test]
    fn crlf_is_normalized_before_comparison() {
        let a = ok("hello\r\nworld\r\n");
        let b = ok("hello\nworld\n");
        assert!(compare(&a, &b, "lua", "lua").is_match());
    }

    #[test]
    fn differing_stdout_is_caught() {
        let a = ok("1\n2\n");
        let b = ok("1\n3\n");
        let v = compare(&a, &b, "lua", "lua");
        assert_eq!(v, Verdict::Mismatch(vec![Axis::Stdout]));
    }

    #[test]
    fn differing_exit_code_is_caught() {
        let a = ok("");
        let b = ExecResult {
            code: Some(1),
            stderr: "lua5.1: FILE:LINE: boom".into(),
            ..ok("")
        };
        let v = compare(&a, &b, "lua", "lua");
        // Exit code differs; error class also differs (a had no error text).
        match v {
            Verdict::Mismatch(axes) => assert!(axes.contains(&Axis::ExitCode)),
            Verdict::Match => panic!("expected mismatch"),
        }
    }

    #[test]
    fn timeout_is_distinct_from_clean_exit() {
        let a = ok("");
        let b = ExecResult {
            timed_out: true,
            code: None,
            ..ok("")
        };
        let v = compare(&a, &b, "lua", "lua");
        match v {
            Verdict::Mismatch(axes) => assert!(axes.contains(&Axis::ExitCode)),
            Verdict::Match => panic!("expected mismatch on timeout"),
        }
    }

    #[test]
    fn same_error_class_across_runtimes_matches() {
        // Same logical error, different interpreter prefix, different line
        // number (lowering shifted it), different temp path — must be equal.
        let a = ExecResult {
            code: Some(1),
            stderr: "lua5.4: /home/x/corpus/foo.lua:3: attempt to call a nil value\n\
                     stack traceback:\n\t[C]: in ?\n"
                .into(),
            ..ok("")
        };
        let b = ExecResult {
            code: Some(1),
            stderr: "lua5.1: /tmp/differ-abc/foo.lua:31: attempt to call a nil value\n\
                     stack traceback:\n\t[C]: in ?\n"
                .into(),
            ..ok("")
        };
        // Position markers are collapsed but retained; both sides normalize to
        // the same string, which is what makes them compare equal.
        assert_eq!(
            normalize_error(&a.stderr, "lua5.4"),
            "FILE:LINE: attempt to call a nil value"
        );
        assert_eq!(
            normalize_error(&a.stderr, "lua5.4"),
            normalize_error(&b.stderr, "lua5.1")
        );
        assert!(compare(&a, &b, "lua5.4", "lua5.1").is_match());
    }

    #[test]
    fn genuinely_different_errors_are_caught() {
        let a = ExecResult {
            code: Some(1),
            stderr: "lua5.4: foo.lua:3: attempt to call a nil value\n".into(),
            ..ok("")
        };
        let b = ExecResult {
            code: Some(1),
            stderr: "lua5.1: foo.lua:9: attempt to index a nil value\n".into(),
            ..ok("")
        };
        let v = compare(&a, &b, "lua5.4", "lua5.1");
        match v {
            Verdict::Mismatch(axes) => assert!(axes.contains(&Axis::ErrorClass)),
            Verdict::Match => panic!("expected error-class mismatch"),
        }
    }

    #[test]
    fn addresses_are_normalized() {
        let n = normalize_error("lua: cannot use table: 0x55f0aabbcc\n", "lua");
        assert_eq!(n, "cannot use table: 0xADDR");
    }
}
