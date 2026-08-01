//! `luabox lint [--fix] [--format <f>]` — the clippy analog (SPEC.md §9).
//!
//! Discovers the project (nearest `luabox.toml`, cargo-style), lints every
//! `.lua` file in parallel over the shared parse/HIR/type machinery, and
//! renders findings in the requested format. Tiers and per-rule levels come
//! from `[lint]` in the manifest; the exit code is nonzero iff any deny-tier
//! finding (or a parse error / malformed suppression) was produced — warnings
//! never fail the command.
//!
//! **`--format`** (#53) is the same closed set `luabox check` carries, through
//! the same renderer: `lint` and `check` produce the same
//! [`luabox_diag::Diagnostic`] values, so a second rendering path for them
//! would only be a way for the two to disagree. Both go through
//! [`crate::project::render_diagnostics`].
//!
//! The exit code does **not** move with the format, and that is the point of
//! carrying severity faithfully into the machine formats. A warn-tier finding
//! still exits 0 (SPEC.md §9) — luabox does not decide that a warning should
//! fail somebody's build. A CI consumer that *does* want to gate on warnings
//! reads the severity back out of the report and makes that call itself,
//! which it can only do if the report says `warning` where the human
//! rendering says `warning`. A `[lint]` deny escalation moves both together:
//! the finding is reported as an error *and* the command exits 1.
//!
//! `--fix` applies machine-applicable fixes to disk (innermost-first,
//! non-overlapping), re-linting each file until it converges. A file with parse
//! errors is never rewritten. A `lint: N errors, M warnings in K files` summary
//! goes to stderr.
//!
//! **`[lint]` config problems** (CC-M8): a key that names neither `globals`,
//! a tier, nor a rule this build has is reported once as an `LB1004` warning
//! before the per-file findings, with a "did you mean" nudge across both
//! vocabularies. It is a warning because the entry is merely inert — the exit
//! code is unchanged.
//!
//! **Known globals** (ticket #103, `undefined-global`): the dialect stdlib
//! plus any `[types] defs` packages, resolved from `defs/` the same way
//! `luabox check` builds its `Ambient` layer — one shared walk in
//! `luabox_manifest::layout`, so the two commands cannot disagree about which
//! globals a project declares. Test files
//! (SPEC.md §11: `*_test.lua`/`*.test.lua`/anything under `tests/`)
//! additionally see the conventional busted-style test globals (`describe`,
//! `it`, `before_each`, `after_each`, `test`), which a test framework injects
//! as globals at run time — so a test file using them doesn't spuriously trip
//! `undefined-global`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format};
use luabox_lint::{LintConfig, UnknownRuleId, apply_fixes, lint_source};
use luabox_manifest::layout::{self, DefFiles};
use luabox_manifest::model::Manifest;
use luabox_syntax::Dialect;
use luabox_types::{Ambient, build_ambient, stdlib_defs};
use rayon::prelude::*;

use layout::display_rel;

use crate::emit::errln;

/// The most fix passes to run per file before giving up on convergence.
const MAX_FIX_PASSES: usize = 8;

/// Conventional busted-style test globals a test framework injects before
/// running a test file: `describe`/`it`/`before_each`/`after_each` plus the
/// native flat `test`. `assert` is not listed — it's already stdlib.
const TEST_HARNESS_GLOBALS: [&str; 5] = ["describe", "it", "before_each", "after_each", "test"];

