//! The line-oriented machine protocol spoken by the embedded Lua harness
//! (`harness.lua`) and parsed here.
//!
//! Every protocol line is `TAG\tfield\tfield…`, where each field escapes
//! `\`, newline, carriage return and tab so a field never spans lines or
//! contains the delimiter. Lines that don't start with a known tag (e.g.
//! arbitrary `print` output from a test body) are ignored, so user output
//! can't corrupt the result stream.

/// One test case's outcome as reported by the harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseResult {
    pub name: String,
    pub outcome: Outcome,
}

/// Pass, or fail with the harness-reported message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Pass,
    Fail(String),
}

/// Everything parsed out of one harness process's stdout.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedRun {
    /// The runtime's `_VERSION` (e.g. `"Lua 5.4"`), if it announced one.
    pub version: Option<String>,
    /// Per-case results, in the order the harness emitted them.
    pub cases: Vec<CaseResult>,
    /// The `(passed, failed)` counts from the terminating `DONE` line, if
    /// the harness ran to completion. `None` means the process died before
    /// finishing (crash / uncaught load error).
    pub done: Option<(usize, usize)>,
}

// `LUABOX_TEST_BEGIN` is a progress-only marker the parser deliberately
// ignores (the verdict rides on PASS/FAIL), so it has no constant here.
const RUNTIME: &str = "LUABOX_TEST_RUNTIME";
const PASS: &str = "LUABOX_TEST_PASS";
const FAIL: &str = "LUABOX_TEST_FAIL";
const DONE: &str = "LUABOX_TEST_DONE";

/// Parse a harness process's full stdout into a [`ParsedRun`]. Unknown or
/// malformed lines are skipped rather than erroring — robustness over
/// strictness, since test bodies share the stdout stream.
#[must_use]
pub fn parse(stdout: &str) -> ParsedRun {
    let mut run = ParsedRun::default();
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
            PASS => {
                if let Some(name) = fields.next() {
                    run.cases.push(CaseResult {
                        name: unescape(name),
                        outcome: Outcome::Pass,
                    });
                }
            }
            FAIL => {
                if let Some(name) = fields.next() {
                    let message = fields.next().map(unescape).unwrap_or_default();
                    run.cases.push(CaseResult {
                        name: unescape(name),
                        outcome: Outcome::Fail(message),
                    });
                }
            }
            DONE => {
                let passed = fields.next().and_then(|s| s.parse().ok());
                let failed = fields.next().and_then(|s| s.parse().ok());
                if let (Some(p), Some(f)) = (passed, failed) {
                    run.done = Some((p, f));
                }
            }
            // BEGIN lines (progress only) and any unknown line — including
            // arbitrary `print` output from a test body — are ignored.
            _ => {}
        }
    }
    run
}

/// Reverse the harness's field escaping (`\\`, `\n`, `\r`, `\t`).
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
            // Escaped backslash, or a trailing lone backslash: one backslash.
            Some('\\') | None => out.push('\\'),
            // Unknown escape: keep the backslash and the char verbatim.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Outcome, parse};

    #[test]
    fn parses_a_full_passing_run() {
        let out = "LUABOX_TEST_RUNTIME\tLua 5.4\n\
                   LUABOX_TEST_BEGIN\tadds numbers\n\
                   LUABOX_TEST_PASS\tadds numbers\n\
                   LUABOX_TEST_DONE\t1\t0\n";
        let run = parse(out);
        assert_eq!(run.version.as_deref(), Some("Lua 5.4"));
        assert_eq!(run.done, Some((1, 0)));
        assert_eq!(run.cases.len(), 1);
        assert_eq!(run.cases[0].name, "adds numbers");
        assert_eq!(run.cases[0].outcome, Outcome::Pass);
    }

    #[test]
    fn parses_failure_message_with_escaped_newline() {
        let out = "LUABOX_TEST_FAIL\tit breaks\tline one\\nline two\n\
                   LUABOX_TEST_DONE\t0\t1\n";
        let run = parse(out);
        assert_eq!(run.done, Some((0, 1)));
        assert_eq!(
            run.cases[0].outcome,
            Outcome::Fail("line one\nline two".to_string())
        );
    }

    #[test]
    fn unescapes_backslash_paths_and_tabs() {
        let out = "LUABOX_TEST_FAIL\tt\\tname\tC:\\\\proj\\\\a.lua:5: boom\n";
        let run = parse(out);
        assert_eq!(run.cases[0].name, "t\tname");
        assert_eq!(
            run.cases[0].outcome,
            Outcome::Fail("C:\\proj\\a.lua:5: boom".to_string())
        );
    }

    #[test]
    fn ignores_unknown_lines_from_test_output() {
        let out = "hello from a print\n\
                   LUABOX_TEST_PASS\tworks\n\
                   42\n\
                   LUABOX_TEST_DONE\t1\t0\n";
        let run = parse(out);
        assert_eq!(run.cases.len(), 1);
        assert_eq!(run.done, Some((1, 0)));
    }

    #[test]
    fn missing_done_leaves_none() {
        let out = "LUABOX_TEST_PASS\ta\n";
        let run = parse(out);
        assert_eq!(run.done, None);
        assert_eq!(run.cases.len(), 1);
    }
}
