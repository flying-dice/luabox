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
use luabox_diag::{Code, Diagnostic, Format, Label, Span};
use luabox_resolve::manifest::{Dependency, Manifest};
use luabox_syntax::{Dialect, lua};
use luabox_types::ty::Ty;
use luabox_types::{Ambient, DefFile, Strictness, build_ambient_checked, stdlib_defs};
use rayon::prelude::*;

use crate::project::{collect_lua_files, display_rel};

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
// Require-resolution is single-sourced through [`luabox_bundle::resolve_module`]
// (SPEC.md §7): this CLI path resolves against the filesystem, and luabox-db —
// the Semantics seam behind the LSP — resolves the same candidate ordering
// ([`luabox_bundle::resolve_candidates`]) over its in-memory file set. The two
// front-ends therefore cannot disagree on which file a `require` names (the
// prior db path-suffix approximation is gone). Surface assembly likewise flows
// through the one `luabox_types::module_surface` producer on both sides.
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

    let lua_files = collect_lua_files(&project.root, project.out_dir.as_deref(), true)?;
    // Definition packages (SPEC.md §3): the dialect stdlib layer, plus any
    // project-local `[types] defs` resolved from `<root>/defs/`, plus each
    // direct dependency's own `[types] defs` — the luals `workspace.library`
    // model (#108): a dependency's def files join the consumer's ambient
    // scope. Winner-first order (project defs, then dependencies
    // alphabetically); `build_ambient_checked` reports cross-package class
    // collisions (`LB0307`). Built once and shared by reference across the
    // rayon workers; the no-defs case reuses a process-lifetime cache.
    let (mut all_defs, mut def_diags) = resolve_project_defs(&project.root, &project.defs);
    all_defs.extend(project.dep_defs.iter().cloned());
    let ambient_owned: Option<Ambient> = if all_defs.is_empty() {
        None
    } else {
        let (ambient, collisions) = build_ambient_checked(project.dialect, &all_defs);
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
        project.dialect,
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
    let counts = crate::project::render_diagnostics(diags, format, root);
    eprintln!(
        "check: {} errors, {} warnings in {file_count} files",
        counts.errors, counts.warnings
    );
    if counts.errors > 0 {
        bail!("check failed with {} error(s)", counts.errors);
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
    let Some((root, manifest)) = crate::project::discover_manifest(cwd)? else {
        return Ok(Project {
            root: cwd.to_path_buf(),
            dialect: Dialect::Lua54,
            strictness: Strictness::Warn,
            out_dir: None,
            build_target: Dialect::Lua54,
            defs: Vec::new(),
            dep_defs: Vec::new(),
        });
    };
    let manifest_path = root.join("luabox.toml");
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
    Ok(Project {
        root: root.clone(),
        dialect,
        strictness: Strictness::from_manifest_flag(manifest.types.strict),
        out_dir: Some(root.join(&manifest.build.out)),
        build_target,
        defs: manifest.types.defs.clone(),
        dep_defs: resolve_dep_defs(&root, &manifest),
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
/// kind under `lua_modules/<name>/` — the rock tree the user materializes
/// with luarocks; luabox only reads it), read that dependency's *own*
/// `[types] defs`, and load those files from the dependency's `defs/`
/// directory. A dependency with no manifest on disk (not materialized, or a
/// source kind whose root cannot be located here) or no `[types] defs` simply
/// contributes nothing. Resolution is one level deep only: a dependency's
/// *own* dependencies' defs do not transit.
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

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Write `contents` to `root/rel`, creating parent directories.
    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("has a parent")).expect("create parents");
        fs::write(&path, contents).expect("write file");
    }

    /// A `luabox.toml` body with the given extra tables appended.
    fn manifest(edition: &str, extra: &str) -> String {
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n{extra}"
        )
    }

    /// A project rooted in a fresh tempdir with the given manifest body.
    fn project(manifest_text: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "luabox.toml", manifest_text);
        tmp
    }

    // -- format parsing ----------------------------------------------------

    #[test]
    fn every_documented_output_format_is_accepted() {
        for (name, expected) in [
            ("human", Format::Human),
            ("json", Format::Json),
            ("sarif", Format::Sarif),
            ("github", Format::GithubActions),
            ("gitlab", Format::GitlabCodeQuality),
        ] {
            assert_eq!(
                parse_format(name).expect("accepted"),
                expected,
                "for {name}"
            );
        }
    }

    #[test]
    fn an_unknown_output_format_is_rejected_listing_the_valid_ones() {
        let error = parse_format("xml").unwrap_err().to_string();
        assert!(error.contains("unknown format `xml`"), "{error}");
        assert!(
            error.contains("human, json, sarif, github, or gitlab"),
            "{error}"
        );
    }

    #[test]
    fn run_once_rejects_a_bad_format_before_touching_the_filesystem() {
        // No project, no files — the format is validated first.
        let tmp = tempfile::tempdir().expect("tempdir");
        assert!(run_once(tmp.path(), None, "yaml", None).is_err());
    }

    // -- discovery ---------------------------------------------------------

    #[test]
    fn a_manifest_less_directory_checks_as_lua_54_in_warn_mode() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let project = discover(tmp.path()).expect("manifest-less default");
        assert_eq!(project.root, tmp.path().to_path_buf());
        assert_eq!(project.dialect, Dialect::Lua54);
        assert_eq!(project.build_target, Dialect::Lua54);
        assert_eq!(project.strictness, Strictness::Warn);
        assert!(project.out_dir.is_none());
        assert!(project.defs.is_empty());
        assert!(project.dep_defs.is_empty());
    }

    #[test]
    fn discover_reads_edition_build_target_out_dir_strictness_and_defs() {
        let tmp = project(&manifest(
            "5.1",
            "\n[build]\ntarget = \"5.4\"\nout = \"build\"\n\n[types]\nstrict = true\ndefs = [\"mylib\"]\n",
        ));
        let project = discover(tmp.path()).expect("discovers");
        assert_eq!(project.dialect, Dialect::Lua51);
        assert_eq!(project.build_target, Dialect::Lua54);
        assert_eq!(project.out_dir, Some(tmp.path().join("build")));
        assert_eq!(project.strictness, Strictness::Strict);
        assert_eq!(project.defs, vec!["mylib".to_owned()]);
    }

    #[test]
    fn the_build_target_defaults_to_the_edition_when_unset() {
        let tmp = project(&manifest("5.2", ""));
        let project = discover(tmp.path()).expect("discovers");
        assert_eq!(project.dialect, Dialect::Lua52);
        assert_eq!(project.build_target, Dialect::Lua52);
    }

    #[test]
    fn discovery_walks_up_from_a_subdirectory_to_the_project_root() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/deep/main.lua", "return 0\n");
        let project = discover(&tmp.path().join("src").join("deep")).expect("discovers");
        assert_eq!(project.root, tmp.path().to_path_buf());
    }

    // -- the one-shot check ------------------------------------------------

    #[test]
    fn a_clean_project_checks_successfully() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "local x = 1\nprint(x)\n");
        run_once(tmp.path(), None, "human", None).expect("check passes");
    }

    #[test]
    fn an_empty_project_checks_successfully() {
        let tmp = project(&manifest("5.4", ""));
        run_once(tmp.path(), None, "human", None).expect("check passes");
    }

    #[test]
    fn a_syntax_error_fails_the_check() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "local x = \n");
        let error = run_once(tmp.path(), None, "human", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("check failed with"), "{error}");
        assert!(error.contains("error(s)"), "{error}");
    }

    #[test]
    fn a_strict_type_error_fails_the_check() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "src/main.lua",
            "---@param n number\nlocal function double(n)\n  return n * 2\nend\ndouble(\"nope\")\n",
        );
        assert!(run_once(tmp.path(), None, "human", None).is_err());
    }

    #[test]
    fn the_same_type_mismatch_is_only_a_warning_outside_strict_mode() {
        let tmp = project(&manifest("5.4", ""));
        write(
            tmp.path(),
            "src/main.lua",
            "---@param n number\nlocal function double(n)\n  return n * 2\nend\ndouble(\"nope\")\n",
        );
        // Warnings never fail the command (module docs, SPEC.md §3).
        run_once(tmp.path(), None, "human", None).expect("warnings do not fail");
    }

    #[test]
    fn an_unknown_target_is_reported_as_a_diagnostic_not_a_bare_error() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "return 0\n");
        let error = run_once(tmp.path(), Some("5.9"), "human", None)
            .unwrap_err()
            .to_string();
        // LB1001 is rendered like any other diagnostic, then the run fails
        // with the usual error-count summary.
        assert!(error.contains("check failed with 1 error(s)"), "{error}");
    }

    #[test]
    fn every_valid_target_is_accepted() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "return 0\n");
        for target in ["5.1", "5.2", "5.3", "5.4", "luajit"] {
            run_once(tmp.path(), Some(target), "human", None)
                .unwrap_or_else(|e| panic!("target {target} should check clean: {e}"));
        }
    }

    #[test]
    fn a_construct_illegal_on_the_ship_target_is_reported_only_with_that_target() {
        let tmp = project(&manifest("5.4", ""));
        // `goto` is legal in 5.4 (the edition) but not in 5.1.
        write(
            tmp.path(),
            "src/main.lua",
            "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\n",
        );
        run_once(tmp.path(), None, "human", None).expect("legal in the edition");
        assert!(run_once(tmp.path(), Some("5.1"), "human", None).is_err());
    }

    #[test]
    fn a_target_equal_to_the_edition_does_not_duplicate_findings() {
        let tmp = project(&manifest("5.1", ""));
        write(tmp.path(), "src/main.lua", "local i = 0\ngoto top\n");
        let with_target = run_once(tmp.path(), Some("5.1"), "human", None)
            .unwrap_err()
            .to_string();
        let without = run_once(tmp.path(), None, "human", None)
            .unwrap_err()
            .to_string();
        // The edition pass and the target pass are the same dialect: the
        // finding is reported once, so the error counts match exactly.
        assert_eq!(with_target, without);
    }

    #[test]
    fn diagnostics_render_in_the_requested_machine_format() {
        for format in ["human", "json", "sarif", "github", "gitlab"] {
            let tmp = project(&manifest("5.4", ""));
            write(tmp.path(), "src/main.lua", "local x = \n");
            let error = run_once(tmp.path(), None, format, None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("check failed"), "for {format}: {error}");
        }
    }

    #[test]
    fn definition_files_are_not_checked_as_project_source() {
        let tmp = project(&manifest("5.4", ""));
        // Broken syntax in a `*.d.lua` would fail the check if it were
        // walked as project source; `*.d.lua` are ambient surfaces instead.
        write(tmp.path(), "defs/broken.d.lua", "local x = \n");
        run_once(tmp.path(), None, "human", None).expect("d.lua files are skipped");
    }

    #[test]
    fn previously_emitted_build_output_is_skipped_via_skip_out() {
        let tmp = project(&manifest("5.4", "\n[build]\nout = \"dist\"\n"));
        write(tmp.path(), "src/main.lua", "return 0\n");
        write(tmp.path(), "custom-out/main.lua", "local x = \n");

        // The manifest's out dir doesn't cover `custom-out/`, so an
        // unqualified check sees the broken emitted file...
        assert!(run_once(tmp.path(), None, "human", None).is_err());
        // ...but `build` passing its chosen `--out` as `skip_out` does not.
        let out = tmp.path().join("custom-out");
        run_once(tmp.path(), None, "human", Some(&out)).expect("emitted output is skipped");
    }

    #[test]
    fn a_malformed_manifest_fails_the_check_rather_than_defaulting() {
        let tmp = project("not = = toml\n");
        let error = run_once(tmp.path(), None, "human", None)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("invalid `"), "{error}");
    }

    #[test]
    fn a_require_of_a_sibling_module_reaches_that_module_s_export_type() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "src/greet.lua",
            "local M = {}\n\
             ---@param name string\n\
             ---@return string\n\
             function M.hello(name)\n  return \"hi \" .. name\nend\n\
             return M\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "local greet = require(\"src.greet\")\nprint(greet.hello(\"world\"))\n",
        );
        run_once(tmp.path(), None, "human", None).expect("cross-file require checks clean");
    }

    // -- `[types] defs` resolution -----------------------------------------

    #[test]
    fn a_defs_entry_resolves_to_a_single_d_lua_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "defs/mylib.d.lua",
            "---@meta\n---@class MyLib\nmylib = {}\n",
        );
        let (defs, diags) = resolve_project_defs(tmp.path(), &["mylib".to_owned()]);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "defs/mylib.d.lua");
        assert!(defs[0].text.contains("---@class MyLib"));
    }

    #[test]
    fn a_defs_entry_resolves_to_every_d_lua_file_under_a_directory_sorted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/pack/z.d.lua", "---@meta\n");
        write(tmp.path(), "defs/pack/a.d.lua", "---@meta\n");
        write(tmp.path(), "defs/pack/nested/m.d.lua", "---@meta\n");
        // Not a definition file — never picked up.
        write(tmp.path(), "defs/pack/plain.lua", "return 0\n");

        let (defs, diags) = resolve_project_defs(tmp.path(), &["pack".to_owned()]);
        assert!(diags.is_empty(), "{diags:?}");
        let files: Vec<&str> = defs.iter().map(|d| d.file.as_str()).collect();
        assert_eq!(
            files,
            [
                "defs/pack/a.d.lua",
                "defs/pack/nested/m.d.lua",
                "defs/pack/z.d.lua"
            ]
        );
    }

    #[test]
    fn a_single_file_and_a_directory_of_the_same_name_both_contribute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "defs/both.d.lua", "---@meta\n");
        write(tmp.path(), "defs/both/extra.d.lua", "---@meta\n");
        let (defs, diags) = resolve_project_defs(tmp.path(), &["both".to_owned()]);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(defs.len(), 2);
    }

    #[test]
    fn an_unresolvable_defs_entry_is_reported_as_lb1002_with_the_expected_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (defs, diags) = resolve_project_defs(tmp.path(), &["ghost".to_owned()]);
        assert!(defs.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code.to_string(), "LB1002");
        assert!(diags[0].message.contains("ghost"), "{}", diags[0].message);
        let notes = format!("{:?}", diags[0].notes);
        assert!(notes.contains("defs/ghost.d.lua"), "{notes}");
        assert!(notes.contains("defs/ghost/"), "{notes}");
    }

    #[test]
    fn an_unresolvable_defs_entry_fails_the_check() {
        let tmp = project(&manifest("5.4", "\n[types]\ndefs = [\"ghost\"]\n"));
        write(tmp.path(), "src/main.lua", "return 0\n");
        let error = run_once(tmp.path(), None, "human", None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("check failed with 1 error(s)"), "{error}");
    }

    #[test]
    fn a_defs_declared_global_is_in_scope_for_the_checked_project() {
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"mylib\"]\n",
        ));
        write(
            tmp.path(),
            "defs/mylib.d.lua",
            "---@meta\n\
             ---@class MyLib\n\
             ---@field version string\n\
             mylib = {}\n",
        );
        write(tmp.path(), "src/main.lua", "print(mylib.version)\n");
        run_once(tmp.path(), None, "human", None).expect("the ambient global checks clean");
    }

    // -- dependency-contributed defs (#108) --------------------------------

    #[test]
    fn a_path_dependency_contributes_its_own_defs_to_the_consumer() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "5.4",
                "\n[dependencies]\ngeometry = { path = \"vendor/geometry\" }\n",
            ),
        );
        write(
            tmp.path(),
            "vendor/geometry/luabox.toml",
            &manifest("5.4", "\n[types]\ndefs = [\"geometry\"]\n"),
        );
        write(
            tmp.path(),
            "vendor/geometry/defs/geometry.d.lua",
            "---@meta\n---@class geometry.Shape\n",
        );

        let manifest = read_manifest_for_test(tmp.path());
        let defs = resolve_dep_defs(tmp.path(), &manifest);
        assert_eq!(defs.len(), 1);
        // The label is dependency-prefixed and forward-slashed.
        assert_eq!(defs[0].file, "geometry/defs/geometry.d.lua");
        assert!(defs[0].text.contains("geometry.Shape"));
    }

    #[test]
    fn a_non_path_dependency_is_read_from_the_lua_modules_rock_tree() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("5.4", "\n[dependencies]\nlpeg = \"1.0\"\n"),
        );
        write(
            tmp.path(),
            "lua_modules/lpeg/luabox.toml",
            &manifest("5.4", "\n[types]\ndefs = [\"lpeg\"]\n"),
        );
        write(
            tmp.path(),
            "lua_modules/lpeg/defs/lpeg.d.lua",
            "---@meta\nlpeg = {}\n",
        );

        let manifest = read_manifest_for_test(tmp.path());
        let defs = resolve_dep_defs(tmp.path(), &manifest);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, "lpeg/defs/lpeg.d.lua");
    }

    #[test]
    fn dependency_defs_are_ordered_alphabetically_across_both_dependency_tables() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "5.4",
                "\n[dependencies]\nzeta = { path = \"vendor/zeta\" }\n\
                 \n[dev-dependencies]\nalpha = { path = \"vendor/alpha\" }\n",
            ),
        );
        for name in ["alpha", "zeta"] {
            write(
                tmp.path(),
                &format!("vendor/{name}/luabox.toml"),
                &manifest("5.4", &format!("\n[types]\ndefs = [\"{name}\"]\n")),
            );
            write(
                tmp.path(),
                &format!("vendor/{name}/defs/{name}.d.lua"),
                "---@meta\n",
            );
        }

        let manifest = read_manifest_for_test(tmp.path());
        let files: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.file)
            .collect();
        assert_eq!(files, ["alpha/defs/alpha.d.lua", "zeta/defs/zeta.d.lua"]);
    }

    #[test]
    fn a_dependency_directory_defs_package_contributes_every_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "5.4",
                "\n[dependencies]\npack = { path = \"vendor/pack\" }\n",
            ),
        );
        write(
            tmp.path(),
            "vendor/pack/luabox.toml",
            &manifest("5.4", "\n[types]\ndefs = [\"api\"]\n"),
        );
        write(tmp.path(), "vendor/pack/defs/api/b.d.lua", "---@meta\n");
        write(tmp.path(), "vendor/pack/defs/api/a.d.lua", "---@meta\n");

        let manifest = read_manifest_for_test(tmp.path());
        let files: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.file)
            .collect();
        assert_eq!(files, ["pack/defs/api/a.d.lua", "pack/defs/api/b.d.lua"]);
    }

    #[test]
    fn a_dependency_that_is_not_materialized_contributes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "5.4",
                "\n[dependencies]\nmissing = \"1.0\"\nalso-missing = { path = \"nowhere\" }\n",
            ),
        );
        let manifest = read_manifest_for_test(tmp.path());
        assert!(resolve_dep_defs(tmp.path(), &manifest).is_empty());
    }

    #[test]
    fn a_dependency_with_an_unparseable_manifest_contributes_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest(
                "5.4",
                "\n[dependencies]\nbroken = { path = \"vendor/broken\" }\n",
            ),
        );
        write(tmp.path(), "vendor/broken/luabox.toml", "= = =\n");
        write(tmp.path(), "vendor/broken/defs/broken.d.lua", "---@meta\n");

        let manifest = read_manifest_for_test(tmp.path());
        assert!(resolve_dep_defs(tmp.path(), &manifest).is_empty());
    }

    #[test]
    fn dependency_defs_resolution_is_one_level_deep_only() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "luabox.toml",
            &manifest("5.4", "\n[dependencies]\nmid = { path = \"vendor/mid\" }\n"),
        );
        write(
            tmp.path(),
            "vendor/mid/luabox.toml",
            &manifest(
                "5.4",
                "\n[types]\ndefs = [\"mid\"]\n\n[dependencies]\ndeep = { path = \"../deep\" }\n",
            ),
        );
        write(tmp.path(), "vendor/mid/defs/mid.d.lua", "---@meta\n");
        write(
            tmp.path(),
            "vendor/deep/luabox.toml",
            &manifest("5.4", "\n[types]\ndefs = [\"deep\"]\n"),
        );
        write(tmp.path(), "vendor/deep/defs/deep.d.lua", "---@meta\n");

        let manifest = read_manifest_for_test(tmp.path());
        let files: Vec<String> = resolve_dep_defs(tmp.path(), &manifest)
            .into_iter()
            .map(|d| d.file)
            .collect();
        // `deep` is a transitive dependency: its defs do not transit.
        assert_eq!(files, ["mid/defs/mid.d.lua"]);
    }

    fn read_manifest_for_test(root: &Path) -> Manifest {
        let text = fs::read_to_string(root.join("luabox.toml")).expect("manifest");
        Manifest::parse(&text).expect("manifest parses")
    }

    // -- small helpers -----------------------------------------------------

    #[test]
    fn dep_def_label_prefixes_the_dependency_name_and_forward_slashes_the_rest() {
        let dep_root = Path::new("/tmp/proj/vendor/geometry");
        let file = dep_root.join("defs").join("geometry.d.lua");
        assert_eq!(
            dep_def_label("geometry", &file, dep_root),
            "geometry/defs/geometry.d.lua"
        );
    }

    #[test]
    fn dep_def_label_falls_back_to_the_whole_path_when_it_is_not_under_the_dep_root() {
        let label = dep_def_label("dep", Path::new("/elsewhere/x.d.lua"), Path::new("/root"));
        assert_eq!(label, "dep//elsewhere/x.d.lua");
    }

    #[test]
    fn collect_d_lua_recurses_and_takes_only_d_lua_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "a.d.lua", "");
        write(tmp.path(), "plain.lua", "");
        write(tmp.path(), "notes.txt", "");
        write(tmp.path(), "nested/b.d.lua", "");

        let mut found = Vec::new();
        collect_d_lua(tmp.path(), &mut found);
        found.sort();
        let rel: Vec<String> = found.iter().map(|p| display_rel(p, tmp.path())).collect();
        assert_eq!(rel, ["a.d.lua", "nested/b.d.lua"]);
    }

    #[test]
    fn collect_d_lua_on_a_missing_directory_yields_nothing() {
        let mut found = Vec::new();
        collect_d_lua(Path::new("no-such-directory-xyzzy"), &mut found);
        assert!(found.is_empty());
    }

    #[test]
    fn canonical_falls_back_to_the_raw_path_for_a_file_that_does_not_exist() {
        let missing = Path::new("no-such-file-xyzzy.lua");
        assert_eq!(canonical(missing), missing.to_path_buf());
    }

    #[test]
    fn to_range_converts_a_text_range_to_byte_offsets() {
        let range = rowan::TextRange::new(3.into(), 7.into());
        assert_eq!(to_range(range), 3..7);
    }
}
