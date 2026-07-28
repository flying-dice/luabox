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
use luabox_manifest::layout::{self, DefFiles, DefSource};
use luabox_manifest::model::{Build, DialectId, Manifest};
use luabox_syntax::{Dialect, lua};
use luabox_types::ty::Ty;
use luabox_types::{Ambient, DefFile, Strictness, build_ambient_checked, stdlib_defs};
use rayon::prelude::*;

use layout::display_rel;

/// What a manifest-less directory is checked as: Lua 5.4, warn mode — least
/// surprise. Held in both vocabularies because a manifest-less project still
/// needs a `[build]` config (`Build::defaults`) to build with.
const DEFAULT_DIALECT_ID: DialectId = DialectId::Lua54;
const DEFAULT_DIALECT: Dialect = Dialect::Lua54;

/// Execute `luabox check` from `cwd`. With `watch`, the check reruns on
/// every debounced, filtered filesystem change under the project root
/// (`crate::watch`) until interrupted (Ctrl-C); a failing rerun is
/// reported but does not stop the watcher, so in watch mode this function
/// only returns on setup failure (e.g. the watch root can't be observed).
/// Without `watch` it runs once and its `Result` becomes the process exit
/// code, as before.
pub fn run(cwd: &Path, target: Option<&str>, format: Format, watch: bool) -> anyhow::Result<()> {
    let project = discover(cwd)?;
    if watch {
        let cwd = cwd.to_path_buf();
        let target = target.map(str::to_owned);
        return crate::watch::run(&project.root, project.out_dir.as_deref(), move || {
            // Rediscovered per rerun — that is what makes a manifest edit
            // (edition, strictness) take effect on the very next rerun. The
            // discovery above is only for the root/out-dir being watched.
            run_once(&discover(&cwd)?, target.as_deref(), format)
        });
    }
    run_once(&project, target, format)
}

