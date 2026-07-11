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
//! 3. **Typecheck** (annotation-driven, against the ambient definition
//!    layer, with each file's cross-file `require` exports in reach — #85)
//!    at the manifest's strictness: `[types] strict = true` → strict
//!    (errors), otherwise warn.
//!
//! Output goes to stdout in the chosen format; a `check: N errors, M
//! warnings in K files` summary goes to stderr. The exit code is nonzero
//! iff any Error-severity diagnostic was produced — warnings never fail
//! the command.
//!
//! `--watch` (SPEC.md §4) turns this into a long-running rerun-on-change
//! loop instead of a one-shot check — see `crate::watch` for the debounce
//! and filtering rules.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span, render};
use luabox_resolve::manifest::{Dependency, Manifest};
use luabox_syntax::{Dialect, lua};
use luabox_types::ty::Ty;
use luabox_types::{Ambient, DefFile, Strictness, combined_defs_checked, stdlib_defs};
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
        // manifest edit (edition, strictness) takes effect on the very
        // next rerun without any extra plumbing here.
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

    let lua_files = collect_files(&project)?;
    // Definition packages (SPEC.md §3): the dialect stdlib layer, plus any
    // project-local `[types] defs` resolved from `<root>/defs/`, plus each
    // direct dependency's own `[types] defs` — the luals `workspace.library`
    // model (#108): a dependency's def files join the consumer's ambient
    // scope. Winner-first order (project defs, then dependencies
    // alphabetically); `combined_defs_checked` reports cross-package class
    // collisions (`LB0307`). Built once and shared by reference across the
    // rayon workers; the no-defs case reuses a process-lifetime cache.
    let (mut all_defs, mut def_diags) = resolve_project_defs(&project.root, &project.defs);
    all_defs.extend(project.dep_defs.iter().cloned());
    let ambient_owned: Option<Ambient> = if all_defs.is_empty() {
        None
    } else {
        let (ambient, collisions) = combined_defs_checked(project.dialect, &all_defs);
        def_diags.extend(collisions);
        Some(ambient)
    };
    let ambient: &Ambient = ambient_owned
        .as_ref()
        .unwrap_or_else(|| stdlib_defs(project.dialect));

    // Cross-file pre-pass (#85): reify every project file's surface up
    // front — its `require`-export type (keyed by canonical path) plus its
    // workspace-global `---@class`/`---@enum` declarations (luals parity:
    // classes declared in any checked file, including their
    // `function Class:method` member attachments, are nameable and
    // resolvable from every other file). Exports are check-mode
    // (annotations authoritative, no call-site seeding) and a module's own
    // requires are left unresolved, so the registry is acyclic and
    // cycle-tolerant. Resolution reuses the bundler's exact `require`
    // path-mapping ([`luabox_bundle::resolve_module`]).
    let surfaces: Vec<(PathBuf, String, luabox_types::ModuleSurface)> = lua_files
        .par_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(path).ok()?;
            let parse = lua::parse(&source, project.dialect);
            let rel = display_rel(path, &project.root);
            Some((
                canonical(path),
                rel.clone(),
                luabox_types::module_surface(&parse, &rel, Some(ambient)),
            ))
        })
        .collect();
    let exports: HashMap<PathBuf, Ty> = surfaces
        .iter()
        .filter_map(|(path, _rel, surface)| Some((path.clone(), surface.export.clone()?)))
        .collect();
    // Duplicate `---@alias` across project files / `[types] defs` (luals
    // `duplicate-doc-alias`, LB0310, #113): a project-assembly finding — like
    // the LB0307 class collisions above — computed over the whole source set,
    // never in the per-file check, so a file checked standalone and in-project
    // stays consistent. Winner order matches `with_project_types`.
    def_diags.extend(luabox_types::alias_collisions(
        &all_defs,
        &surfaces
            .iter()
            .map(|(_, rel, s)| (rel.clone(), &s.types))
            .collect::<Vec<_>>(),
    ));
    // The project-wide ambient: defs + every file's workspace-global
    // classes/enums, merged (defs win same-name member collisions; luals
    // merges duplicate class declarations' fields rather than dropping).
    let ambient = ambient.with_project_types(surfaces.iter().map(|(_, _, s)| &s.types));
    let ambient = &ambient;

    // SPEC.md §16: rayon per-module. Each file is checked against the
    // shared project ambient plus its own resolved `require` exports;
    // collecting per-file Vecs preserves source order.
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
                &project,
                target_dialect,
                ambient,
                &exports,
                &mut diags,
            );
            Ok(diags)
        })
        .collect();
    let mut diags: Vec<Diagnostic> = def_diags;
    for result in per_file {
        diags.extend(result?);
    }

    finish(&diags, format, &project.root, lua_files.len())
}

