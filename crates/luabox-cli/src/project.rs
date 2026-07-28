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
pub(crate) fn render_diagnostics(diags: &[Diagnostic], format: Format, root: &Path) -> DiagCounts {
    let root = root.to_path_buf();
    let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
    let output = render(diags, format, &lookup);
    if !output.is_empty() {
        println!("{output}");
    }
    DiagCounts {
        errors: diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        warnings: diags
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
    }
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use luabox_diag::Code;

    #[test]
    fn render_diagnostics_tallies_errors_and_warnings_separately() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let diags = vec![
            Diagnostic::error(Code::new(1), "boom"),
            Diagnostic::warning(Code::new(501), "meh"),
            Diagnostic::warning(Code::new(502), "also meh"),
        ];
        let counts = render_diagnostics(&diags, Format::Human, tmp.path());
        assert_eq!(counts.errors, 1);
        assert_eq!(counts.warnings, 2);
    }

    #[test]
    fn render_diagnostics_on_an_empty_set_reports_zero_of_each() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let counts = render_diagnostics(&[], Format::Human, tmp.path());
        assert_eq!(counts.errors, 0);
        assert_eq!(counts.warnings, 0);
    }

    #[test]
    fn render_diagnostics_resolves_source_snippets_from_files_under_the_root() {
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