/// Execute `luabox lint` from `cwd`, rendering findings in `format`.
pub fn run(cwd: &Path, fix: bool, format: Format) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let files =
        layout::collect_lua_files(&project.root, project.out_dir.as_deref(), DefFiles::Include)?;

    // SPEC.md §16: rayon per file — files are independent.
    let per_file: Vec<anyhow::Result<FileResult>> = files
        .par_iter()
        .map(|path| lint_one(path, &project, fix))
        .collect();

    let mut diags: Vec<Diagnostic> = Vec::new();
    let mut fixed_files = 0usize;
    for result in per_file {
        let result = result?;
        if result.was_fixed {
            fixed_files += 1;
        }
        diags.extend(result.diagnostics);
    }

    diags.sort_by(|a, b| {
        let key = |d: &Diagnostic| {
            (
                d.primary_label()
                    .map_or(String::new(), |l| l.span.file.clone()),
                d.primary_label().map_or(0, |l| l.span.range.start),
            )
        };
        key(a).cmp(&key(b))
    });

    // Config problems lead: they are about the manifest, not any one file,
    // and they explain why a rule the reader thought they had configured is
    // still firing (or still silent).
    let mut report: Vec<Diagnostic> = project
        .unknown_lint_rules
        .iter()
        .map(unknown_rule_diagnostic)
        .collect();
    report.append(&mut diags);

    finish(
        &report,
        format,
        &project.root,
        files.len(),
        fixed_files,
        fix,
    )
}

/// One file's diagnostics plus whether `--fix` rewrote it.
struct FileResult {
    diagnostics: Vec<Diagnostic>,
    was_fixed: bool,
}

/// Lint one file, applying and re-checking fixes when `fix` is set.
fn lint_one(path: &Path, project: &Project, fix: bool) -> anyhow::Result<FileResult> {
    let rel = display_rel(path, &project.root);
    let original = fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;

    // `undefined-global` (ticket #103): the project's known-globals baseline,
    // widened with the conventional busted-style test globals for test files
    // (`*_test.lua`/`*.test.lua`/under `tests/`) — a test framework injects
    // `describe`/`it`/... as run-time globals, though they're never declared
    // in a `.d.lua` defs package.
    let is_test_file = is_test_file(&rel);
    let mut known_owned;
    let known_globals: &HashSet<String> = if is_test_file {
        known_owned = project.known_globals.clone();
        known_owned.extend(TEST_HARNESS_GLOBALS.iter().map(|s| (*s).to_owned()));
        &known_owned
    } else {
        &project.known_globals
    };

    let mut source = original.clone();
    let mut outcome = lint_source(&rel, &source, project.dialect, &project.lint, known_globals);

    // Never rewrite a file with parse errors.
    if fix && !outcome.had_parse_errors {
        let mut passes = 0;
        while !outcome.fixes.is_empty() && passes < MAX_FIX_PASSES {
            source = apply_fixes(&source, &outcome.fixes);
            outcome = lint_source(&rel, &source, project.dialect, &project.lint, known_globals);
            passes += 1;
        }
    }

    let was_fixed = fix && source != original;
    if was_fixed {
        // Never `fs::write`: that truncates the user's source before it writes
        // a byte, so a failed write destroys it. See `crate::atomic_write`.
        crate::atomic_write::write_atomic(path, &source)
            .with_context(|| format!("cannot write `{rel}`"))?;
    }

    Ok(FileResult {
        diagnostics: outcome.diagnostics,
        was_fixed,
    })
}

/// Render, summarize, and translate the error count into the exit code.
///
/// The summary goes to **stderr** and the report to stdout, so a machine
/// format is never adulterated by the `lint: N errors, M warnings` line — a
/// consumer can pipe stdout straight into a parser.
///
/// The error count, and therefore the exit code, is read off the same
/// diagnostics whatever `format` renders them as: the report never changes
/// the verdict and the verdict never changes the report.
fn finish(
    diags: &[Diagnostic],
    format: Format,
    root: &Path,
    file_count: usize,
    fixed_files: usize,
    fix: bool,
) -> anyhow::Result<()> {
    let counts = crate::project::render_diagnostics(diags, format, root);
    let (errors, warnings) = (counts.errors, counts.warnings);
    if fix {
        errln!(
            "lint: {errors} errors, {warnings} warnings in {file_count} files ({fixed_files} fixed)"
        );
    } else {
        errln!("lint: {errors} errors, {warnings} warnings in {file_count} files");
    }
    if errors > 0 {
        bail!("lint failed with {errors} error(s)");
    }
    Ok(())
}