/// All three passes for one file.
#[allow(
    clippy::too_many_arguments,
    reason = "the check pipeline threads its shared context"
)]
fn check_one(
    source: &str,
    rel: &str,
    project: &Project,
    target: Option<Dialect>,
    ambient: &Ambient,
    exports: &HashMap<PathBuf, Ty>,
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

    // 3. Types against the ambient definition-package layer (SPEC.md §3),
    // with this file's resolved `require` exports in reach (#85).
    let requires = resolve_requires(&parse, &project.root, exports);
    diags.extend(luabox_types::check_file_with_requires(
        &parse,
        rel,
        project.strictness,
        Some(ambient),
        &requires,
    ));
}

/// Map each static `require("mod")` in `parse` to the export type of the
/// project file it resolves to, using the bundler's `require` path-mapping.
/// Requires that resolve outside the project (dependencies, external
/// runtime modules) have no entry in `exports` and are simply skipped —
/// their types come from ambient `[types] defs` (#108), not the module
/// return value.
fn resolve_requires(
    parse: &lua::Parse,
    root: &Path,
    exports: &HashMap<PathBuf, Ty>,
) -> HashMap<String, Ty> {
    let mut requires = HashMap::new();
    for module in luabox_types::module_requires(parse) {
        if let Some(target) = luabox_bundle::resolve_module(root, &module)
            && let Some(ty) = exports.get(&target)
        {
            requires.insert(module, ty.clone());
        }
    }
    requires
}

/// Canonicalize a path for identity comparison against
/// [`luabox_bundle::resolve_module`]'s canonicalized results; fall back to
/// the raw path when the file cannot be canonicalized.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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
    /// `[types] defs`, ambient definition packages resolved from the
    /// project-local `defs/` directory (SPEC.md §3, §5). `pub(crate)` so
    /// `doc_cmd` can resolve the same def files it uses for type-checking
    /// when harvesting classes for documentation (#87).
    pub(crate) defs: Vec<String>,
    /// Definition files each direct dependency contributes to *this* project's
    /// ambient scope (#108, the luals `workspace.library` model): each direct
    /// dependency's own `[types] defs`, resolved from that dependency's
    /// `defs/` directory, in dependency-name-alphabetical order. Loaded into
    /// the same ambient layer as the project's own defs, after them.
    /// `pub(crate)` for the same reason as `defs` above.
    pub(crate) dep_defs: Vec<DefFile>,
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
                defs: manifest.types.defs.clone(),
                dep_defs: resolve_dep_defs(current, &manifest),
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
        defs: Vec::new(),
        dep_defs: Vec::new(),
    })
}