/// The single-pass body of `luabox check`: typecheck every file of an
/// already-discovered project and translate the diagnostics into an exit
/// code. Shared by one-shot `run`, each rerun of `run` in `--watch` mode, and
/// the check-first gate of `luabox build` (`crate::build_cmd`).
///
/// It takes a `&Project` rather than a directory to discover (CC-M11): the
/// caller decides what the project *is*, which is how `build` gets its chosen
/// `--out` skipped — it sets `out_dir` on the project it already discovered,
/// so previously emitted output is never checked as source, and no second
/// manifest read is needed to arrange that.
// Require-resolution is single-sourced through [`luabox_bundle::resolve_module`]
// (SPEC.md §7): this CLI path resolves against the filesystem, and luabox-db —
// the Semantics seam behind the LSP — resolves the same candidate ordering
// ([`luabox_bundle::resolve_candidates`]) over its in-memory file set. The two
// front-ends therefore cannot disagree on which file a `require` names (the
// prior db path-suffix approximation is gone). Surface assembly likewise flows
// through the one `luabox_types::module_surface` producer on both sides.
pub(crate) fn run_once(
    project: &Project,
    target: Option<&str>,
    format: Format,
) -> anyhow::Result<()> {
    // Validate --target up front: a bad value is itself a diagnostic, so it
    // rides the chosen output format like any other finding rather than
    // becoming a usage error (`crate::dialect`).
    let mut target_dialect = None;
    if let Some(id) = target {
        match crate::dialect::parse("target", id) {
            Ok(dialect) => target_dialect = Some(dialect),
            Err(unknown) => return finish(&[unknown.diagnostic()], format, &project.root, 0),
        }
    }

    let lua_files =
        layout::collect_lua_files(&project.root, project.out_dir.as_deref(), DefFiles::Exclude)?;
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
                project,
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
            Diagnostic::error(Code::new(1), err.message.clone()).with_label(Label::primary(
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
    let mut seen: HashSet<(u16, u32, u32)> = HashSet::new();
    for dialect in passes {
        for err in lua::validate::validate(&parse, dialect) {
            let key = (err.code, err.range.start().into(), err.range.end().into());
            if !seen.insert(key) {
                continue;
            }
            diags.push(
                Diagnostic::error(Code::new(err.code), err.message).with_label(Label::primary(
                    Span::new(rel, to_range(err.range)),
                    "not legal in this edition",
                )),
            );
        }
    }

    // 3. Types against the ambient definition-package layer (SPEC.md §3),
    // with this file's resolved `require` exports in reach (#85).
    let requires = resolve_requires(&parse, &project.root, project.build_target, exports);
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
///
/// `dialect` is the project's `[build] target` (the edition when none is
/// set): it selects the `lua_modules/share/lua/<X.Y>/` version directory of
/// a luarocks tree, so `check` looks where the build will.
fn resolve_requires(
    parse: &lua::Parse,
    root: &Path,
    dialect: Dialect,
    exports: &HashMap<PathBuf, Ty>,
) -> HashMap<String, Ty> {
    let mut requires = HashMap::new();
    for module in luabox_types::module_requires(parse) {
        if let Some(target) = luabox_bundle::resolve_module(root, &module, dialect)
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

fn to_range(range: rowan::TextRange) -> std::ops::Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) dialect: Dialect,
    strictness: Strictness,
    pub(crate) out_dir: Option<PathBuf>,
    /// `[build] target` — the dialect you ship (SPEC.md §2.1, §5); defaults
    /// to the edition. Consumed by `crate::build_cmd`.
    pub(crate) build_target: Dialect,
    /// `[package] name`, empty when the manifest omits it (SPEC.md §6: the
    /// rockspec is the package manifest) or when there is no manifest at all.
    /// Each consumer supplies its own substitute — `build` bundles as
    /// `"bundle"`, `doc` titles the site after the project directory — which
    /// is why discovery reports the manifest's answer rather than guessing.
    pub(crate) name: String,
    /// `[package] description`, for the `nvim-plugin` doc stub.
    pub(crate) description: Option<String>,
    /// `[build]` (SPEC.md §5, §7). Discovery is the *only* manifest read on
    /// the `build` path: flags override these fields, nothing re-parses
    /// `luabox.toml` to find them again.
    pub(crate) build: Build,
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
    let Some((root, manifest)) = layout::discover_manifest(cwd)? else {
        return Ok(Project {
            root: cwd.to_path_buf(),
            dialect: DEFAULT_DIALECT,
            strictness: Strictness::Warn,
            out_dir: None,
            build_target: DEFAULT_DIALECT,
            name: String::new(),
            description: None,
            build: Build::defaults(DEFAULT_DIALECT_ID),
            defs: Vec::new(),
            dep_defs: Vec::new(),
        });
    };
    // No "unknown edition in a manifest that already parsed" arm: `[package]
    // edition` and `[build] target` are typed as `DialectId` by
    // `Manifest::parse`, so the mapping inward is exhaustive (CC-M13).
    Ok(Project {
        dialect: crate::dialect::from_manifest(manifest.package.edition),
        strictness: Strictness::from_manifest_flag(manifest.types.strict),
        out_dir: Some(root.join(&manifest.build.out)),
        build_target: crate::dialect::from_manifest(manifest.build.target),
        name: manifest.package.name.clone(),
        description: manifest.package.description.clone(),
        build: manifest.build.clone(),
        defs: manifest.types.defs.clone(),
        dep_defs: resolve_dep_defs(&root, &manifest),
        root,
    })
}

/// [`layout::resolve_project_defs`] with `check`'s diagnostic for the entries
/// that resolve to nothing: `LB1002`, naming the two layouts a definition
/// package may take.
///
/// The resolution itself is shared (`lint` and the LSP run the same walk);
/// what is `check`'s alone is *reporting* an unresolved entry as an error —
/// `lint` and the editor simply do without those globals.
///
/// `pub(crate)`: `doc_cmd` reuses this to harvest classes declared in
/// `---@meta` def files onto their own doc pages (#87) — the same resolution
/// the typecheck uses, so the two cannot drift.
pub(crate) fn resolve_project_defs(
    root: &Path,
    names: &[String],
) -> (Vec<DefFile>, Vec<Diagnostic>) {
    let (defs, unresolved) = layout::resolve_project_defs(root, names);
    let diags = unresolved
        .into_iter()
        .map(|name| {
            Diagnostic::error(
                Code::new(1002),
                format!("cannot resolve definition package `{name}` from `[types] defs`"),
            )
            .with_note(format!(
                "expected `defs/{name}.d.lua` or a `defs/{name}/` directory of `*.d.lua` files under the project root"
            ))
        })
        .collect();
    (defs.into_iter().map(def_file).collect(), diags)
}

/// [`layout::resolve_dep_defs`] in the typechecker's own [`DefFile`] shape —
/// the def files each DIRECT dependency contributes to this project's ambient
/// scope (#108, the luals `workspace.library` model).
///
/// Shared with `lint_cmd` (its `undefined-global` known-globals baseline must
/// count dependency defs' globals too, #103/#108).
pub(crate) fn resolve_dep_defs(root: &Path, manifest: &Manifest) -> Vec<DefFile> {
    layout::resolve_dep_defs(root, manifest)
        .into_iter()
        .map(def_file)
        .collect()
}

/// A resolved definition file in the typechecker's shape: `luabox-manifest`
/// owns the layout walk but must not depend on `luabox-types` (SPEC.md §16 —
/// Distribution never reaches into Semantics), so the label/text pair crosses
/// the boundary and is re-wrapped here.
fn def_file(source: DefSource) -> DefFile {
    DefFile {
        file: source.label,
        text: source.text,
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

    /// `luabox check` as the CLI runs it: discover, then check once. The
    /// output format is clap's problem now (`crate::FormatArg`), so it
    /// arrives already typed.
    fn check(cwd: &Path, target: Option<&str>, format: Format) -> anyhow::Result<()> {
        run_once(&discover(cwd)?, target, format)
    }

    /// [`check`] with the chosen out directory overridden, as `luabox build`
    /// does for its check gate.
    fn check_skipping(cwd: &Path, out: &Path) -> anyhow::Result<()> {
        let mut project = discover(cwd)?;
        project.out_dir = Some(out.to_path_buf());
        run_once(&project, None, Format::Human)
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
        check(tmp.path(), None, Format::Human).expect("check passes");
    }

    #[test]
    fn an_empty_project_checks_successfully() {
        let tmp = project(&manifest("5.4", ""));
        check(tmp.path(), None, Format::Human).expect("check passes");
    }

    #[test]
    fn a_syntax_error_fails_the_check() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "local x = \n");
        let error = check(tmp.path(), None, Format::Human)
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
        assert!(check(tmp.path(), None, Format::Human).is_err());
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
        check(tmp.path(), None, Format::Human).expect("warnings do not fail");
    }

    #[test]
    fn an_unknown_target_is_reported_as_a_diagnostic_not_a_bare_error() {
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "return 0\n");
        let error = check(tmp.path(), Some("5.9"), Format::Human)
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
            check(tmp.path(), Some(target), Format::Human)
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
        check(tmp.path(), None, Format::Human).expect("legal in the edition");
        assert!(check(tmp.path(), Some("5.1"), Format::Human).is_err());
    }

    #[test]
    fn a_target_equal_to_the_edition_does_not_duplicate_findings() {
        let tmp = project(&manifest("5.1", ""));
        write(tmp.path(), "src/main.lua", "local i = 0\ngoto top\n");
        let with_target = check(tmp.path(), Some("5.1"), Format::Human)
            .unwrap_err()
            .to_string();
        let without = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        // The edition pass and the target pass are the same dialect: the
        // finding is reported once, so the error counts match exactly.
        assert_eq!(with_target, without);
    }

    #[test]
    fn a_construct_illegal_in_both_the_edition_and_the_target_is_reported_once() {
        // Floor division is 5.3+, so a 5.1 project shipping to 5.2 fails
        // both passes for the same range — the second is deduplicated, so
        // the reader sees one finding, not the same one twice.
        let tmp = project(&manifest("5.1", ""));
        write(tmp.path(), "src/main.lua", "local x = 7 // 2\n");
        let error = check(tmp.path(), Some("5.2"), Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
    }

    #[test]
    fn diagnostics_render_in_the_requested_machine_format() {
        for format in [
            Format::Human,
            Format::Json,
            Format::Sarif,
            Format::GithubActions,
            Format::GitlabCodeQuality,
        ] {
            let tmp = project(&manifest("5.4", ""));
            write(tmp.path(), "src/main.lua", "local x = \n");
            let error = check(tmp.path(), None, format).unwrap_err().to_string();
            assert!(error.contains("check failed"), "for {format:?}: {error}");
        }
    }

    #[test]
    fn definition_files_are_not_checked_as_project_source() {
        let tmp = project(&manifest("5.4", ""));
        // Broken syntax in a `*.d.lua` would fail the check if it were
        // walked as project source; `*.d.lua` are ambient surfaces instead.
        write(tmp.path(), "defs/broken.d.lua", "local x = \n");
        check(tmp.path(), None, Format::Human).expect("d.lua files are skipped");
    }

    #[test]
    fn previously_emitted_build_output_is_skipped_via_the_project_out_dir() {
        let tmp = project(&manifest("5.4", "\n[build]\nout = \"dist\"\n"));
        write(tmp.path(), "src/main.lua", "return 0\n");
        write(tmp.path(), "custom-out/main.lua", "local x = \n");

        // The manifest's out dir doesn't cover `custom-out/`, so an
        // unqualified check sees the broken emitted file...
        assert!(check(tmp.path(), None, Format::Human).is_err());
        // ...but `build` setting its chosen `--out` on the project does not.
        let out = tmp.path().join("custom-out");
        check_skipping(tmp.path(), &out).expect("emitted output is skipped");
    }

    #[test]
    fn a_malformed_manifest_fails_the_check_rather_than_defaulting() {
        let tmp = project("not = = toml\n");
        let error = check(tmp.path(), None, Format::Human)
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
        check(tmp.path(), None, Format::Human).expect("cross-file require checks clean");
    }

    // -- `[types] defs` resolution -----------------------------------------
    //
    // The `defs/<name>.d.lua` / `defs/<name>/` resolution itself belongs to
    // `luabox_manifest::layout` and is tested there; these pin what `check`
    // adds on top — the `DefFile` shape and the `LB1002` an unresolved entry
    // becomes.

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
        let error = check(tmp.path(), None, Format::Human)
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
        check(tmp.path(), None, Format::Human).expect("the ambient global checks clean");
    }

    // -- dependency-contributed defs (#108) --------------------------------
    //
    // The resolution rules (rock-tree vs path roots, alphabetical winner
    // order, one level deep) are `luabox_manifest::layout`'s and are tested
    // there. What is checked here is the CLI's half: the label and text
    // arriving intact on a `DefFile` the typechecker can consume.

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

    fn read_manifest_for_test(root: &Path) -> Manifest {
        let text = fs::read_to_string(root.join("luabox.toml")).expect("manifest");
        Manifest::parse(&text).expect("manifest parses")
    }

    // -- small helpers -----------------------------------------------------

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
