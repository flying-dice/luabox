//! Per-file diagnostics for publishing: parse errors, dialect legality, type
//! diagnostics, and lint findings for `.lua` files.
//!
//! Mirrors `luabox check`'s passes over one memoized parse, then runs the
//! `luabox lint` engine (the same one the CLI drives), converting every
//! finding to LSP ranges through the file's [`LineIndex`]. Control-flow
//! legality (#44) rides in with the lint engine — it is the one pass that
//! already holds the file's HIR — and is re-tagged to the toolchain source on
//! the way out, since it is not a lint rule.

use std::collections::HashSet;
use std::path::Path;

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use luabox_db::Analysis;
use luabox_lint::{LintConfig, lint_source};
use luabox_syntax::lua::{Dialect, validate};
use luabox_types::{Ambient, RockSurfaces, Strictness, check_file_with_requires};

use crate::line_index::LineIndex;
use crate::requires::RequireExports;

/// The `source` field on published type, parse, and dialect diagnostics.
const TYPE_SOURCE: &str = "luabox";

/// The code a recovered parse error is published under. `luabox_syntax`'s
/// parse errors carry a message and a range but no code of their own — the
/// toolchain assigns them all `LB0001`.
const PARSE_ERROR: luabox_diag::Code = luabox_diag::Code::new(1);

/// The `source` on published lint diagnostics, distinct from [`TYPE_SOURCE`]
/// so the editor — and the code-action matcher in [`crate::server`] — can tell
/// lint findings apart from type diagnostics. The `LB05xx` code carries the
/// specific rule.
pub(crate) const LINT_SOURCE: &str = "luabox-lint";

/// The `source` a diagnostic is published under, decided by its code.
///
/// `lint_source` also carries the control-flow legality errors (#44) —
/// `LB0020`-`LB0022` from `luabox_hir::validate`, which are *not* lint rules:
/// they have no tier and no `---@luabox-ignore` id, and the runtime refuses to
/// load the file either way. Those go out under the toolchain source alongside
/// the parse, dialect and type diagnostics; only the lint band gets
/// [`LINT_SOURCE`]. The band is `luabox_diag`'s to define
/// ([`luabox_diag::Code::is_lint`]) — an open-coded `number() / 100 == 5` here
/// was a contract nothing asserted.
///
/// Every publisher goes through this — the type pass, the parse and dialect
/// passes, the lint pass, and the code-action matcher, which re-converts the
/// originating diagnostic to pair it with its quick fix. That last site
/// hardcoded [`LINT_SOURCE`], which happened to be right for every fix that
/// exists today (all of them come from lint rules) and would silently mis-pair
/// the first fix-carrying diagnostic that does not (Shockwave round 5); the
/// first three hardcoded [`TYPE_SOURCE`], which is right for every code they
/// can emit today and would be wrong for the first one outside the band
/// (Shockwave round 6). "Every publisher" is now true rather than nearly true,
/// which is cheaper than keeping the exceptions enumerated and correct.
pub(crate) const fn source_for(code: luabox_diag::Code) -> &'static str {
    if code.is_lint() {
        LINT_SOURCE
    } else {
        TYPE_SOURCE
    }
}