/// Resolve `[types] defs` entries against the project-local `defs/`
/// directory: each name loads `defs/<name>.d.lua` or every `*.d.lua` under
/// `defs/<name>/` (SPEC.md §3 — registry-distributed packages are P2+).
/// Returns the resolved def files (each carrying a root-relative label for
/// diagnostics) plus a diagnostic per unresolvable entry.
///
/// `pub(crate)`: `doc_cmd` reuses this to harvest classes declared in
/// `---@meta` def files onto their own doc pages (#87) — the same
/// resolution `check_once` uses for type-checking, so the two stay in sync
/// with no duplicated logic.
pub(crate) fn resolve_project_defs(
    root: &Path,
    names: &[String],
) -> (Vec<DefFile>, Vec<Diagnostic>) {
    let mut defs = Vec::new();
    let mut diags = Vec::new();
    let defs_dir = root.join("defs");
    for name in names {
        let single = defs_dir.join(format!("{name}.d.lua"));
        let dir = defs_dir.join(name);
        let mut found = false;
        if single.is_file()
            && let Ok(text) = fs::read_to_string(&single)
        {
            defs.push(DefFile {
                file: display_rel(&single, root),
                text,
            });
            found = true;
        }
        if dir.is_dir() {
            let mut files = Vec::new();
            collect_d_lua(&dir, &mut files);
            files.sort();
            for file in files {
                if let Ok(text) = fs::read_to_string(&file) {
                    defs.push(DefFile {
                        file: display_rel(&file, root),
                        text,
                    });
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
    (defs, diags)
}

/// Resolve the def files each DIRECT dependency contributes to the consuming
/// project's ambient scope (#108, the luals `workspace.library` model). For
/// each direct dependency (`[dependencies]` + `[dev-dependencies]`) in
/// alphabetical name order — the deterministic collision-winner order — locate
/// its package root (a path dependency in place at its `path`, every other
/// kind under `lua_modules/<name>/`), read that dependency's *own* `[types]
/// defs`, and load those files from the dependency's `defs/` directory. A
/// dependency with no manifest on disk (uninstalled, or a source kind whose
/// root cannot be located here) or no `[types] defs` simply contributes
/// nothing. Resolution is one level deep only: a dependency's *own*
/// dependencies' defs do not transit.
///
/// Shared with `lint_cmd` (its `undefined-global` known-globals baseline must
/// count dependency defs' globals too, #103/#108).
pub(crate) fn resolve_dep_defs(root: &Path, manifest: &Manifest) -> Vec<DefFile> {
    // `[dependencies]` and `[dev-dependencies]` are each `BTreeMap`s (already
    // name-sorted); merge them into one name-sorted list so the winner order
    // is a single alphabetical sweep across both.
    let mut deps: Vec<(&String, &Dependency)> = manifest
        .dependencies
        .iter()
        .chain(&manifest.dev_dependencies)
        .collect();
    deps.sort_by(|a, b| a.0.cmp(b.0));

    let mut out = Vec::new();
    for (name, dep) in deps {
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
        let defs_dir = dep_root.join("defs");
        for def_name in &dep_manifest.types.defs {
            let single = defs_dir.join(format!("{def_name}.d.lua"));
            if single.is_file()
                && let Ok(text) = fs::read_to_string(&single)
            {
                out.push(DefFile {
                    file: dep_def_label(name, &single, &dep_root),
                    text,
                });
            }
            let dir = defs_dir.join(def_name);
            if dir.is_dir() {
                let mut files = Vec::new();
                collect_d_lua(&dir, &mut files);
                files.sort();
                for file in files {
                    if let Ok(text) = fs::read_to_string(&file) {
                        out.push(DefFile {
                            file: dep_def_label(name, &file, &dep_root),
                            text,
                        });
                    }
                }
            }
        }
    }
    out
}

/// A readable, deterministic label for a dependency-contributed def file: the
/// dependency name plus the file's path within the dependency
/// (`<dep>/defs/<name>.d.lua`), forward-slashed for cross-platform stability.
fn dep_def_label(dep_name: &str, file: &Path, dep_root: &Path) -> String {
    let rel = file
        .strip_prefix(dep_root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    format!("{dep_name}/{rel}")
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

/// All `*.lua` files under the project root, deterministic order, skipping
/// dot-directories and the build output directory.
pub(crate) fn collect_files(project: &Project) -> anyhow::Result<Vec<PathBuf>> {
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
        } else if !hidden {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // `*.d.lua` are `---@meta` definition files (ambient type
            // surfaces), never checked as project source.
            if path.extension().and_then(|e| e.to_str()) == Some("lua") && !name.ends_with(".d.lua")
            {
                lua.push(path);
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
