//! Per-file diagnostics for publishing: parse errors, dialect legality, and
//! type diagnostics for `.lua`; shape parse errors for `.lb`.
//!
//! Mirrors `luabox check`'s three passes over one memoized parse, converted
//! to LSP ranges through the file's [`LineIndex`].

use std::path::Path;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use luabox_db::Analysis;
use luabox_syntax::lua::{Dialect, validate};
use luabox_syntax::shape;

use crate::line_index::LineIndex;

/// Diagnostics for one `.lua` file known to `analysis`, plus the line index
/// used to convert them. `None` when the file is unknown.
#[must_use]
pub fn lua_diagnostics(
    analysis: &Analysis,
    path: &Path,
    dialect: Dialect,
) -> Option<Vec<Diagnostic>> {
    let text = analysis.file_text(path)?;
    let index = LineIndex::new(text);
    let parsed = analysis.parse(path)?;
    let mut out = Vec::new();

    // 1. Parse errors (the tree is recovered; later passes still run).
    for err in parsed.errors() {
        out.push(diagnostic(
            &index,
            usize::from(err.range.start())..usize::from(err.range.end()),
            DiagnosticSeverity::ERROR,
            "LB0001",
            err.message.clone(),
        ));
    }

    // 2. Dialect legality against the project edition.
    for err in validate::validate(parsed.parse(), dialect) {
        out.push(diagnostic(
            &index,
            usize::from(err.range.start())..usize::from(err.range.end()),
            DiagnosticSeverity::ERROR,
            err.code,
            err.message,
        ));
    }

    // 3. Type diagnostics (annotation-driven, per-file).
    for diag in analysis.diagnostics(path)? {
        let range = diag
            .primary_label()
            .map_or(0..0, |label| label.span.range.clone());
        let severity = match diag.severity {
            luabox_diag::Severity::Error => DiagnosticSeverity::ERROR,
            luabox_diag::Severity::Warning => DiagnosticSeverity::WARNING,
            luabox_diag::Severity::Note => DiagnosticSeverity::INFORMATION,
            luabox_diag::Severity::Help => DiagnosticSeverity::HINT,
        };
        let mut message = diag.message.clone();
        for note in &diag.notes {
            message.push('\n');
            message.push_str(note);
        }
        out.push(diagnostic(
            &index,
            range,
            severity,
            &diag.code.to_string(),
            message,
        ));
    }

    Some(out)
}

/// Diagnostics for one `.lb` shape file: its parse errors (including the
/// `LB2010` body rejection), straight from `shape::parse` over `text`.
#[must_use]
pub fn lb_diagnostics(text: &str) -> Vec<Diagnostic> {
    let index = LineIndex::new(text);
    shape::parse(text)
        .errors()
        .iter()
        .map(|err| {
            diagnostic(
                &index,
                usize::from(err.range.start())..usize::from(err.range.end()),
                DiagnosticSeverity::ERROR,
                err.code.unwrap_or("LB0001"),
                err.message.clone(),
            )
        })
        .collect()
}

fn diagnostic(
    index: &LineIndex,
    range: std::ops::Range<usize>,
    severity: DiagnosticSeverity,
    code: &str,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: index.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some("luabox".to_string()),
        message,
        ..Diagnostic::default()
    }
}
