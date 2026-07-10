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

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Diagnostic, Format, Severity, render};
use luabox_lint::{LintConfig, apply_fixes, lint_source};
use luabox_resolve::manifest::{Lint, LintLevel, Manifest};
use luabox_syntax::Dialect;
use rayon::prelude::*;

/// The most fix passes to run per file before giving up on convergence.
const MAX_FIX_PASSES: usize = 8;

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

    let mut source = original.clone();
    let mut outcome = lint_source(&rel, &source, project.dialect, &project.lint);

    // Never rewrite a file with parse errors.
    if fix && !outcome.had_parse_errors {
        let mut passes = 0;
        while !outcome.fixes.is_empty() && passes < MAX_FIX_PASSES {
            source = apply_fixes(&source, &outcome.fixes);
            outcome = lint_source(&rel, &source, project.dialect, &project.lint);
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
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`, or a
/// manifest-less default rooted at `cwd` (Lua 5.4, empty lint config).
fn discover(cwd: &Path) -> anyhow::Result<Project> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        let manifest_path = current.join("luabox.toml");
        if manifest_path.is_file() {
            let text = fs::read_to_string(&manifest_path)
                .with_context(|| format!("cannot read `{}`", manifest_path.display()))?;
            let manifest = Manifest::parse(&text).map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                anyhow::anyhow!("invalid `{}`:\n{rendered}", manifest_path.display())
            })?;
            let Some(dialect) = Dialect::from_manifest_id(&manifest.package.edition) else {
                bail!(
                    "unknown edition `{}` in `{}` (see `luabox explain LB1001`)",
                    manifest.package.edition,
                    manifest_path.display()
                );
            };
            return Ok(Project {
                root: current.to_path_buf(),
                dialect,
                out_dir: Some(current.join(&manifest.build.out)),
                lint: build_config(&manifest.lint),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        dialect: Dialect::Lua54,
        out_dir: None,
        lint: LintConfig::new(),
    })
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
