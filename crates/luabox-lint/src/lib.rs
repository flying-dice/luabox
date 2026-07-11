//! Type-informed lint rules — the clippy analog (SPEC.md §9).
//!
//! Consumer crate of the Semantics context (like `luabox-lsp`), added
//! alongside the SPEC §16 crate list the way clippy sits beside rustc.
//! All rules run over the same parse/HIR/type machinery as `check` —
//! no regex lints. Tiers: correctness (deny), suspicious, perf, style,
//! pedantic (opt-in). `--fix` applies machine-applicable fixes via the
//! lossless tree.
//!
//! # Shape
//!
//! - [`rule::Rule`] — one analysis: id (kebab-case, also the human name),
//!   [`rule::Tier`], `LB05xx` [`luabox_diag::Code`], and `check`.
//! - [`context::LintContext`] — per-file inputs: parse tree, HIR
//!   [`luabox_hir::LoweredFile`], declared-type [`facts::TypeFacts`], config,
//!   and a shared binding use-index.
//! - [`diagnostic::LintDiagnostic`] / [`diagnostic::Fix`] — what a rule emits.
//! - [`lint_source`] — orchestrates: parse → lower → facts → suppression →
//!   rules → config levels → [`luabox_diag::Diagnostic`]s + fixes.
//!
//! Suppression is `---@luabox-ignore rule-id reason` (reason mandatory —
//! a bare tag is itself `LB0500`). Config is `[lint]` in the manifest,
//! translated into a [`config::LintConfig`] by the Frontend.

mod config;
mod context;
mod diagnostic;
mod facts;
mod rule;
mod rules;
mod suppress;

#[cfg(test)]
mod tests;

pub use config::{Level, LintConfig, tier_default};
pub use context::LintContext;
pub use diagnostic::{Fix, LintDiagnostic};
pub use facts::TypeFacts;
pub use rule::{Rule, Tier};
pub use rules::rules;

use std::collections::HashSet;
use std::ops::Range;

use luabox_diag::{Code, Diagnostic, Label, Severity, Span, Suggestion};
use luabox_hir::lower;
use luabox_syntax::{Dialect, lua};

use context::to_range;
use suppress::{Suppressions, line_of};

/// A machine-applicable edit gathered for `--fix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// The byte range to replace.
    pub range: Range<usize>,
    /// The replacement text.
    pub replacement: String,
}

/// The result of linting one file.
#[derive(Debug, Default)]
pub struct LintOutcome {
    /// Diagnostics ready to render, sorted by span.
    pub diagnostics: Vec<Diagnostic>,
    /// Machine-applicable edits from non-suppressed findings (empty when the
    /// file had parse errors — never fix a broken file).
    pub fixes: Vec<TextEdit>,
    /// Whether the parser reported any errors.
    pub had_parse_errors: bool,
    /// Number of error-severity diagnostics (deny-tier findings, malformed
    /// ignores, and parse errors) — nonzero means the command should fail.
    pub error_count: usize,
}

/// Lint one Lua source string.
///
/// `known_globals` is the project's known-global baseline — the dialect
/// stdlib plus any `[types] defs` packages (SPEC.md §3) — built by the
/// caller the same way `luabox check` builds its `Ambient` layer (see
/// `luabox-cli`'s `lint_cmd::known_globals`). It feeds the `undefined-global`
/// rule; the `[lint] globals` allow-list is separate and lives on `config`.
#[must_use]
#[allow(
    clippy::implicit_hasher,
    reason = "internal API — every caller passes a plain `HashSet<String>`, never a custom hasher"
)]
pub fn lint_source(
    file: &str,
    source: &str,
    dialect: Dialect,
    config: &LintConfig,
    known_globals: &HashSet<String>,
) -> LintOutcome {
    let parse = lua::parse(source, dialect);
    let mut diagnostics = Vec::new();
    let mut fixes = Vec::new();
    let had_parse_errors = !parse.errors().is_empty();

    for err in parse.errors() {
        diagnostics.push(
            Diagnostic::error(Code::new(1), err.message.clone()).with_label(Label::primary(
                Span::new(file, to_range(err.range)),
                "syntax error here",
            )),
        );
    }

    let lowered = lower(&parse);
    let facts = TypeFacts::build(&parse, &lowered);
    let suppress = Suppressions::collect(&parse, source);
    let ctx = LintContext::new(
        file,
        source,
        &parse,
        &lowered,
        &facts,
        config,
        known_globals,
    );

    for rule in rules() {
        let Some(severity) = config.effective(rule.as_ref()).severity() else {
            continue;
        };
        for finding in rule.check(&ctx) {
            let line = line_of(source, finding.range.start);
            if suppress.is_suppressed(rule.id(), line) {
                continue;
            }
            let mut diag = Diagnostic::new(rule.code(), severity, finding.message.clone())
                .with_label(Label::primary(Span::new(file, finding.range.clone()), ""));
            for (range, message) in &finding.secondary {
                diag = diag.with_label(Label::secondary(
                    Span::new(file, range.clone()),
                    message.clone(),
                ));
            }
            if let Some(fix) = &finding.fix {
                diag = diag.with_suggestion(Suggestion::new(
                    Span::new(file, fix.range.clone()),
                    fix.replacement.clone(),
                    format!("{}: apply fix", rule.id()),
                ));
                if fix.is_machine_applicable && !had_parse_errors {
                    fixes.push(TextEdit {
                        range: fix.range.clone(),
                        replacement: fix.replacement.clone(),
                    });
                }
            }
            for note in &finding.notes {
                diag = diag.with_note(note.clone());
            }
            diag = diag.with_note(format!(
                "{} lint ({}); silence with `---@luabox-ignore {} <reason>`",
                rule.id(),
                rule.tier().name(),
                rule.id()
            ));
            diagnostics.push(diag);
        }
    }

    // Malformed `---@luabox-ignore` tags are themselves correctness-tier
    // diagnostics and cannot be suppressed.
    for (range, message) in &suppress.malformed {
        diagnostics.push(
            Diagnostic::error(Code::new(500), message.clone())
                .with_label(Label::primary(Span::new(file, range.clone()), ""))
                .with_note("malformed-ignore lint (correctness)"),
        );
    }

    diagnostics.sort_by(|a, b| {
        let key = |d: &Diagnostic| {
            d.primary_label()
                .map_or((0, 0), |l| (l.span.range.start, l.span.range.end))
        };
        key(a).cmp(&key(b))
    });
    let error_count = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();

    LintOutcome {
        diagnostics,
        fixes,
        had_parse_errors,
        error_count,
    }
}

/// Apply text edits to a source string: non-overlapping, innermost-first.
///
/// Overlapping edits are resolved by keeping the earliest and dropping any
/// that overlaps an already-chosen one; the survivors are applied back-to-front
/// so byte offsets stay valid. Applying the fixes of a `--fix` pass and
/// re-linting converges (the tests assert idempotence).
#[must_use]
pub fn apply_fixes(source: &str, edits: &[TextEdit]) -> String {
    let mut sorted: Vec<&TextEdit> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        a.range
            .start
            .cmp(&b.range.start)
            .then(a.range.end.cmp(&b.range.end))
    });
    let mut chosen: Vec<&TextEdit> = Vec::new();
    let mut last_end = 0usize;
    for edit in sorted {
        if edit.range.start >= last_end {
            last_end = edit.range.end;
            chosen.push(edit);
        }
    }
    let mut result = source.to_owned();
    for edit in chosen.iter().rev() {
        if edit.range.end <= result.len() {
            result.replace_range(edit.range.clone(), &edit.replacement);
        }
    }
    result
}
