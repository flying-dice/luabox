//! `luabox lint [--fix]` — the clippy analog (SPEC.md §9).
//!
//! Discovers the project (nearest `luabox.toml`, cargo-style), lints every
//! `.lua` file in parallel over the shared parse/HIR/type machinery, and
//! renders findings in the human format. Tiers and per-rule levels come from
//! `[lint]` in the manifest; the exit code is nonzero iff any deny-tier finding
//! (or a parse error / malformed suppression) was produced — warnings never
//! fail the command.
//!
//! `--fix` applies machine-applicable fixes to disk (innermost-first,
//! non-overlapping), re-linting each file until it converges. A file with parse
//! errors is never rewritten. A `lint: N errors, M warnings in K files` summary
//! goes to stderr.
//!
//! **Known globals** (ticket #103, `undefined-global`): the dialect stdlib
//! plus any `[types] defs` packages, resolved from `defs/` the same way
//! `luabox check` builds its `Ambient` layer (see
//! `luabox-cli::check_cmd::resolve_project_defs`, duplicated here in
//! miniature — the two commands don't share a `Project` type). Test files —
//! anything `luabox test` would also discover (SPEC.md §11:
//! `*_test.lua`/`*.test.lua`/anything under `tests/`) — additionally see the
//! embedded test harness's own globals (`describe`, `it`, `before_each`,
//! `after_each`, `test`), since `crates/luabox-test/src/harness.lua` genuinely
//! injects them as globals at run time before `dofile`-ing the test file.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Diagnostic, Format, Severity, render};
use luabox_lint::{LintConfig, apply_fixes, lint_source};
use luabox_resolve::manifest::{Lint, LintLevel, Manifest};
use luabox_syntax::Dialect;
use luabox_types::{Ambient, combined_defs, stdlib_defs};
use rayon::prelude::*;

/// The most fix passes to run per file before giving up on convergence.
const MAX_FIX_PASSES: usize = 8;

/// Globals the embedded test harness (`crates/luabox-test/src/harness.lua`)
/// genuinely injects before `dofile`-ing a test file: the busted-compatible
/// `describe`/`it`/`before_each`/`after_each` plus the native flat `test`.
/// `assert` is not listed — it's already stdlib, the harness only replaces
/// its value, not its name.
const TEST_HARNESS_GLOBALS: [&str; 5] = ["describe", "it", "before_each", "after_each", "test"];

/// Execute `luabox lint` from `cwd`.
pub fn run(cwd: &Path, fix: bool) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    let files = collect_files(&project)?;

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

    finish(&diags, &project.root, files.len(), fixed_files, fix)
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
    // widened with the embedded test harness's own globals for files
    // `luabox test` would itself discover — those genuinely are globals at
    // run time (`harness.lua` assigns `describe`/`it`/... before `dofile`-ing
    // the test file), just never declared in a `.d.lua` defs package.
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
        fs::write(path, &source).with_context(|| format!("cannot write `{rel}`"))?;
    }

    Ok(FileResult {
        diagnostics: outcome.diagnostics,
        was_fixed,
    })
}