struct Project {
    root: PathBuf,
    dialect: Dialect,
    out_dir: Option<PathBuf>,
    lint: LintConfig,
    /// `undefined-global`'s known-globals baseline (ticket #103): the
    /// dialect stdlib, plus any `[types] defs` packages resolved from
    /// `defs/` — the same `Ambient` layer `luabox check` builds, so a
    /// project's ambient LuaCATS globals (`love`, project-specific
    /// `.d.lua` packages, ...) don't spuriously trip the lint.
    known_globals: HashSet<String>,
    /// `[lint]` keys that name no known rule id — reported as `LB1004`
    /// warnings before the per-file findings, since the manifest parser
    /// cannot validate ids that live in `luabox-lint` (CC-M8).
    unknown_lint_rules: Vec<UnknownRuleId>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (Lua 5.4, empty lint config).
fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let Some((root, manifest)) = layout::discover_manifest(cwd)? else {
        return Ok(Project {
            root: cwd.to_path_buf(),
            dialect: Dialect::Lua54,
            out_dir: None,
            lint: LintConfig::new(),
            known_globals: stdlib_defs(Dialect::Lua54).global_names().clone(),
            unknown_lint_rules: Vec::new(),
        });
    };
    // `Manifest::parse` types `[package] edition` as a `DialectId`, so there
    // is nothing left to re-validate here (CC-M13).
    let dialect = crate::dialect::from_manifest(manifest.package.edition);
    let known_globals = known_globals(dialect, &root, &manifest);
    // The `[lint]` translation is `luabox-lint`'s single one, shared with the
    // LSP; it hands back the unknown ids rather than swallowing them.
    let (lint, unknown_lint_rules) = LintConfig::from_manifest(&manifest.lint);
    Ok(Project {
        out_dir: Some(root.join(&manifest.build.out)),
        dialect,
        lint,
        known_globals,
        unknown_lint_rules,
        root,
    })
}

/// The `undefined-global` known-globals baseline for one project: the
/// dialect stdlib, widened with any `[types] defs` packages resolved from
/// `<root>/defs/` (SPEC.md §3) *and* every direct dependency's own `[types]
/// defs` (#108, the luals `workspace.library` model — a dependency's ambient
/// globals must not spuriously trip the consumer's `undefined-global`).
/// Resolution is `luabox_manifest::layout`'s — the same walk `luabox check`
/// and the LSP run, so all three see one set of ambient globals. Only the
/// texts matter here (a global is a global, whatever file declared it), so the
/// labels are dropped. A project-defs entry that fails to resolve is silently
/// skipped: `luabox check` is the command that reports `LB1002`; `lint` falls
/// back to the stdlib-only baseline for that entry.
fn known_globals(dialect: Dialect, root: &Path, manifest: &Manifest) -> HashSet<String> {
    let (project_defs, _unresolved) = layout::resolve_project_defs(root, &manifest.types.defs);
    let sources: Vec<String> = project_defs
        .into_iter()
        .chain(layout::resolve_dep_defs(root, manifest))
        .map(|def| def.text)
        .collect();
    if sources.is_empty() {
        return stdlib_defs(dialect).global_names().clone();
    }
    let ambient: Ambient = build_ambient(dialect, &sources);
    ambient.global_names().clone()
}

/// `LB1004` — a `[lint]` key naming no known rule id (nor a tier).
///
/// Not `LB1003`: SPEC.md §6 reserves that number for the parked dependency
/// dialect-set check.
const UNKNOWN_LINT_RULE: Code = Code::new(1004);

/// The note every unknown-rule-id report carries, pointing at the page that
/// lists both vocabularies.
const UNKNOWN_LINT_RULE_NOTE: &str =
    "run `luabox explain LB1004` for the known tier names and rule ids";