/// The project's type-checking and lint context: strictness, the ambient
/// definition-package layer, and the lint configuration/known-globals baseline.
/// Owned by the server.
pub struct CheckCtx<'a> {
    pub strictness: Strictness,
    /// The MERGED ambient layer to check against — defs + workspace-global
    /// project types + rock types, as [`crate::server`]'s revision-keyed
    /// cache builds it — so a dependency's classes resolve in the editor
    /// exactly as they do under `luabox check`, and one merge serves every
    /// surface instead of being rebuilt per published file.
    pub ambient: &'a Ambient,
    /// The type surfaces harvested from the project's vendored luarocks tree
    /// (#30): rock classes/enums/aliases, plus each rock module's
    /// `require`-export type. Merged *after* the project's own types, so a name
    /// the project declares wins over a rock's (explicit beats implicit).
    pub rocks: &'a RockSurfaces,
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
pub fn diagnostics(
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
            PARSE_ERROR,
            err.message.clone(),
        ));
    }

    // 2. Dialect legality against the project edition.
    for err in validate::validate(parsed.parse(), dialect) {
        out.push(diagnostic(
            &index,
            usize::from(err.range.start())..usize::from(err.range.end()),
            DiagnosticSeverity::ERROR,
            // `DialectError::code` is the bare number; `Code` is what carries
            // the `LBnnnn` spelling — and what `source_for` reads.
            luabox_diag::Code::new(err.code),
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
    // The project's and the rock tree's `require` answers, merged by the one
    // resolver every surface shares (#54) — hover and completion read the very
    // same map, so a `require` binding cannot type one way here and another
    // way under the cursor.
    let requires = RequireExports::resolve(analysis, path, ctx.rocks);
    for diag in check_file_with_requires(
        parsed.parse(),
        &rel,
        ctx.strictness,
        dialect,
        Some(ctx.ambient),
        requires.by_module(),
    ) {
        // Type diagnostics are all `LB03xx`, so this publishes under
        // `TYPE_SOURCE` today — but through the band authority inside
        // `convert`, so a code moving band moves its source with it.
        out.push(convert(&index, &diag));
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
            // `lint_source` also carries the control-flow legality errors
            // (#44) — `LB0020`-`LB0022` from `luabox_hir::validate`, which are
            // not lint rules: they have no tier and no `---@luabox-ignore` id,
            // and the runtime refuses to load the file either way. They are
            // published under the toolchain source alongside the parse,
            // dialect and type diagnostics above; only the `LB05xx` rule
            // findings get the lint source, which is what the code-action
            // matcher keys its quick fixes off. The band is `luabox_diag`'s
            // to define ([`luabox_diag::Code::is_lint`]) — an open-coded
            // `number() / 100 == 5` here was a contract nothing asserted.
            out.push(convert(&index, diag));
        }
    }

    Some(out)
}

/// Convert a toolchain [`luabox_diag::Diagnostic`] to an LSP diagnostic through
/// `index`. Shared by the type pass, the lint pass, and the code-action
/// matcher so a lint diagnostic offered on a quick-fix is byte-identical to
/// the one published for the same finding.
///
/// The `source` is *derived* here, from [`source_for`], rather than taken as a
/// parameter. It used to be a `&str` argument, and all three non-test callers
/// passed exactly `source_for(diag.code)` — an invariant held by convention
/// across two modules, which is the shape of both source-mismatch bugs the
/// previous rounds found (a hardcoded [`LINT_SOURCE`] at the code-action site
/// in round 5, a hardcoded [`TYPE_SOURCE`] at three publishers in round 6).
/// A caller can no longer pass a source that disagrees with the code, because
/// it can no longer pass one at all (Shockwave round 7, issue X).
pub(crate) fn convert(index: &LineIndex, diag: &luabox_diag::Diagnostic) -> Diagnostic {
    let source = source_for(diag.code);
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

/// A diagnostic assembled from a code and a message rather than converted
/// from a [`luabox_diag::Diagnostic`] — the parse and dialect passes, which
/// carry their own error types.
///
/// The `source` comes from [`source_for`] like every other publisher's.
/// Neither caller can currently produce a code in the lint band, so this is
/// `TYPE_SOURCE` in practice; taking the code rather than a pre-rendered
/// string is what makes that a *derived* fact instead of a hardcoded one
/// (Shockwave round 6).
fn diagnostic(
    index: &LineIndex,
    range: std::ops::Range<usize>,
    severity: DiagnosticSeverity,
    code: luabox_diag::Code,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: index.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some(source_for(code).to_string()),
        message,
        ..Diagnostic::default()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use luabox_db::{AnalysisHost, Change};
    use luabox_types::build_ambient;

    fn path_for(name: &str) -> PathBuf {
        Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" }).join(name)
    }

    /// Diagnostics for `src` checked as `dialect`.
    fn diagnostics_for(src: &str, dialect: Dialect) -> Vec<Diagnostic> {
        diagnostics_with_rocks(src, dialect, &RockSurfaces::default())
    }

    /// [`diagnostics_for`] with harvested rock surfaces in the context (#30).
    fn diagnostics_with_rocks(
        src: &str,
        dialect: Dialect,
        rocks: &RockSurfaces,
    ) -> Vec<Diagnostic> {
        let mut host = AnalysisHost::new(dialect, Strictness::Warn);
        let path = path_for("main.lua");
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect,
            text: src.to_string(),
        });
        let analysis = host.snapshot();
        // The MERGED layer, exactly as the server's revision-keyed cache
        // builds it — the merge lives with the caller now, not in
        // `diagnostics()`.
        let base = build_ambient(dialect, &[]);
        let known_globals = base.global_names().clone();
        let ambient = base
            .with_project_types(&analysis.project_types())
            .with_rock_types(rocks.types());
        let lint = LintConfig::new();
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks,
            lint: &lint,
            known_globals: &known_globals,
        };
        diagnostics(&analysis, &path, dialect, &ctx).expect("diagnostics")
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags
            .iter()
            .filter_map(|d| match &d.code {
                Some(NumberOrString::String(code)) => Some(code.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_dialect_violation_is_reported_against_the_project_edition() {
        // `<const>` is Lua 5.4 syntax; the same source is legal there.
        let src = "local x <const> = 1\nprint(x)\n";
        let under_51 = diagnostics_for(src, Dialect::Lua51);
        let gated = under_51
            .iter()
            .find(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "LB0013"))
            .unwrap_or_else(|| panic!("expected LB0013: {under_51:?}"));
        assert_eq!(gated.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(gated.source.as_deref(), Some(TYPE_SOURCE));
        assert!(gated.message.contains("Lua 5.1"), "{gated:?}");

        assert!(
            !codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0013"),
            "legal under 5.4"
        );
    }

    #[test]
    fn a_parse_error_is_reported_with_the_syntax_code() {
        let diags = diagnostics_for("local = 1\n", Dialect::Lua54);
        assert!(codes(&diags).contains(&"LB0001"), "{diags:?}");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].source.as_deref(), Some(TYPE_SOURCE));
    }

    #[test]
    fn lint_findings_carry_the_lint_source() {
        let diags = diagnostics_for("local unused = 1\n", Dialect::Lua54);
        let lint = diags
            .iter()
            .find(|d| d.source.as_deref() == Some(LINT_SOURCE))
            .unwrap_or_else(|| panic!("expected a lint diagnostic: {diags:?}"));
        // Through the same authority the tagging uses, not a string prefix.
        let code: luabox_diag::Code = codes(std::slice::from_ref(lint))[0]
            .parse()
            .unwrap_or_else(|_| panic!("unparseable code: {lint:?}"));
        assert!(code.is_lint(), "{lint:?}");
    }

    /// The other direction: a finding the lint engine carries but that is not
    /// a lint rule (`LB0020`-`LB0022`, control-flow legality) stays on the
    /// toolchain source, so the quick-fix matcher never offers a fix for it.
    #[test]
    fn control_flow_legality_does_not_get_the_lint_source() {
        let diags = diagnostics_for("local x = 1\nbreak\n", Dialect::Lua54);
        let found = diags
            .iter()
            .find(|d| d.code == Some(lsp_types::NumberOrString::String("LB0022".to_owned())))
            .unwrap_or_else(|| panic!("expected LB0022: {diags:?}"));
        assert_eq!(found.source.as_deref(), Some(TYPE_SOURCE));
        assert!(!luabox_diag::Code::new(22).is_lint());
    }

    /// The single decision both publishers share — the diagnostic stream and
    /// the code-action matcher, which re-converts the originating diagnostic
    /// to pair it with its fix. Asserted on the band boundary rather than on
    /// whichever codes happen to carry fixes today, because the point of the
    /// helper is the case that does not exist yet: a fix-carrying diagnostic
    /// outside the lint band, which the hardcoded source would mis-pair.
    #[test]
    fn the_published_source_follows_the_lint_band_in_both_directions() {
        for number in [1_u16, 22, 300, 306, 499, 600, 601, 1001] {
            let code = luabox_diag::Code::new(number);
            assert_eq!(source_for(code), TYPE_SOURCE, "LB{number:04} is not a lint");
        }
        for number in [500_u16, 501, 509, 510, 599] {
            let code = luabox_diag::Code::new(number);
            assert_eq!(source_for(code), LINT_SOURCE, "LB{number:04} is a lint");
        }
    }

    /// The invariant the helper exists for, asserted over the published
    /// stream rather than over the helper: *every* diagnostic this module
    /// emits carries the source its code implies. Two publishers used to
    /// bypass `source_for` and hardcode [`TYPE_SOURCE`] — harmless, since
    /// neither can produce an `LB05xx`, and exactly the kind of "correct for
    /// the codes that exist today" that the round-5 code-action bug was
    /// (Shockwave round 6). Routing them through the helper kills the class;
    /// this is what notices if one grows back.
    #[test]
    fn every_published_diagnostic_carries_the_source_its_code_implies() {
        // Between them these reach all four publishers: a recovered parse
        // error (LB0001), a dialect violation under 5.1 (LB0013), a type
        // diagnostic (LB03xx) and a lint finding (LB05xx). The lint pass is
        // skipped when the parse is dirty, so the parse-error case has to be
        // its own source.
        let sources = [
            "local y: = 3\n",
            "local x <const> = 1\nlocal unused = 2\nreturn x\n",
            "---@type string\nlocal s = 1\nlocal spare = 2\nreturn s\n",
        ];
        let mut bands = HashSet::new();
        let mut seen = 0_usize;
        for src in sources {
            for diag in &diagnostics_for(src, Dialect::Lua51) {
                let Some(NumberOrString::String(spelling)) = &diag.code else {
                    panic!("every diagnostic carries an LBnnnn code: {diag:?}");
                };
                let code: luabox_diag::Code = spelling.parse().expect("a well-formed code");
                assert_eq!(
                    diag.source.as_deref(),
                    Some(source_for(code)),
                    "{spelling} published under the wrong source: {diag:?}"
                );
                bands.insert(code.is_lint());
                seen += 1;
            }
        }
        assert!(seen >= 4, "expected a mixed stream, saw {seen}");
        assert_eq!(bands.len(), 2, "both bands must be represented");
    }

    // --- harvested rock surfaces (#30) -----------------------------------

    /// The surfaces of one annotated rock installed as `mylib`.
    fn mylib_rock() -> RockSurfaces {
        let source = luabox_types::RockModule {
            module: "mylib".to_string(),
            label: "lua_modules/share/lua/5.4/mylib/init.lua".to_string(),
            path: path_for("lua_modules/share/lua/5.4/mylib/init.lua"),
            text: "\
---@class mylib.Point
---@field x number
---@field y number

local M = {}

---@param x number
---@param y number
---@return mylib.Point
function M.point(x, y)
  return { x = x, y = y }
end

return M
"
            .to_string(),
        };
        let ambient = build_ambient(Dialect::Lua54, &[]);
        luabox_types::rocks::harvest(&ambient, &[source])
    }

    #[test]
    fn a_harvested_rock_class_resolves_in_the_editor() {
        let src = "---@type mylib.Point\nlocal p = { x = 1, y = 2 }\nreturn p\n";
        // Without the tree the class is an unknown type name…
        assert!(
            codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0305"),
            "expected LB0305 without a rock tree"
        );
        // …and with it, it resolves and the literal conforms.
        let with_rock = diagnostics_with_rocks(src, Dialect::Lua54, &mylib_rock());
        assert!(codes(&with_rock).is_empty(), "{with_rock:?}");
    }

    #[test]
    fn a_rock_module_export_types_a_require_the_database_cannot_resolve() {
        // `mylib` is not a project file, so the db resolves nothing; the rock's
        // export type is matched by module name instead, and its `---@return`
        // flows into the consumer's use site.
        let src = "\
---@param s string
local function want(s) end
local mylib = require(\"mylib\")
want(mylib.point(1, 2))
";
        let diags = diagnostics_with_rocks(src, Dialect::Lua54, &mylib_rock());
        assert!(
            codes(&diags).contains(&"LB0300"),
            "the rock's return type must reach the consumer: {diags:?}"
        );
        // The same source without the tree cannot know what `mylib` is.
        assert!(!codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0300"));
    }

    #[test]
    fn a_file_the_analysis_does_not_know_has_no_diagnostics() {
        let host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let analysis = host.snapshot();
        let ambient = build_ambient(Dialect::Lua54, &[]);
        let lint = LintConfig::new();
        let known_globals = ambient.global_names().clone();
        let rocks = RockSurfaces::default();
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks: &rocks,
            lint: &lint,
            known_globals: &known_globals,
        };
        assert!(diagnostics(&analysis, &path_for("absent.lua"), Dialect::Lua54, &ctx).is_none());
    }
}
