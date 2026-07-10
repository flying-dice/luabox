//! `luabox check [--target <t>] [--format <f>] [--watch]` — the CI-grade
//! standalone typecheck (SPEC.md §3, §4, §14).
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
//!    `[types] strict = true` → strict (errors), otherwise warn — plus
//!    `.lb` shape bindings (`---@use`/`---@struct`/`---@impl`, SHAPES.md),
//!    whose `LB2xxx` rules are hard errors at every strictness.
//!
//! `.lb` shape modules are checked too: parse errors (including `LB2010`
//! body rejection) and shape-level diagnostics (`LB2005`/`LB2007`) carry
//! the `.lb` file and spans. Shape resolution uses the file's directory
//! (sibling tier) plus the manifest's `[types] shape-paths`; parsed
//! modules are cached in a store shared across the rayon workers.
//!
//! Output goes to stdout in the chosen format; a `check: N errors, M
//! warnings in K files` summary goes to stderr. The exit code is nonzero
//! iff any Error-severity diagnostic was produced — warnings never fail
//! the command.
//!
//! `--watch` (SPEC.md §4) turns this into a long-running rerun-on-change
//! loop instead of a one-shot check — see `crate::watch` for the debounce
//! and filtering rules.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span, render};
use luabox_resolve::manifest::{Dependency, Manifest};
use luabox_syntax::{Dialect, lua};
use luabox_types::{
    Ambient, DepShapeExport, ShapeOptions, ShapeStore, Strictness, combined_defs, stdlib_defs,
};
use rayon::prelude::*;

/// Execute `luabox check` from `cwd`. With `watch`, the check reruns on
/// every debounced, filtered filesystem change under the project root
/// (`crate::watch`) until interrupted (Ctrl-C); a failing rerun is
/// reported but does not stop the watcher, so in watch mode this function
/// only returns on setup failure (e.g. the watch root can't be observed).
/// Without `watch` it runs once and its `Result` becomes the process exit
/// code, as before.
pub fn run(cwd: &Path, target: Option<&str>, format: &str, watch: bool) -> anyhow::Result<()> {
    if watch {
        // Discover once up front purely to get a root/out-dir to watch;
        // `run_once` rediscovers the project fresh on every rerun, so a
        // manifest edit (edition, strictness, shape paths) takes effect
        // on the very next rerun without any extra plumbing here.
        let project = discover(cwd)?;
        let cwd = cwd.to_path_buf();
        let target = target.map(str::to_owned);
        let format = format.to_owned();
        return crate::watch::run(&project.root, project.out_dir.as_deref(), move || {
            run_once(&cwd, target.as_deref(), &format, None)
        });
    }
    run_once(cwd, target, format, None)
}

/// The single-pass body of `luabox check`: discover the project, typecheck
/// every file, and translate the diagnostics into an exit code. Shared by
/// one-shot `run`, each rerun of `run` in `--watch` mode, and the
/// check-first gate of `luabox build` (`crate::build_cmd`), which passes
/// its chosen out directory as `skip_out` so previously emitted output is
/// never checked as project source even under a custom `--out`.
pub(crate) fn run_once(
    cwd: &Path,
    target: Option<&str>,
    format: &str,
    skip_out: Option<&Path>,
) -> anyhow::Result<()> {
    let format = parse_format(format)?;
    let mut project = discover(cwd)?;
    if let Some(out) = skip_out {
        project.out_dir = Some(out.to_path_buf());
    }

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

    let (lua_files, lb_files) = collect_files(&project)?;
    // Definition packages (SPEC.md §3): the dialect stdlib layer, plus any
    // project-local `[types] defs` resolved from `<root>/defs/`. Built once
    // and shared by reference across the rayon workers; the stdlib-only case
    // reuses a process-lifetime cache (perf gate: paid once).
    let (def_sources, def_diags) = resolve_project_defs(&project.root, &project.defs);
    let ambient_owned: Option<Ambient> = if project.defs.is_empty() {
        None
    } else {
        Some(combined_defs(project.dialect, &def_sources))
    };
    let ambient: &Ambient = ambient_owned
        .as_ref()
        .unwrap_or_else(|| stdlib_defs(project.dialect));

    // Shape modules are parsed once and cached across workers.
    let store = ShapeStore::new(project.root.clone());
    // SPEC.md §16: rayon per-module. Files are independent (cross-file
    // resolution is P1); collecting per-file Vecs preserves source order.
    let per_file: Vec<anyhow::Result<Vec<Diagnostic>>> = lua_files
        .par_iter()
        .map(|path| {
            let rel = display_rel(path, &project.root);
            let source =
                fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
            let mut diags = Vec::new();
            check_one(
                &source,
                &rel,
                path,
                &project,
                target_dialect,
                &store,
                ambient,
                &mut diags,
            );
            Ok(diags)
        })
        .collect();
    // `.lb` shape files get their own pass: parse errors (LB2010, LB0001)
    // plus shape-level diagnostics attributed to the declaring file.
    let per_lb: Vec<anyhow::Result<Vec<Diagnostic>>> = lb_files
        .par_iter()
        .map(|path| {
            let rel = display_rel(path, &project.root);
            let source =
                fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
            Ok(store.check_lb_file(path, &source, &project.shape_paths, &project.dependencies))
        })
        .collect();
    let mut diags: Vec<Diagnostic> = def_diags;
    for result in per_file.into_iter().chain(per_lb) {
        diags.extend(result?);
    }

    finish(
        &diags,
        format,
        &project.root,
        lua_files.len() + lb_files.len(),
    )
}