/// The rendered `LB1004` for one unknown `[lint]` key.
///
/// A **warning**: the entry is inert, not fatal, so a manifest shared with a
/// toolchain that has a rule this build does not still lints (and the exit
/// code is unchanged). The message and the "did you mean" nudge are
/// `luabox-lint`'s, so the editor words this identically.
fn unknown_rule_diagnostic(unknown: &UnknownRuleId) -> Diagnostic {
    let mut diag = Diagnostic::warning(UNKNOWN_LINT_RULE, unknown.message());
    for note in unknown.notes() {
        diag = diag.with_note(note);
    }
    diag.with_note(UNKNOWN_LINT_RULE_NOTE)
}

/// Whether `rel` (a root-relative, forward-slash path) is a test file
/// (SPEC.md §11: `*_test.lua`, `*.test.lua`, or anything under a `tests/`
/// directory). `*.d.lua` definition files are never tests, even under
/// `tests/`.
fn is_test_file(rel: &str) -> bool {
    let mut segments = rel.split('/');
    let name = segments.next_back().unwrap_or(rel);
    let in_tests = segments.any(|seg| seg == "tests");
    let is_lua = Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("lua"));
    if !is_lua || name.ends_with(".d.lua") {
        return false;
    }
    in_tests || name.ends_with("_test.lua") || name.ends_with(".test.lua")
}

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::testutil::{read, write};

    /// Every lint fixture is a 5.4 project — the dialect is never the subject
    /// here — so the edition is pinned rather than threaded through each call.
    fn manifest(extra: &str) -> String {
        crate::testutil::manifest("5.4", extra)
    }

    fn project(extra: &str) -> tempfile::TempDir {
        crate::testutil::project("5.4", extra)
    }

    // -- report formats (#53) ----------------------------------------------

    /// Every format `check` carries, `lint` carries — same closed set, same
    /// renderer. The exit code is lint's own and must not move with it.
    const EVERY_FORMAT: [Format; 5] = [
        Format::Human,
        Format::Json,
        Format::Sarif,
        Format::GithubActions,
        Format::GitlabCodeQuality,
    ];

    /// `metatable-without-index` (LB0510, suspicious tier): a warn-tier
    /// finding, so the command still exits 0 while reporting it.
    const LB0510_REPRO: &str = "\
---@class Counter
---@field n integer
local Counter = {}

function Counter:value()
  return self.n
end