/// Render, summarize, and translate the error count into the exit code.
fn finish(
    diags: &[Diagnostic],
    root: &Path,
    file_count: usize,
    fixed_files: usize,
    fix: bool,
) -> anyhow::Result<()> {
    let root_buf = root.to_path_buf();
    let lookup = move |file: &str| fs::read_to_string(root_buf.join(file)).ok();
    let output = render(diags, Format::Human, &lookup);
    if !output.is_empty() {
        println!("{output}");
    }

    let errors = diags
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diags
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .count();
    if fix {
        eprintln!(
            "lint: {errors} errors, {warnings} warnings in {file_count} files ({fixed_files} fixed)"
        );
    } else {
        eprintln!("lint: {errors} errors, {warnings} warnings in {file_count} files");
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
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (Lua 5.4, empty lint config).
fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let Some((root, manifest)) = crate::project::discover_manifest(cwd)? else {
        return Ok(Project {
            root: cwd.to_path_buf(),
            dialect: Dialect::Lua54,
            out_dir: None,
            lint: LintConfig::new(),
            known_globals: stdlib_defs(Dialect::Lua54).global_names().clone(),
        });
    };
    let Some(dialect) = Dialect::from_manifest_id(&manifest.package.edition) else {
        bail!(
            "unknown edition `{}` in `{}` (see `luabox explain LB1001`)",
            manifest.package.edition,
            root.join("luabox.toml").display()
        );
    };
    let known_globals = known_globals(dialect, &root, &manifest);
    Ok(Project {
        out_dir: Some(root.join(&manifest.build.out)),
        dialect,
        lint: build_config(&manifest.lint),
        known_globals,
        root,
    })
}

/// The `undefined-global` known-globals baseline for one project: the
/// dialect stdlib, widened with any `[types] defs` packages resolved from
/// `<root>/defs/` (SPEC.md §3) *and* every direct dependency's own `[types]
/// defs` (#108, the luals `workspace.library` model — a dependency's ambient
/// globals must not spuriously trip the consumer's `undefined-global`).
/// Mirrors `check_cmd::resolve_project_defs` in miniature (same resolution
/// rules), and reuses `check_cmd::resolve_dep_defs` for the dependency side,
/// since the two commands don't share a `Project` type. A project-defs entry
/// that fails to resolve is silently skipped here — `luabox check` is the
/// command that reports `LB1002`; `lint` falls back to the stdlib-only
/// baseline for that entry.
fn known_globals(dialect: Dialect, root: &Path, manifest: &Manifest) -> HashSet<String> {
    let mut sources = Vec::new();
    let defs_dir = root.join("defs");
    for name in &manifest.types.defs {
        let single = defs_dir.join(format!("{name}.d.lua"));
        if single.is_file()
            && let Ok(text) = fs::read_to_string(&single)
        {
            sources.push(text);
        }
        let dir = defs_dir.join(name);
        if dir.is_dir() {
            let mut files = Vec::new();
            collect_d_lua(&dir, &mut files);
            files.sort();
            for file in files {
                if let Ok(text) = fs::read_to_string(&file) {
                    sources.push(text);
                }
            }
        }
    }
    for dep_def in crate::check_cmd::resolve_dep_defs(root, manifest) {
        sources.push(dep_def.text);
    }
    if sources.is_empty() {
        return stdlib_defs(dialect).global_names().clone();
    }
    let ambient: Ambient = combined_defs(dialect, &sources);
    ambient.global_names().clone()
}

/// Collect every `*.d.lua` file under `dir`, recursively (mirrors
/// `check_cmd`'s helper of the same shape).
fn collect_d_lua(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_d_lua(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("lua")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".d.lua"))
        {
            out.push(path);
        }
    }
}

/// Translate the manifest `[lint]` table into a [`LintConfig`].
fn build_config(lint: &Lint) -> LintConfig {
    let mut config = LintConfig::new();
    for name in &lint.globals {
        config.allow_global(name.clone());
    }
    for (tier, level) in &lint.tiers {
        config.set_tier(tier, level_keyword(*level));
    }
    for (rule, level) in &lint.rules {
        config.set_rule(rule, level_keyword(*level));
    }
    config
}

fn level_keyword(level: LintLevel) -> &'static str {
    match level {
        LintLevel::Allow => "allow",
        LintLevel::Warn => "warn",
        LintLevel::Deny => "deny",
    }
}

/// All `*.lua` files under the project root, deterministic order, skipping
/// dot-directories and the build output directory.
fn collect_files(project: &Project) -> anyhow::Result<Vec<PathBuf>> {
    let mut lua = Vec::new();
    walk(&project.root, project, &mut lua)?;
    Ok(lua)
}

fn walk(dir: &Path, project: &Project, lua: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("cannot read directory `{}`", dir.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if path.is_dir() {
            let is_out = project.out_dir.as_deref() == Some(path.as_path());
            if !hidden && !is_out {
                walk(&path, project, lua)?;
            }
        } else if !hidden && path.extension().and_then(|e| e.to_str()) == Some("lua") {
            lua.push(path);
        }
    }
    Ok(())
}

/// Root-relative path with forward slashes — stable output across platforms.
fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Whether `rel` (a root-relative, forward-slash path) is a file `luabox
/// test` would itself discover (SPEC.md §11: `*_test.lua`, `*.test.lua`, or
/// anything under a `tests/` directory) — mirrors
/// `luabox_test::discovery::is_test_file` exactly, just fed a path string
/// instead of a directory walk, since `lint`'s own file collection already
/// has one.
fn is_test_file(rel: &str) -> bool {
    let mut segments = rel.split('/');
    let name = segments.next_back().unwrap_or(rel);
    let in_tests = segments.any(|seg| seg == "tests");
    luabox_test::discovery::is_test_file(name, in_tests)
}