/// All three passes for one file.
#[allow(clippy::too_many_arguments)]
fn check_one(
    source: &str,
    rel: &str,
    path: &Path,
    project: &Project,
    target: Option<Dialect>,
    store: &ShapeStore,
    ambient: &Ambient,
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

    // 3. Types + shape bindings (SHAPES.md §4–§6). Shape resolution needs
    // the file's own directory (sibling tier) plus the manifest's
    // `[types] shape-paths`.
    let file_dir = path.parent().unwrap_or(&project.root);
    let opts = ShapeOptions {
        store,
        file_dir,
        shape_paths: &project.shape_paths,
        dependencies: &project.dependencies,
    };
    diags.extend(luabox_types::check_file_shaped(
        &parse,
        rel,
        project.strictness,
        Some(&opts),
        Some(ambient),
    ));
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

pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) dialect: Dialect,
    strictness: Strictness,
    pub(crate) out_dir: Option<PathBuf>,
    /// `[build] target` — the dialect you ship (SPEC.md §2.1, §5); defaults
    /// to the edition. Consumed by `crate::build_cmd`.
    pub(crate) build_target: Dialect,
    /// `[types] shape-paths`, absolute, in manifest order (SHAPES.md §6).
    shape_paths: Vec<PathBuf>,
    /// Dependencies that export shape modules (SHAPES.md §6, tier 3), each
    /// with its package root and its own `[types] shapes`/`shape-paths`.
    dependencies: Vec<DepShapeExport>,
    /// `[types] defs`, ambient definition packages resolved from the
    /// project-local `defs/` directory (SPEC.md §3, §5).
    defs: Vec<String>,
}

/// Find the project: nearest `luabox.toml` walking up from `cwd`
/// (cargo-style), or a manifest-less default rooted at `cwd` (Lua 5.4,
/// warn mode — least surprise).
pub(crate) fn discover(cwd: &Path) -> anyhow::Result<Project> {
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
            let Some(build_target) = Dialect::from_manifest_id(&manifest.build.target) else {
                bail!(
                    "unknown build target `{}` in `{}` (see `luabox explain LB1001`)",
                    manifest.build.target,
                    manifest_path.display()
                );
            };
            return Ok(Project {
                root: current.to_path_buf(),
                dialect,
                strictness: Strictness::from_manifest_flag(manifest.types.strict),
                out_dir: Some(current.join(&manifest.build.out)),
                build_target,
                shape_paths: manifest
                    .types
                    .shape_paths
                    .iter()
                    .map(|p| current.join(p))
                    .collect(),
                dependencies: resolve_dep_shape_exports(current, &manifest),
                defs: manifest.types.defs.clone(),
            });
        }
        dir = current.parent();
    }
    Ok(Project {
        root: cwd.to_path_buf(),
        dialect: Dialect::Lua54,
        strictness: Strictness::Warn,
        out_dir: None,
        build_target: Dialect::Lua54,
        shape_paths: Vec::new(),
        dependencies: Vec::new(),
        defs: Vec::new(),
    })
}

