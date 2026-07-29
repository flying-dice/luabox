//! The diagnostics epilogue every project command shares.
//!
//! Project *layout* — discovery, the source walk, `[types] defs` resolution —
//! lives in `luabox_manifest::layout`, so the CLI, the LSP and any future
//! frontend answer "which files are this project's?" identically. What stays
//! here is the part that is genuinely the CLI's: turning a diagnostic set into
//! printed output and the tallies each command shapes its summary line from.

use std::fs;
use std::path::Path;

use luabox_diag::{Diagnostic, Format, Severity, render};

use crate::emit::outln;

/// Error/warning tallies from a diagnostic set, returned by
/// [`render_diagnostics`] so each command can shape its own summary line and
/// exit semantics — those genuinely differ (`check`/`lint` summarize to
/// stderr and fail on any error; `build`/`bundle` print a success report only;
/// `audit` has its own finding-count wording).
pub(crate) struct DiagCounts {
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

/// The diagnostics epilogue every project command shares: render `diags` in
/// `format` — resolving source snippets from files under `root` — print the
/// rendered frames to stdout when non-empty, and tally severities.
///
/// This is the common core the five reporting commands duplicated; the parts
/// that genuinely vary (the summary line's wording/shape, whether it prints on
/// success only or always, the bail message and exit code) stay in each
/// command, driven by the returned [`DiagCounts`]. `audit` folds in too: its
/// findings carry no labels, so the root-based lookup is never invoked and the
/// output is identical to its former no-op lookup.
///
/// It also records the run's **verdict** for a reader that hangs up mid-report
/// ([`crate::emit::set_exit_on_reader_gone`]), which is why the recording
/// belongs here rather than in each command: every caller of this function
/// fails iff `errors > 0` (`check`, `lint`, `doc`'s parse gate and both of
/// `build`'s gates all `bail!` on it), and this is the last moment before a
/// single byte of the report is written. `luabox check | head -1` over a
/// broken tree therefore still exits 1 — see the `emit` module docs.
pub(crate) fn render_diagnostics(diags: &[Diagnostic], format: Format, root: &Path) -> DiagCounts {
    let counts = DiagCounts {
        errors: diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        warnings: diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
    };
    // Before the first write, not after: the pipe can close on any of them.
    crate::emit::set_exit_on_reader_gone(i32::from(counts.errors > 0));

    let root = root.to_path_buf();
    let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
    let output = render(diags, format, &lookup);
    if !output.is_empty() {
        outln!("{output}");
    }
    counts
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use luabox_diag::Code;

    #[test]
    fn render_diagnostics_tallies_errors_and_warnings_separately() {
        let _guard = crate::emit::verdict_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let diags = vec![
            Diagnostic::error(Code::new(1), "boom"),
            Diagnostic::warning(Code::new(501), "meh"),
            Diagnostic::warning(Code::new(502), "also meh"),
        ];
        let counts = render_diagnostics(&diags, Format::Human, tmp.path());
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.warnings, 2);
        // ...and the verdict a reader that hangs up mid-report inherits is the
        // failure, not a silent 0 (`crate::emit`).
        assert_eq!(crate::emit::recorded_verdict(), 1);
    }

    #[test]
    fn render_diagnostics_on_an_empty_set_reports_zero_of_each() {
        let _guard = crate::emit::verdict_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let counts = render_diagnostics(&[], Format::Human, tmp.path());
        assert_eq!(counts.errors, 0);
        assert_eq!(counts.warnings, 0);
        assert_eq!(crate::emit::recorded_verdict(), 0);
    }

    #[test]
    fn warnings_alone_leave_a_departed_reader_with_a_clean_verdict() {
        // Warnings never fail `check`/`lint`, so a truncated warning report
        // must not invent a failure either.
        let _guard = crate::emit::verdict_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let diags = vec![Diagnostic::warning(Code::new(501), "meh")];
        assert_eq!(
            render_diagnostics(&diags, Format::Human, tmp.path()).warnings,
            1
        );
        assert_eq!(crate::emit::recorded_verdict(), 0);
    }

    #[test]
    fn render_diagnostics_resolves_source_snippets_from_files_under_the_root() {
        let _guard = crate::emit::verdict_lock();
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("src").join("main.lua");
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, "local x = 1\n").expect("write file");
        let diag = Diagnostic::error(Code::new(1), "bad thing").with_label(
            luabox_diag::Label::primary(luabox_diag::Span::new("src/main.lua", 6..7), "here"),
        );
        // The lookup closure is what turns a label into a rendered snippet;
        // exercising it proves the root-relative resolution works.
        let counts = render_diagnostics(std::slice::from_ref(&diag), Format::Human, tmp.path());
        assert_eq!(counts.errors, 1);
    }
}