local c = setmetatable({ n = 1 }, Counter)
return c:value()
";

    #[test]
    fn a_warn_tier_finding_leaves_the_exit_code_at_zero_in_every_format() {
        for format in EVERY_FORMAT {
            let tmp = project("");
            write(tmp.path(), "src/main.lua", LB0510_REPRO);
            run(tmp.path(), false, format)
                .unwrap_or_else(|e| panic!("a warning must not fail lint in {format:?}: {e}"));
        }
    }

    /// A `[lint]` deny escalation has to move the exit code in every format
    /// alike — the rendering never decides the verdict.
    #[test]
    fn a_deny_escalation_fails_the_command_in_every_format() {
        for format in EVERY_FORMAT {
            let tmp = project("\n[lint]\nsuspicious = \"deny\"\n");
            write(tmp.path(), "src/main.lua", LB0510_REPRO);
            let error = run(tmp.path(), false, format).unwrap_err().to_string();
            assert!(error.contains("lint failed with"), "in {format:?}: {error}");
        }
    }

    /// An empty project is the happy path a machine consumer still has to
    /// parse, so the document must be well-formed and empty rather than
    /// absent — matching what `check` already emits.
    #[test]
    fn a_clean_project_still_emits_a_valid_document_in_every_format() {
        for format in EVERY_FORMAT {
            let tmp = project("");
            write(
                tmp.path(),
                "src/main.lua",
                "local x = 1\nprint(x)\nreturn 0\n",
            );
            run(tmp.path(), false, format).expect("a clean project lints successfully");
        }
    }

    // -- the command -------------------------------------------------------

    #[test]
    fn a_clean_project_lints_successfully() {
        let tmp = project("");
        write(
            tmp.path(),
            "src/main.lua",
            "local x = 1\nprint(x)\nreturn 0\n",
        );
        run(tmp.path(), false, Format::Human).expect("lint passes");
    }

    #[test]
    fn an_empty_project_lints_successfully() {
        let tmp = project("");
        run(tmp.path(), false, Format::Human).expect("lint passes");
    }

    #[test]
    fn a_style_tier_finding_warns_without_failing_the_command() {
        let tmp = project("");
        write(tmp.path(), "src/main.lua", "local unused = 1\nreturn 0\n");
        // `unused-local` is style-tier: a warning, exit zero (SPEC.md §9).
        run(tmp.path(), false, Format::Human).expect("warnings do not fail lint");
    }

    #[test]
    fn a_correctness_tier_finding_fails_the_command() {
        let tmp = project("");
        // A `---@luabox-ignore` without a reason is itself a correctness
        // finding (LB0500).
        write(
            tmp.path(),
            "src/main.lua",
            "---@luabox-ignore unused-local\nlocal x = 1\nreturn 0\n",
        );
        let error = run(tmp.path(), false, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.contains("lint failed with"), "{error}");
        assert!(error.contains("error(s)"), "{error}");
    }

    #[test]
    fn a_tier_promoted_to_deny_in_the_manifest_fails_the_command() {
        let tmp = project("\n[lint]\nstyle = \"deny\"\n");
        write(tmp.path(), "src/main.lua", "local unused = 1\nreturn 0\n");
        let error = run(tmp.path(), false, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.contains("lint failed with"), "{error}");
    }

    #[test]
    fn a_rule_allowed_in_the_manifest_stops_being_reported() {
        let tmp = project("\n[lint]\nstyle = \"deny\"\nunused-local = \"allow\"\n");
        write(tmp.path(), "src/main.lua", "local unused = 1\nreturn 0\n");
        // The rule-level `allow` wins over the tier-level `deny`.
        run(tmp.path(), false, Format::Human).expect("the allowed rule no longer fires");
    }

    #[test]
    fn a_global_declared_in_the_manifest_is_treated_as_intentional() {
        let tmp = project("\n[lint]\ncorrectness = \"deny\"\nglobals = [\"counter\"]\n");
        write(tmp.path(), "src/main.lua", "counter = 0\nreturn counter\n");
        run(tmp.path(), false, Format::Human)
            .expect("an allow-listed global does not fire global-write");
    }

    #[test]
    fn fix_rewrites_machine_applicable_findings_to_disk() {
        let tmp = project("");
        write(tmp.path(), "src/main.lua", "local unused = 1\nreturn 0\n");

        run(tmp.path(), true, Format::Human).expect("lint --fix succeeds");
        assert!(read(tmp.path(), "src/main.lua").contains("_unused"));
        // The rewritten file is clean on a second, non-fixing pass.
        run(tmp.path(), false, Format::Human).expect("the fixed file lints clean");
    }

    #[test]
    fn without_fix_nothing_is_written_back() {
        let tmp = project("");
        let original = "local unused = 1\nreturn 0\n";
        write(tmp.path(), "src/main.lua", original);
        run(tmp.path(), false, Format::Human).expect("lint passes");
        assert_eq!(read(tmp.path(), "src/main.lua"), original);
    }

    #[test]
    fn fix_never_rewrites_a_file_with_parse_errors() {
        let tmp = project("");
        let broken = "local unused = 1\nlocal = \n";
        write(tmp.path(), "src/main.lua", broken);

        // Parse errors are correctness-tier, so the command fails with the
        // lint summary (not, say, an io error from a half-written rewrite) —
        // and the file itself survives byte-for-byte. `local = ` trips the
        // parser twice: no name, then no expression.
        let error = run(tmp.path(), true, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "lint failed with 2 error(s)");
        assert_eq!(read(tmp.path(), "src/main.lua"), broken);
    }

    #[test]
    fn definition_files_are_linted_unlike_check_and_build() {
        let tmp = project("");
        // `lint` walks with `DefFiles::Include`: `*.d.lua` are walked too, so
        // a parse error in one is reported rather than skipped — and it is
        // the `.d.lua`'s own parse errors (no name, then no expression), from
        // the only file in the project.
        write(tmp.path(), "defs/broken.d.lua", "local = \n");
        let error = run(tmp.path(), false, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "lint failed with 2 error(s)");
    }

    #[test]
    fn the_build_output_directory_is_never_linted() {
        let tmp = project("\n[build]\nout = \"dist\"\n");
        write(tmp.path(), "dist/main.lua", "local = \n");
        run(tmp.path(), false, Format::Human).expect("emitted output is not project source");
    }

    #[test]
    fn a_manifest_less_directory_lints_as_lua_54_with_the_default_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = discover(tmp.path()).expect("manifest-less default");
        assert_eq!(project.root, tmp.path().to_path_buf());
        assert_eq!(project.dialect, Dialect::Lua54);
        assert!(project.out_dir.is_none());
        // The stdlib is still the known-globals baseline.
        assert!(project.known_globals.contains("print"));

        write(tmp.path(), "main.lua", "local x = 1\nprint(x)\n");
        run(tmp.path(), false, Format::Human).expect("lint passes");
    }

    #[test]
    fn a_malformed_manifest_fails_the_lint_rather_than_defaulting() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", "= = =\n");
        let error = run(tmp.path(), false, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    #[test]
    fn discover_reads_the_edition_and_out_dir_from_the_manifest() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            "[package]\nname = \"f\"\nversion = \"0.1.0\"\nedition = \"5.1\"\n\n[build]\nout = \"build\"\n",
        );
        let project = discover(tmp.path()).expect("discovers");
        assert_eq!(project.dialect, Dialect::Lua51);
        assert_eq!(project.out_dir, Some(tmp.path().join("build")));
    }

    // -- known globals (#103/#108) -----------------------------------------

    #[test]
    fn defs_declared_globals_join_the_known_globals_baseline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("\n[types]\ndefs = [\"mylib\"]\n"),
        );
        write(tmp.path(), "defs/mylib.d.lua", "---@meta\nmylib = {}\n");

        let project = discover(tmp.path()).expect("discovers");
        assert!(project.known_globals.contains("mylib"));
        assert!(project.known_globals.contains("print"));
    }

    #[test]
    fn a_directory_defs_package_contributes_its_globals_too() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("\n[types]\ndefs = [\"pack\"]\n"),
        );
        write(tmp.path(), "defs/pack/a.d.lua", "---@meta\nalpha = {}\n");
        write(tmp.path(), "defs/pack/b.d.lua", "---@meta\nbeta = {}\n");

        let project = discover(tmp.path()).expect("discovers");
        assert!(project.known_globals.contains("alpha"));
        assert!(project.known_globals.contains("beta"));
    }

    #[test]
    fn a_dependency_s_own_defs_globals_do_not_trip_undefined_global() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("\n[dependencies]\ngeo = { path = \"vendor/geo\" }\n"),
        );
        write(
            tmp.path(),
            "vendor/geo/luabox.toml",
            "[package]\nname = \"geo\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[types]\ndefs = [\"geo\"]\n",
        );
        write(
            tmp.path(),
            "vendor/geo/defs/geo.d.lua",
            "---@meta\ngeo = {}\n",
        );

        let project = discover(tmp.path()).expect("discovers");
        assert!(project.known_globals.contains("geo"));
    }

    #[test]
    fn an_unresolvable_defs_entry_silently_falls_back_to_the_stdlib_baseline() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("\n[types]\ndefs = [\"ghost\"]\n"),
        );
        // `luabox check` is the command that reports LB1002; lint just
        // narrows its baseline.
        let project = discover(tmp.path()).expect("discovers");
        assert!(project.known_globals.contains("print"));
        assert!(!project.known_globals.contains("ghost"));
    }

    #[test]
    fn known_globals_without_defs_is_exactly_the_dialect_stdlib() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let text = manifest("");
        let parsed = Manifest::parse(&text).expect("parses");
        assert_eq!(
            known_globals(Dialect::Lua54, tmp.path(), &parsed),
            *stdlib_defs(Dialect::Lua54).global_names()
        );
    }

    // -- test-harness globals ----------------------------------------------

    /// The busted-style call every harness-globals test uses.
    const BUSTED_CALL: &str =
        "describe(\"a thing\", function()\n  it(\"works\", function() end)\nend)\n";

    #[test]
    fn a_test_file_may_use_busted_style_harness_globals() {
        let tmp = project("\n[lint]\nundefined-global = \"deny\"\n");
        write(tmp.path(), "tests/spec.lua", BUSTED_CALL);
        run(tmp.path(), false, Format::Human).expect("harness globals are known inside tests/");
    }

    #[test]
    fn every_documented_harness_global_is_known_inside_a_test_file() {
        let tmp = project("\n[lint]\nundefined-global = \"deny\"\n");
        let calls = TEST_HARNESS_GLOBALS
            .iter()
            .fold(String::new(), |mut acc, g| {
                acc.push_str(g);
                acc.push_str("(\"x\", function() end)\n");
                acc
            });
        write(tmp.path(), "spec_test.lua", &calls);
        run(tmp.path(), false, Format::Human).expect("all harness globals are known");
    }

    #[test]
    fn a_non_test_file_does_not_get_the_harness_globals() {
        let tmp = project("\n[lint]\nundefined-global = \"deny\"\n");
        write(tmp.path(), "src/main.lua", BUSTED_CALL);
        // `describe` is not declared anywhere for ordinary sources.
        let error = run(tmp.path(), false, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.contains("lint failed with"), "{error}");
    }

    #[test]
    fn test_file_detection_covers_the_three_spec_conventions() {
        assert!(is_test_file("tests/spec.lua"));
        assert!(is_test_file("src/tests/deep/spec.lua"));
        assert!(is_test_file("src/thing_test.lua"));
        assert!(is_test_file("src/thing.test.lua"));
    }

    #[test]
    fn ordinary_sources_and_definition_files_are_not_test_files() {
        assert!(!is_test_file("src/main.lua"));
        assert!(!is_test_file("src/testing.lua"));
        assert!(!is_test_file("tests/README.md"));
        // A `*.d.lua` is an ambient surface, never a test — even under tests/.
        assert!(!is_test_file("tests/love.d.lua"));
        assert!(!is_test_file("src/thing_test.d.lua"));
    }

    #[test]
    fn test_file_detection_accepts_an_uppercase_lua_extension() {
        assert!(is_test_file("tests/Spec.LUA"));
    }

    #[test]
    fn a_bare_file_name_with_no_directory_is_handled() {
        assert!(is_test_file("spec_test.lua"));
        assert!(!is_test_file("main.lua"));
    }

    // -- config translation ------------------------------------------------

    #[test]
    fn the_manifest_lint_table_threads_globals_tiers_and_rules_into_the_lint_config() {
        let text =
            manifest("\n[lint]\nglobals = [\"vim\"]\nstyle = \"deny\"\nunused-local = \"allow\"\n");
        let parsed = Manifest::parse(&text).expect("parses");
        let (config, unknown) = LintConfig::from_manifest(&parsed.lint);
        assert!(unknown.is_empty(), "{unknown:?}");
        // `LintConfig` exposes no getters, so assert through behaviour: the
        // allowed rule is silent while another style rule now denies.
        let known = stdlib_defs(Dialect::Lua54).global_names().clone();
        let outcome = luabox_lint::lint_source(
            "src/main.lua",
            "local unused = 1\nreturn 0\n",
            Dialect::Lua54,
            &config,
            &known,
        );
        assert_eq!(outcome.error_count, 0, "{:?}", outcome.diagnostics);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    // -- unknown `[lint]` rule ids (LB1004, CC-M8) -------------------------

    /// The unknown rule ids `discover` finds for a manifest body.
    fn unknown_rules(extra: &str) -> Vec<UnknownRuleId> {
        let tmp = project(extra);
        discover(tmp.path())
            .expect("discovery succeeds")
            .unknown_lint_rules
    }

    #[test]
    fn a_typod_rule_id_becomes_an_lb1004_warning_naming_the_rule_it_meant() {
        let unknown = unknown_rules("\n[lint]\nunused-locl = \"allow\"\n");
        assert_eq!(unknown.len(), 1, "{unknown:?}");

        let diag = unknown_rule_diagnostic(&unknown[0]);
        assert_eq!(diag.code, UNKNOWN_LINT_RULE);
        assert_eq!(diag.code.to_string(), "LB1004");
        // A warning: the entry is inert, not fatal — the exit code is unchanged.
        assert_eq!(diag.severity, luabox_diag::Severity::Warning);
        assert_eq!(
            diag.message,
            "unknown lint rule id `unused-locl` in `[lint]`"
        );
        assert_eq!(
            diag.notes,
            vec![
                "did you mean `unused-local`?".to_owned(),
                "this `[lint]` entry has no effect".to_owned(),
                UNKNOWN_LINT_RULE_NOTE.to_owned(),
            ]
        );
    }

    #[test]
    fn a_typod_tier_name_is_nudged_back_at_the_tier() {
        // `pedantics` is not a tier, so the manifest records it as a rule-id
        // override; the nudge searches tier names too (audit note, CC-M8).
        let unknown = unknown_rules("\n[lint]\npedantics = \"warn\"\n");
        assert_eq!(unknown.len(), 1, "{unknown:?}");
        assert_eq!(
            unknown_rule_diagnostic(&unknown[0]).notes[0],
            "did you mean `pedantic`?"
        );
    }

    #[test]
    fn known_rule_ids_and_tiers_produce_no_config_warning() {
        assert!(
            unknown_rules(
                "\n[lint]\nglobals = [\"acme\"]\npedantic = \"warn\"\nunused-local = \"allow\"\n"
            )
            .is_empty()
        );
        assert!(unknown_rules("").is_empty());
    }

    #[test]
    fn a_typod_rule_id_warns_without_failing_the_command() {
        let tmp = project("\n[lint]\nunused-locl = \"allow\"\n");
        write(tmp.path(), "src/main.lua", "return 0\n");
        // Exit code unchanged: `LB1004` is a warning (SPEC.md §9 — only
        // deny-tier findings, parse errors and malformed ignores fail).
        run(tmp.path(), false, Format::Human).expect("a config warning does not fail lint");
    }

    #[test]
    fn a_manifest_less_project_has_no_lint_config_to_complain_about() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(
            discover(tmp.path())
                .expect("discovery succeeds")
                .unknown_lint_rules
                .is_empty()
        );
    }

    #[test]
    fn fix_converges_within_the_pass_budget() {
        let tmp = project("");
        // Several independent fixable findings in one file: `--fix` re-lints
        // until nothing is left to apply, well inside MAX_FIX_PASSES.
        write(
            tmp.path(),
            "src/main.lua",
            "local a = 1\nlocal b = 2\nlocal c = 3\nreturn 0\n",
        );
        run(tmp.path(), true, Format::Human).expect("lint --fix succeeds");
        let fixed = read(tmp.path(), "src/main.lua");
        assert!(fixed.contains("_a"), "{fixed}");
        assert!(fixed.contains("_b"), "{fixed}");
        assert!(fixed.contains("_c"), "{fixed}");
    }

    #[test]
    fn findings_are_reported_in_file_then_offset_order() {
        let tmp = project("");
        write(tmp.path(), "src/b.lua", "local unused_b = 1\nreturn 0\n");
        write(tmp.path(), "src/a.lua", "local unused_a = 1\nreturn 0\n");
        // Ordering is a rendering concern; the command still succeeds and the
        // sort must not panic on labelless diagnostics.
        run(tmp.path(), false, Format::Human).expect("lint passes");
    }
}