/// Build the dependency shape-export table for resolution tier 3 (SHAPES.md
/// §6): for each declared dependency, locate its package root (a path
/// dependency in place at its `path`, every other kind under
/// `lua_modules/<name>/`) and read that dependency's *own* `[types] shapes`
/// and `[types] shape-paths` from its manifest. Only dependencies that
/// actually export shape modules contribute an entry; a dependency with no
/// manifest on disk (uninstalled, or a source kind whose root cannot be
/// located here) or an empty `[types] shapes` simply exports nothing and is
/// skipped. `.lb` sources are never parsed here — only the manifest is read
/// (SHAPES.md §7: distribution ships `.lb` opaquely).
fn resolve_dep_shape_exports(root: &Path, manifest: &Manifest) -> Vec<DepShapeExport> {
    let mut exports = Vec::new();
    for (name, dep) in manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
    {
        let dep_root = match dep {
            Dependency::Path(p) => root.join(p.path.replace('\\', "/")),
            _ => root.join("lua_modules").join(name),
        };
        let Ok(text) = fs::read_to_string(dep_root.join("luabox.toml")) else {
            continue;
        };
        let Ok(dep_manifest) = Manifest::parse(&text) else {
            continue;
        };
        if dep_manifest.types.shapes.is_empty() {
            continue;
        }
        exports.push(DepShapeExport {
            name: name.clone(),
            shape_paths: dep_manifest
                .types
                .shape_paths
                .iter()
                .map(|p| dep_root.join(p))
                .collect(),
            exported: dep_manifest.types.shapes.clone(),
            root: dep_root,
        });
    }
    exports
}

/// Resolve `[types] defs` entries against the project-local `defs/`
/// directory: each name loads `defs/<name>.d.lua` or every `*.d.lua` under
/// `defs/<name>/` (SPEC.md §3 — registry-distributed packages are P2+).
/// Returns the concatenated sources plus a diagnostic per unresolvable entry.
fn resolve_project_defs(root: &Path, names: &[String]) -> (Vec<String>, Vec<Diagnostic>) {
    let mut sources = Vec::new();
    let mut diags = Vec::new();
    let defs_dir = root.join("defs");
    for name in names {
        let single = defs_dir.join(format!("{name}.d.lua"));
        let dir = defs_dir.join(name);
        let mut found = false;
        if single.is_file()
            && let Ok(text) = fs::read_to_string(&single)
        {
            sources.push(text);
            found = true;
        }
        if dir.is_dir() {
            let mut files = Vec::new();
            collect_d_lua(&dir, &mut files);
            files.sort();
            for file in files {
                if let Ok(text) = fs::read_to_string(&file) {
                    sources.push(text);
                    found = true;
                }
            }
        }
        if !found {
            diags.push(
                Diagnostic::error(
                    code(1002),
                    format!(
                        "cannot resolve definition package `{name}` from `[types] defs`"
                    ),
                )
                .with_note(format!(
                    "expected `defs/{name}.d.lua` or a `defs/{name}/` directory of `*.d.lua` files under the project root"
                )),
            );
        }
    }
    (sources, diags)
}

/// Collect every `*.d.lua` file under `dir`, recursively.
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

/// All `*.lua` and `*.lb` files under the project root, deterministic
/// order, skipping dot-directories and the build output directory.
pub(crate) fn collect_files(project: &Project) -> anyhow::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut lua = Vec::new();
    let mut lb = Vec::new();
    walk(&project.root, project, &mut lua, &mut lb)?;
    Ok((lua, lb))
}

fn walk(
    dir: &Path,
    project: &Project,
    lua: &mut Vec<PathBuf>,
    lb: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
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
                walk(&path, project, lua, lb)?;
            }
        } else if !hidden {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            match path.extension().and_then(|e| e.to_str()) {
                // `*.d.lua` are `---@meta` definition files (ambient type
                // surfaces), never checked as project source.
                Some("lua") if !name.ends_with(".d.lua") => lua.push(path),
                Some("lb") => lb.push(path),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Root-relative path with forward slashes — stable output across platforms.
pub(crate) fn display_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}
