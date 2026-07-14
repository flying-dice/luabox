//! Per-file diagnostics for publishing: parse errors, dialect legality, type
//! diagnostics, and lint findings for `.lua` files.
//!
//! Mirrors `luabox check`'s three passes over one memoized parse, then runs
//! the `luabox lint` engine (the same one the CLI drives), converting every
//! finding to LSP ranges through the file's [`LineIndex`].

use std::collections::HashSet;
use std::path::Path;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use luabox_db::Analysis;
use luabox_lint::{LintConfig, lint_source};
use luabox_syntax::lua::{Dialect, validate};
use luabox_types::{Ambient, Strictness, check_file_with_requires};

use crate::line_index::LineIndex;

/// The `source` field on published type, parse, and dialect diagnostics.
const TYPE_SOURCE: &str = "luabox";

/// The `source` on published lint diagnostics, distinct from [`TYPE_SOURCE`]
/// so the editor — and the code-action matcher in [`crate::server`] — can tell
/// lint findings apart from type diagnostics. The `LB05xx` code carries the
/// specific rule.
pub(crate) const LINT_SOURCE: &str = "luabox-lint";

/// The project's type-checking and lint context: strictness, the ambient
/// definition-package layer, and the lint configuration/known-globals baseline.
/// Owned by the server.
pub struct CheckCtx<'a> {
    pub strictness: Strictness,
    /// The ambient layer to check against — the editor's counterpart of the
    /// CLI's `build_ambient_checked` ambient, so a dependency's classes resolve
    /// in the editor exactly as they do under `luabox check`.
    pub ambient: &'a Ambient,
    /// The resolved `[lint]` configuration (tiers/rules/allowed globals), built
    /// from the manifest the same way `luabox lint` builds it.
    pub lint: &'a LintConfig,
    /// The `undefined-global` known-globals baseline (dialect stdlib + project
    /// and dependency defs), built the same way `luabox lint` builds it.
    pub known_globals: &'a HashSet<String>,
}

/// Diagnostics for one `.lua` file known to `analysis`, plus the line index
/// used to convert them. `None` when the file is unknown.
#[must_use]
pub fn lua_diagnostics(
    analysis: &Analysis,
    path: &Path,
    dialect: Dialect,
    ctx: &CheckCtx<'_>,
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

    // 3. Types against the ambient definition-package layer — the same pass
    // as `luabox check`, so classes resolve identically in the editor and in
    // CI. The file's cross-file `require` exports are in reach (#85), so a
    // `require("mod")` result types from the module's annotations in the
    // editor exactly as under `luabox check`; the project's workspace-global
    // classes (declared in any checked file, member attachments included —
    // luals parity) merge beneath the defs layer the same way `check_cmd`
    // merges them. The span file name is dropped on conversion (LSP
    // diagnostics are already per-document), so the lossy path is fine.
    let rel = path.to_string_lossy();
    let requires = analysis.require_exports(path).unwrap_or_default();
    let project_types = analysis.project_types();
    let ambient = ctx.ambient.with_project_types(&project_types);
    for diag in check_file_with_requires(
        parsed.parse(),
        &rel,
        ctx.strictness,
        dialect,
        Some(&ambient),
        &requires,
    ) {
        out.push(convert(&index, &diag, TYPE_SOURCE));
    }

    // 4. Lint findings — the `luabox lint` engine (SPEC.md §9), published
    // alongside the type diagnostics. `lint_source` applies the `[lint]`
    // tiers/config and `---@luabox-ignore` suppression itself. It re-parses
    // internally (the double-parse cost is per keystroke, but lint only runs
    // over one small file at a time). Skipped when the parse is not clean:
    // lint's own parse-error diagnostics would duplicate pass 1's, and lint
    // findings over a recovered tree are transient editor noise.
    if parsed.errors().is_empty() {
        let outcome = lint_source(&rel, index.text(), dialect, ctx.lint, ctx.known_globals);
        for diag in &outcome.diagnostics {
            out.push(convert(&index, diag, LINT_SOURCE));
        }
    }

    Some(out)
}

/// Convert a toolchain [`luabox_diag::Diagnostic`] to an LSP diagnostic through
/// `index`, tagging it with `source`. Shared by the type pass, the lint pass,
/// and the code-action matcher so a lint diagnostic offered on a quick-fix is
/// byte-identical to the one published for the same finding.
pub(crate) fn convert(
    index: &LineIndex,
    diag: &luabox_diag::Diagnostic,
    source: &str,
) -> Diagnostic {
    let range = diag
        .primary_label()
        .map_or(0..0, |label| label.span.range.clone());
    let severity = match diag.severity {
        luabox_diag::Severity::Error => DiagnosticSeverity::ERROR,
        luabox_diag::Severity::Warning => DiagnosticSeverity::WARNING,
    };
    let mut message = diag.message.clone();
    for note in &diag.notes {
        message.push('\n');
        message.push_str(note);
    }
    Diagnostic {
        range: index.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(diag.code.to_string())),
        source: Some(source.to_string()),
        message,
        ..Diagnostic::default()
    }
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
        source: Some(TYPE_SOURCE.to_string()),
        message,
        ..Diagnostic::default()
    }
}
