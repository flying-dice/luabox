//! `luabox check [--target <t>] [--format <f>]` — the CI-grade standalone
//! typecheck (SPEC.md §3, §4, §14).
//!
//! Per `.lua` file, three passes over one parse:
//!
//! 1. **Parse errors** → `LB0001` (the parser is error-resilient; later
//!    passes still run on the recovered tree).
//! 2. **Dialect legality** against the project `edition` — and, with
//!    `--target`, against the ship target too (that is what `--target`
//!    means before lowering exists: "would this source be legal there?").
//!    Duplicate findings (same code, same range) are reported once.
//! 3. **Typecheck** (annotation-driven, per-file environment; cross-file
//!    `require` resolution is P1) at the manifest's strictness:
//!    `[types] strict = true` → strict (errors), otherwise warn.
//!
//! Output goes to stdout in the chosen format; a `check: N errors, M
//! warnings in K files` summary goes to stderr. The exit code is nonzero
//! iff any Error-severity diagnostic was produced — warnings never fail
//! the command.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span, render};
use luabox_resolve::manifest::Manifest;
use luabox_syntax::{Dialect, lua};
use luabox_types::Strictness;
use rayon::prelude::*;

/// Execute `luabox check` from `cwd`.
pub fn run(cwd: &Path, target: Option<&str>, format: &str) -> anyhow::Result<()> {
    let format = parse_format(format)?;
    let project = discover(cwd)?;

    // Validate --target up front: a bad value is itself a diagnostic.
    let mut target_dialect = None;
    if let Some(id) = target {
        let Some(dialect) = Dialect::from_manifest_id(id) else {
            let diag = Diagnostic::error(
                code(1001),
                format!("unknown target `{id}`; expected one of: 5.1, 5.2, 5.3, 5.4, luajit"),
            )
            .with_note("run `luabox explain LB1001` for the full list of editions");
            return finish(&[diag], format, &project.root, 0);
        };
        target_dialect = Some(dialect);
    }

    let files = collect_lua_files(&project)?;
    // SPEC.md §16: rayon per-module. Files are independent (cross-file
    // resolution is P1); collecting per-file Vecs preserves source order.
    let per_file: Vec<anyhow::Result<Vec<Diagnostic>>> = files
        .par_iter()
        .map(|path| {
            let rel = display_rel(path, &project.root);
            let source =
                fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
            let mut diags = Vec::new();
            check_one(&source, &rel, &project, target_dialect, &mut diags);
            Ok(diags)
        })
        .collect();
    let mut diags: Vec<Diagnostic> = Vec::new();
    for result in per_file {
        diags.extend(result?);
    }

    finish(&diags, format, &project.root, files.len())
}

/// All three passes for one file.
fn check_one(
    source: &str,
    rel: &str,
    project: &Project,
    target: Option<Dialect>,
    diags: &mut Vec<Diagnostic>,
) {
    let parse = lua::parse(source, project.dialect);

    // 1. Parse errors.
    for err in parse.errors() {
        diags.push(
            Diagnostic::error(code(1), err.message.clone()).with_label(Label::primary(
                Span::new(rel, to_range(err.range)),
                "syntax error here",
            )),
        );
    }

    // 2. Dialect legality: edition, then ship target (deduplicated — the
    // same construct may be illegal in both).
    let mut passes = vec![project.dialect];
    if let Some(target) = target
        && target != project.dialect
    {
        passes.push(target);
    }
    let mut seen: HashSet<(&'static str, u32, u32)> = HashSet::new();
    for dialect in passes {
        for err in lua::validate::validate(&parse, dialect) {
            let key = (err.code, err.range.start().into(), err.range.end().into());
            if !seen.insert(key) {
                continue;
            }
            let parsed: Code = err
                .code
                .parse()
                .unwrap_or_else(|_| unreachable!("validator emits registered codes"));
            diags.push(
                Diagnostic::error(parsed, err.message).with_label(Label::primary(
                    Span::new(rel, to_range(err.range)),
                    "not legal in this edition",
                )),
            );
        }
    }

    // 3. Types.
    diags.extend(luabox_types::check_file(&parse, rel, project.strictness));
}

/// Render, summarize, and translate error count into the exit code.
fn finish(
    diags: &[Diagnostic],
    format: Format,
    root: &Path,
    file_count: usize,
) -> anyhow::Result<()> {
    let root = root.to_path_buf();
    let lookup = move |file: &str| fs::read_to_string(root.join(file)).ok();
    let output = render(diags, format, &lookup);
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
    eprintln!("check: {errors} errors, {warnings} warnings in {file_count} files");
    if errors > 0 {
        bail!("check failed with {errors} error(s)");
    }
    Ok(())
}

fn code(number: u16) -> Code {
    Code::new(number)
}

fn to_range(range: rowan::TextRange) -> std::ops::Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

fn parse_format(format: &str) -> anyhow::Result<Format> {
    Ok(match format {
        "human" => Format::Human,
        "json" => Format::Json,
        "sarif" => Format::Sarif,
        "github" => Format::GithubActions,
        "gitlab" => Format::GitlabCodeQuality,
        other => bail!("unknown format `{other}`; expected human, json, sarif, github, or gitlab"),
    })
}

struct Project {
    root: PathBuf,
    dialect: Dialect,
    strictness: Strictness,
    out_dir: Option<PathBuf>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`
/// (cargo-style), or a manifest-less default rooted at `cwd` (Lua 5.4,
/// warn mode — least surprise).
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
                strictness: Strictness::from_manifest_flag(manifest.types.strict),
                out_dir: Some(current.join(&manifest.build.out)),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        dialect: Dialect::Lua54,
        strictness: Strictness::Warn,
        out_dir: None,
    })
}

/// All `*.lua` files under the project root, deterministic order, skipping
/// dot-directories and the build output directory. (`.lb` shape modules
/// are checked by the P1 shape checker, not here.)
fn collect_lua_files(project: &Project) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    walk(&project.root, project, &mut files)?;
    Ok(files)
}

fn walk(dir: &Path, project: &Project, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
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
                walk(&path, project, files)?;
            }
        } else if !hidden && path.extension().and_then(|e| e.to_str()) == Some("lua") {
            files.push(path);
        }
    }
    Ok(())
}

/// Root-relative path with forward slashes — stable output across platforms.
fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}
