//! Human-readable rendering of runtime reports (SPEC.md §11).
//!
//! One block per runtime in `--matrix` mode (each headed by the runtime
//! label and its announced version), a single flat listing otherwise. A
//! trailing summary line states the overall verdict. JUnit/JSON reporters
//! are a documented follow-up (SPEC.md §11) — human output only for now.

use std::fmt::Write as _;

use crate::protocol::Outcome;
use crate::runner::RuntimeReport;

/// Aggregate pass/fail counts across every runtime in a run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
}

impl Summary {
    #[must_use]
    pub fn is_ok(self) -> bool {
        self.failed == 0
    }
}

/// How [`render`] lays out multiple runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Each runtime gets its own headed block and per-runtime subtotal.
    Matrix,
    /// A single flat listing across all runtimes.
    Flat,
}

/// Render `reports` to a human report plus the aggregate [`Summary`]. In
/// [`Layout::Matrix`] each runtime gets its own headed block and per-runtime
/// subtotal; [`Layout::Flat`] produces a single flat listing.
#[must_use]
pub fn render(reports: &[RuntimeReport], layout: Layout) -> (String, Summary) {
    let matrix = matches!(layout, Layout::Matrix);
    let mut out = String::new();
    let mut summary = Summary::default();

    for report in reports {
        if matrix {
            let version = report
                .files
                .iter()
                .find_map(|f| f.version.clone())
                .unwrap_or_else(|| "not detected".to_string());
            let _ = writeln!(out, "== runtime {} ({version}) ==", report.runtime.label);
        }

        render_report_body(&mut out, report);

        let passed = report.passed();
        let failed = report.failed();
        summary.passed += passed;
        summary.failed += failed;

        if matrix {
            let _ = writeln!(
                out,
                "  {}: {passed} passed; {failed} failed",
                report.runtime.label
            );
            out.push('\n');
        }
    }

    let verdict = if summary.is_ok() { "ok" } else { "FAILED" };
    let _ = writeln!(
        out,
        "test result: {verdict}. {} passed; {} failed",
        summary.passed, summary.failed
    );

    (out, summary)
}

/// The per-test listing for one runtime (no header, no subtotal).
fn render_report_body(out: &mut String, report: &RuntimeReport) {
    for file in &report.files {
        if let Some(error) = &file.error {
            let _ = writeln!(out, "  ERROR {}", file.rel_path);
            for line in error.lines() {
                let _ = writeln!(out, "        {line}");
            }
        }
        for case in &file.cases {
            match &case.outcome {
                Outcome::Pass => {
                    let _ = writeln!(out, "  PASS {}", case.name);
                }
                Outcome::Fail(message) => {
                    let _ = writeln!(out, "  FAIL {}", case.name);
                    for line in message.lines() {
                        let _ = writeln!(out, "        {line}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, render};
    use crate::protocol::{CaseResult, Outcome};
    use crate::runner::{FileOutcome, RuntimeReport};
    use crate::runtime::RuntimeSpec;

    fn spec(label: &str) -> RuntimeSpec {
        RuntimeSpec {
            label: label.to_string(),
            program: "lua".to_string(),
            args: Vec::new(),
        }
    }

    fn report() -> RuntimeReport {
        RuntimeReport {
            runtime: spec("5.4"),
            files: vec![FileOutcome {
                rel_path: "tests/math_test.lua".to_string(),
                version: Some("Lua 5.4".to_string()),
                cases: vec![
                    CaseResult {
                        name: "adds numbers".to_string(),
                        outcome: Outcome::Pass,
                    },
                    CaseResult {
                        name: "divides numbers".to_string(),
                        outcome: Outcome::Fail("expected 2 but got 3".to_string()),
                    },
                ],
                error: None,
            }],
        }
    }

    #[test]
    fn flat_report_lists_cases_and_summary() {
        let (text, summary) = render(&[report()], Layout::Flat);
        assert!(text.contains("PASS adds numbers"));
        assert!(text.contains("FAIL divides numbers"));
        assert!(text.contains("expected 2 but got 3"));
        assert!(text.contains("test result: FAILED. 1 passed; 1 failed"));
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.failed, 1);
        assert!(!summary.is_ok());
    }

    #[test]
    fn matrix_report_groups_per_runtime() {
        let (text, summary) = render(&[report()], Layout::Matrix);
        assert!(text.contains("== runtime 5.4 (Lua 5.4) =="));
        assert!(text.contains("5.4: 1 passed; 1 failed"));
        assert_eq!(summary.failed, 1);
    }

    #[test]
    fn file_error_counts_as_failure() {
        let report = RuntimeReport {
            runtime: spec("5.4"),
            files: vec![FileOutcome {
                rel_path: "tests/broken_test.lua".to_string(),
                version: None,
                cases: Vec::new(),
                error: Some("syntax error near '='".to_string()),
            }],
        };
        let (text, summary) = render(&[report], Layout::Flat);
        assert!(text.contains("ERROR tests/broken_test.lua"));
        assert!(text.contains("syntax error near '='"));
        assert_eq!(summary.failed, 1);
    }
}
