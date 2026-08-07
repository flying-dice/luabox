//! `luabox check [--target <t>] [--format <f>] [--watch]` — the CI-grade
//! standalone typecheck (SPEC.md §3, §4, §14).
//!
//! Per `.lua` file, four passes over one parse:
//!
//! 1. **Parse errors** → `LB0001` (the parser is error-resilient; later
//!    passes still run on the recovered tree).
//! 2. **Dialect legality** against the project `edition` — and, with
//!    `--target`, against the ship target too (that is what `--target`
//!    means before lowering exists: "would this source be legal there?").
//!    Duplicate findings (same code, same range) are reported once.
//! 3. **Control-flow legality** (#44) → `LB0020`-`LB0022`: an unresolved
//!    `goto`, a repeated label, `break` with no enclosing loop. Every
//!    reference Lua rejects these at load time; only the duplicate-label
//!    *scope* differs by dialect, which is why this pass runs for the ship
//!    target as well — and, unlike pass 2, for a ship target the manifest
//!    declares in `[build] target` and not only for `--target` (see
//!    [`TargetPasses`]). Skipped when the parse is not clean.
//!
//! Passes 2 and 3 each merge their edition and target runs: one finding per
//! (code, primary span), the target's verdict winning a construct both
//! reject, rendered in source order.
//! 4. **Typecheck** (annotation-driven, against the ambient definition
//!    layer, with each file's cross-file `require` exports in reach — #85)
//!    at the manifest's strictness: `[types] strict = true` → strict
//!    (errors), otherwise warn.
//!
//! Output goes to stdout in the chosen format; a `check: N errors, M
//! warnings in K files` summary goes to stderr. The exit code is nonzero
//! iff any Error-severity diagnostic was produced — warnings never fail
//! the command.
//!
//! Each file is read from disk, parsed, harvested and lowered exactly once
//! per run: the cross-file surface pre-pass and the per-file check share one
//! set of per-file records (`SourceFile`) rather than each walking the source
//! set on its own (CC-M1).
//!
//! `--watch` (SPEC.md §4) turns this into a long-running rerun-on-change
//! loop instead of a one-shot check — see `crate::watch` for the debounce
//! and filtering rules.

use std::collections::HashMap;
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

use crate::emit::errln;

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
// through the one `luabox_types::module_surface` producer on both sides — the
// CLI reaching it through `module_surface_with_artifacts`, which is that same
// producer handed the harvest + lowering it already has.
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
    // With no `--target`, the manifest's `[build] target` still names a ship
    // dialect, and the *loader* half of that question has to be asked: a
    // project declaring `target = "5.4"` passed `luabox check` on a program
    // `luabox check --target 5.4` rejects, then built an artifact that cannot
    // load (Shockwave round 4). It reaches the control-flow pass only — see
    // [`TargetPasses`] for why the dialect-legality pass is not asked of a
    // manifest target: constructs the target's parser rejects are what the
    // declared lowering exists to rewrite, so reporting them would fail every
    // project that uses the feature. `--target` is the explicit "would this
    // *source* be legal there?" question and still drives both passes.
    // `[build] target` defaults to the edition, and `dialect_passes` drops a
    // target equal to the edition, so a project that declares neither is
    // unaffected.
    let passes = TargetPasses {
        legality: target_dialect,
        control_flow: Some(target_dialect.unwrap_or(project.build_target)),
    };
    run_passes(project, passes, format)
}

/// The ship-target legality passes `run_once` runs on top of the project
/// edition, held apart because [`crate::build_cmd`]'s check gate wants one
/// and not the other.
///
/// `legality` (pass 2) asks whether the source *parses* on the target;
/// `control_flow` (pass 3) asks whether the target's *loader* accepts it.
/// The distinction is what lowering can do about each: a construct the
/// target's parser rejects is exactly what lowering exists to rewrite, so
/// `build` must not gate on it, while nothing lowers a duplicate label away,
/// so `build` must.
///
/// The same split decides what a *manifest* `[build] target` may ask. An
/// explicit `--target` is the literal question ("would this source be legal
/// there?") and sets both. `[build] target` is a declaration that the project
/// is lowered to that dialect, so it sets `control_flow` only — asking it for
/// dialect legality would report `LB0011` on every `//` in a 5.3 project that
/// ships 5.1, i.e. fail `luabox check` for using the feature `[build] target`
/// exists to provide.
#[derive(Clone, Copy, Default)]
pub(crate) struct TargetPasses {
    legality: Option<Dialect>,
    control_flow: Option<Dialect>,
}

impl TargetPasses {
    /// The gate `luabox build` runs before emitting: the target's *loader*
    /// verdict only, for the reason spelled out on [`TargetPasses`].
    pub(crate) fn control_flow_only(target: Dialect) -> Self {
        Self {
            legality: None,
            control_flow: Some(target),
        }
    }
}

/// `luabox check` with an explicitly chosen set of target passes — the entry
/// point [`crate::build_cmd`]'s check gate uses. Everything else about the
/// run is identical to [`run_once`].
pub(crate) fn run_gated(
    project: &Project,
    passes: TargetPasses,
    format: Format,
) -> anyhow::Result<()> {
    run_passes(project, passes, format)
}

fn run_passes(project: &Project, passes: TargetPasses, format: Format) -> anyhow::Result<()> {
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

    // Types from a bare luarocks tree (#30). A rock's installed sources under
    // `lua_modules/share/lua/<X.Y>/` are ordinary annotated Lua; their LuaCATS
    // surfaces — classes/enums/aliases plus each module's `require`-export type
    // — join this project's scope with no manifest declaration at all, which is
    // the whole point: `luarocks install --tree lua_modules <rock>` now buys
    // types as well as resolution and bundling. Surface-only and
    // ambient-relaxed: nothing here checks a vendored body or can produce a
    // diagnostic (`luabox_types::rocks`). The version directory is the one the
    // build resolves against, so `check` looks where the build will.
    let rocks = harvest_rocks(project, ambient);

    // Read and parse the whole source set ONCE (CC-M1). Both halves of the
    // check — the cross-file surface pre-pass below and the per-file check
    // after it — used to walk `lua_files` independently, so every file was
    // read twice, parsed twice, harvested twice and lowered three times over.
    // They share these records instead, so each of those happens once.
    //
    // The price is retention: one syntax tree + HIR per project file stays
    // live for the length of the run instead of dying with its rayon task.
    // Measured on the 100-kLOC perf corpus, peak RSS 64 MiB -> 123 MiB, of
    // which the syntax trees are only ~12 MiB and the HIR + harvested
    // annotations ~47 MiB — which is why re-parsing in the second pass to
    // avoid holding the trees (the alternative the audit allowed) is not
    // worth it: it would give back the read + parse saving to reclaim a fifth
    // of the memory.
    let read: Vec<anyhow::Result<SourceFile>> = lua_files
        .par_iter()
        .map(|path| {
            let rel = display_rel(path, &project.root);
            let source =
                fs::read_to_string(path).with_context(|| format!("cannot read `{rel}`"))?;
            let parse = lua::parse(&source, project.dialect);
            let artifacts = luabox_types::FileArtifacts::new(&parse);
            Ok(SourceFile {
                canonical: canonical(path),
                rel,
                parse,
                artifacts,
            })
        })
        .collect();
    let mut files: Vec<SourceFile> = Vec::with_capacity(read.len());
    for result in read {
        files.push(result?);
    }

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
    let surfaces: Vec<luabox_types::ModuleSurface> = files
        .par_iter()
        .map(|file| {
            luabox_types::module_surface_with_artifacts(
                &file.parse,
                &file.rel,
                Some(ambient),
                &file.artifacts,
            )
        })
        .collect();
    // Duplicate `---@alias` across project files / `[types] defs` (luals
    // `duplicate-doc-alias`, LB0310, #113): a project-assembly finding — like
    // the LB0307 class collisions above — computed over the whole source set,
    // never in the per-file check, so a file checked standalone and in-project
    // stays consistent. Winner order matches `with_project_types`.
    def_diags.extend(luabox_types::alias_collisions(
        &all_defs,
        &files
            .iter()
            .zip(&surfaces)
            .map(|(file, surface)| (file.rel.clone(), &surface.types))
            .collect::<Vec<_>>(),
    ));
    // The project-wide ambient: defs + every file's workspace-global
    // classes/enums, merged (defs win same-name member collisions; luals
    // merges duplicate class declarations' fields rather than dropping),
    // then the harvested rock surfaces (#30) LAST — explicit beats implicit,
    // so a rock's declaration only fills a name neither the defs layer nor any
    // project file claimed. Rock surfaces are deliberately absent from the
    // `LB0307`/`LB0310` collision reports above: the user declared neither side
    // of a rock-vs-rock name clash and cannot act on it, and #30 is about a
    // tree that needs no configuration at all.
    let ambient = ambient
        .with_project_types(surfaces.iter().map(|s| &s.types))
        .with_rock_types(rocks.types());
    let ambient = &ambient;

    // Re-reify every file's EXPORT against the now-merged project ambient
    // (round 3 review F42). `surfaces` above was computed with the defs-only
    // `ambient` — required, since the merge just above is built *from*
    // `surfaces.types`, so reifying against the merge while building it would
    // be circular. That leaves a generic ancestor declared in a *different*
    // project file invisible to `reify_export`'s unbound-parameter check when
    // a file's own surface is first computed: `---@class Sub : Base` (bare,
    // `Base` in another file) crossed `require` as the unerased
    // `Ty::Named("Sub")` instead of the lenient structural template the
    // same-file case already gets, leaking `Base`'s free parameter into every
    // consumer once the *consumer's* fully-merged env resolved `Sub` back to
    // its real, still-unbound shape. `luabox-db`'s `module_export_checked`
    // closes the identical gap for the LSP path this same way; only `.export`
    // is re-derived here. `.types` (each file's own declarations) is NOT
    // re-derived — measurably NOT because it is independent of cross-file
    // visibility (round 4 review R15: it is not — `---@field p Point` with
    // `Point` declared in another file lowers against the defs-only ambient
    // `surfaces` was built with, so the `.types` published above carries
    // `p: unknown`, while this same merged ambient would resolve it to
    // `p: Point`), but because that gap is pre-existing and out of scope
    // here: this pass closes F42's export-seam gap, not every place a
    // project file's own declarations go stale relative to the full project
    // merge.
    //
    // Each file's `TypeEnv` against `ambient` is built, used, and dropped
    // right here — NOT threaded through to `check_one` (round 4 review
    // finding 6, reverting round 4 review R14's env-sharing).
    //
    // R14 kept the `TypeEnv` this pass builds around for `check_one` to
    // reuse instead of building a second one: `TypeEnv::build_from_items`
    // clones the whole `ambient` class/enum/function/global surface into a
    // fresh owned map every time it runs, so skipping the second clone
    // measurably cut CPU time (synthetic N-file project, each with two
    // small classes, a linear `require` chain, release build):
    //
    //   N      2 builds/file (transient)   1 build/file (R14, retained)
    //   500    1.32s                       0.75s
    //   1000   6.15s                       3.29s
    //   2000   26.6s                       13.6s
    //
    // What R14 did not measure is what "retained" costs in *peak memory*:
    // every file's env is independently O(project class count), and the
    // export registry's dependency (every file's export must be known
    // before any file's `require`s can resolve, so the check phase cannot
    // start until every file's export pass has finished) means R14 had to
    // collect *all* of them into one `Vec<TypeEnv>` alive across the whole
    // pass — O(files) envs, each O(classes), an O(N²) peak just for the
    // retained array on an N-file project where file count and class count
    // scale together, which the CPU-time table above cannot show at all.
    // Measured (same shape, two-class-per-file synthetic project, peak RSS
    // via `scripts/peak-rss.py`, release build):
    //
    //   N      2 builds/file (transient)   1 build/file (R14, retained)
    //   100    15 MiB                      79 MiB
    //   500    49 MiB                      1419 MiB
    //
    // A ~29x memory blowup at N=500 for a ~1.76x CPU win is the wrong side
    // of that trade — a project large enough to make the CPU saving matter
    // is exactly the project large enough for the retained-`Vec<TypeEnv>`
    // memory to become the dominant cost, or exhaust memory outright, well
    // before `check` finishes. Back to two transient builds per file: the
    // export re-derivation pass below drops its own env the moment it has
    // this file's `.export` (`module_surface_with_artifacts`, matching the
    // very first surface pass above), and `check_one` builds and drops its
    // own separately (`check_file_with_artifacts`) — at any instant only as
    // many envs are alive as there are rayon workers in flight, not one per
    // project file.
    let file_exports: Vec<Option<Ty>> = files
        .par_iter()
        .map(|file| {
            luabox_types::module_surface_with_artifacts(
                &file.parse,
                &file.rel,
                Some(ambient),
                &file.artifacts,
            )
            .export
        })
        .collect();
    let mut exports: HashMap<PathBuf, Ty> = HashMap::new();
    for (export, file) in file_exports.into_iter().zip(&files) {
        if let Some(export) = export {
            exports.insert(file.canonical.clone(), export);
        }
    }
    // Rock exports join the same path-keyed registry (#30). Keying by path (not
    // by module name) is what keeps precedence exact: `resolve_requires` asks
    // the bundler which *file* a `require` names, and a project file that
    // shadows a rock module is the file it returns.
    for (path, export) in rocks.by_path() {
        exports.entry(canonical(path)).or_insert(export.clone());
    }

    // SPEC.md §16: rayon per-module. Each file is checked against the
    // shared project ambient plus its own resolved `require` exports;
    // collecting per-file Vecs preserves source order. Builds its own
    // `TypeEnv` (see the doc comment above `file_exports`).
    let per_file: Vec<Vec<Diagnostic>> = files
        .par_iter()
        .map(|file| {
            let mut diags = Vec::new();
            check_one(file, ambient, project, passes, &exports, &mut diags);
            diags
        })
        .collect();
    let mut diags: Vec<Diagnostic> = def_diags;
    for file_diags in per_file {
        diags.extend(file_diags);
    }

    finish(&diags, format, &project.root, lua_files.len())
}

/// One project file, read and parsed once: everything both halves of the
/// check need from it, so neither goes back to disk nor re-derives what the
/// other already has (CC-M1).
struct SourceFile {
    /// The identity [`luabox_bundle::resolve_module`] hands back, so a
    /// resolved `require` can be matched to the file it names.
    canonical: PathBuf,
    /// Project-relative display path — how diagnostics name this file.
    rel: String,
    parse: lua::Parse,
    /// Harvested annotations + lowered HIR, derived once and threaded into
    /// the surface pass, the `require` inventory, and the check.
    artifacts: luabox_types::FileArtifacts,
}

/// All three passes for one file.
///
/// `ambient` is the project-merged ambient; the type pass below
/// (`check_file_with_artifacts`) builds and drops its own `TypeEnv` against
/// it, rather than reusing one threaded in from [`run_passes`] (round 4
/// review finding 6 reverted round 4 review R14's env-sharing — see the doc
/// comment above `file_exports` in [`run_passes`] for the measurement: R14's
/// retained `Vec<TypeEnv>` was an O(N²)-peak-memory regression, ~29x at
/// N=500 on the benchmark project, for a ~1.76x CPU win).
fn check_one(
    file: &SourceFile,
    ambient: &Ambient,
    project: &Project,
    passes: TargetPasses,
    exports: &HashMap<PathBuf, Ty>,
    diags: &mut Vec<Diagnostic>,
) {
    let SourceFile {
        rel,
        parse,
        artifacts,
        ..
    } = file;
    let rel = rel.as_str();

    // 1. Parse errors.
    for err in parse.errors() {
        diags.push(
            Diagnostic::error(Code::new(1), err.message.clone()).with_label(Label::primary(
                Span::new(rel, to_range(err.range)),
                "syntax error here",
            )),
        );
    }

    // 2. Dialect legality: edition, then ship target (merged — the same
    // construct may be illegal in both, and is then reported once).
    let legality_passes = dialect_passes(project.dialect, passes.legality);
    let mut findings = Findings::default();
    for (i, dialect) in legality_passes.iter().copied().enumerate() {
        for err in lua::validate::validate(parse, dialect) {
            let range = to_range(err.range);
            let key = (err.code, range.start, range.end);
            // Counterfactual otherwise: a construct the *edition* accepts and
            // only the target rejects was still labelled "not legal in this
            // edition".
            let label = if i > 0 && !findings.contains(key) {
                format!("not legal on target {}", dialect.manifest_id())
            } else {
                "not legal in this edition".to_owned()
            };
            findings.record(
                key,
                Diagnostic::error(Code::new(err.code), err.message)
                    .with_label(Label::primary(Span::new(rel, range), label)),
            );
        }
    }
    // Held rather than emitted. Passes 2 and 3 are two legality *axes* over one
    // file, and a reader scanning a file's findings reads down the file, not
    // down luabox's pass list — so they are sorted together, below. Per-pass
    // sorting concatenated in pass order let an `LB0021` at line 13 render
    // after an `LB0013` at line 29 (Shockwave round 5).
    let mut legality: Vec<Diagnostic> = findings.into_source_order();

    // 3. Control-flow legality (#44): an unresolved `goto`, a repeated label,
    // or `break` outside a loop — code every reference Lua refuses to load,
    // whatever the edition. Skipped on a broken parse: the block structure
    // recovered around a missing `end` is a guess, and a legality verdict over
    // a guess is noise on top of the syntax error already reported.
    //
    // Run for the *same* dialect pair as pass 2, merged the same way.
    // The pass is not fully edition-independent: `checkrepeated` tightened in
    // 5.4 (`luabox_hir::validate::repeated_label_scope`), so `::a:: do ::a::
    // end` is legal source in a 5.2 project yet cannot load on a 5.4 ship
    // target. Checking only the edition let exactly that combination through
    // (Shockwave round 2).
    //
    // This lowers the file rather than reusing the HIR inside `artifacts`:
    // `luabox_types::FileArtifacts` keeps its lowering private. The same pass
    // runs off the lint engine's own lowering and the LSP's memoized one, so
    // all three frontends return the same verdict. Lowering is dialect-free,
    // so both passes read one lowering.
    if parse.errors().is_empty() {
        let lowered = luabox_hir::lower(parse);
        let control_flow_passes = dialect_passes(project.dialect, passes.control_flow);
        let mut findings = Findings::default();
        for (i, dialect) in control_flow_passes.iter().copied().enumerate() {
            for diag in luabox_hir::validate::control_flow(rel, &lowered, dialect) {
                let range = diag.primary_label().map_or(0..0, |l| l.span.range.clone());
                let key = (diag.code.number(), range.start, range.end);
                let diag = if i > 0 && !findings.contains(key) {
                    diag.with_note(format!(
                        "the project edition loads this; the ship target {} does not",
                        dialect.manifest_id()
                    ))
                } else {
                    diag
                };
                findings.record(key, diag);
            }
        }
        legality.extend(findings.into_source_order());
    }

    // Both legality axes, in one globally source-ordered run. The sort is
    // stable and each half arrives already sorted, so two findings at the same
    // span keep dialect-legality before loader-legality: the parser's verdict
    // is the one that explains the loader's.
    legality.sort_by_key(|diag| {
        diag.primary_label()
            .map_or((0, 0), |l| (l.span.range.start, l.span.range.end))
    });
    diags.extend(legality);

    // 4. Types against the ambient definition-package layer (SPEC.md §3),
    // with this file's resolved `require` exports in reach (#85).
    let requires = resolve_requires(artifacts, &project.root, project.build_target, exports);
    diags.extend(luabox_types::check_file_with_artifacts(
        parse,
        rel,
        project.strictness,
        project.dialect,
        Some(ambient),
        &requires,
        artifacts,
    ));
}

/// The dialects a legality pass runs for: the project edition, plus the ship
/// `--target` when it differs.
///
/// One helper for both legality passes (dialect, then control flow) so they
/// cannot drift apart: `--target` means "would this source be legal there?",
/// and a construct the target's *loader* rejects is no less a target problem
/// than one its *parser* rejects. A target equal to the edition adds nothing —
/// the pass would produce identical findings, deduplicated away — so it is not
/// pushed at all.
/// What identifies one legality finding across the edition and target runs:
/// its code and its primary range.
type FindingKey = (u16, usize, usize);

/// The findings of one legality pass, merged across the edition and target
/// runs and returned in source order.
///
/// Two runs reporting the same code at the same primary range are the same
/// *construct*, but not necessarily the same *verdict*:
/// `repeated_label_scope` makes the first-definition site of an `LB0021`
/// dialect-dependent, so `::a:: do ::a:: ::a:: end` at edition 5.2 with
/// `--target 5.4` produced two findings that named each other's spans — one
/// line reported as a duplicate *and* cited as the first definition
/// (Shockwave round 4). Keeping whichever ran first also meant the edition
/// always won, and the rendered order followed pass order rather than the
/// source.
///
/// So: the **later** run wins a key both fill — the ship target is the
/// dialect that decides whether the artifact loads — and the merged set is
/// sorted by span before it is rendered.
#[derive(Default)]
struct Findings {
    order: Vec<Diagnostic>,
    index: HashMap<FindingKey, usize>,
}

impl Findings {
    fn contains(&self, key: FindingKey) -> bool {
        self.index.contains_key(&key)
    }

    fn record(&mut self, key: FindingKey, diag: Diagnostic) {
        if let Some(&at) = self.index.get(&key) {
            self.order[at] = diag;
        } else {
            self.index.insert(key, self.order.len());
            self.order.push(diag);
        }
    }

    fn into_source_order(mut self) -> Vec<Diagnostic> {
        self.order.sort_by_key(|diag| {
            diag.primary_label()
                .map_or((0, 0), |l| (l.span.range.start, l.span.range.end))
        });
        self.order
    }
}

fn dialect_passes(edition: Dialect, target: Option<Dialect>) -> Vec<Dialect> {
    let mut passes = vec![edition];
    if let Some(target) = target
        && target != edition
    {
        passes.push(target);
    }
    passes
}

/// Map each static `require("mod")` the file names to the export type of the
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
    artifacts: &luabox_types::FileArtifacts,
    root: &Path,
    dialect: Dialect,
    exports: &HashMap<PathBuf, Ty>,
) -> HashMap<String, Ty> {
    let mut requires = HashMap::new();
    for module in artifacts.requires() {
        if let Some(target) = luabox_bundle::resolve_module(root, &module, dialect)
            && let Some(ty) = exports.get(&target)
        {
            requires.insert(module, ty.clone());
        }
    }
    requires
}

/// Harvest the type surfaces of the project's vendored luarocks tree, if it has
/// one (#30) — [`luabox_types::rocks::harvest`] over
/// [`layout::collect_rock_sources`].
///
/// The version directory is chosen by `[build] target` (the edition when unset),
/// exactly as [`resolve_requires`] chooses it, so the tree `check` harvests is
/// the tree the build resolves against. A project with no
/// `lua_modules/share/lua/<X.Y>/` directory pays one failed `is_dir` and gets an
/// empty result — the flat `lua_modules/<name>/` layout keeps its existing
/// `[dependencies]` + `[types] defs` path untouched.
///
/// A rock source that did not parse is dropped silently here — `check`'s output
/// is the project's findings and a vendored file the user did not write has no
/// business in it. The skipped labels are still reported where a debug-level
/// note belongs: the LSP names them in its log pane (`crate::lsp_cmd` →
/// `luabox_lsp`).
fn harvest_rocks(project: &Project, ambient: &Ambient) -> luabox_types::RockSurfaces {
    let version_dir = luabox_bundle::rocks_version_dir(project.build_target);
    let sources: Vec<luabox_types::RockModule> =
        layout::collect_rock_sources(&project.root, version_dir)
            .into_iter()
            .map(|source| luabox_types::RockModule {
                module: source.module,
                label: source.label,
                path: source.path,
                text: source.text,
            })
            .collect();
    // Per-file reduction is pure and independent, so it rides the same rayon
    // pool the source set does — a fully annotated 100-kLOC rock tree is
    // otherwise the largest single cost in a run. The fold is what orders the
    // surfaces (path order = precedence), so the parallel and sequential forms
    // give the identical result.
    let files: Vec<luabox_types::RockFile> = sources
        .par_iter()
        .map(|source| luabox_types::rocks::harvest_file(ambient, source))
        .collect();
    luabox_types::RockSurfaces::fold(&sources, files)
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
    errln!(
        "check: {} errors, {} warnings in {file_count} files",
        counts.errors,
        counts.warnings
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
    // These tests vary the manifest body itself (including malformed ones), so
    // they build the project from the text rather than from `(edition, extra)`.
    use crate::testutil::{manifest, project_with_manifest as project, write};

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
        // Exactly one error — the argument mismatch — and not, say, a second
        // one from the annotation itself.
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
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

    // -- cross-file export-seam reification (round 3 review F42) -----------

    #[test]
    fn a_cross_file_bare_generic_ancestor_erases_leniently_not_by_name() {
        // F42: the cross-file surface pre-pass computes every file's export
        // with a defs-only ambient (`ambient` at the top of `run_passes`,
        // *before* `with_project_types` merges the project's own classes in)
        // — required to build that merge without circularity. A generic
        // ancestor declared in a DIFFERENT file is therefore invisible to
        // `reify_export`'s unbound-parameter check when `sub.lua`'s own
        // surface is computed, so `---@class Sub : Base` (bare — `Base`'s
        // parameter unbound) crossed `require` as the unerased
        // `Ty::Named("Sub")` instead of the lenient structural template
        // every same-file bare-parent case already gets
        // (`cross_file_require::a_parent_written_bare_leaves_its_parameter_unbound_and_erased`).
        //
        // The probe: reading an undeclared member on the required carrier.
        // Erased-to-`unknown` is a structural table with no undefined-field
        // obligation, so it must be clean; the bug's telltale is the
        // opposite — `Sub` keeps its class identity, `nope` is enforced
        // against a shape that never declares it, and `LB0306` fires on
        // code that should be lenient exactly like the same-file case is.
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "src/base.lua",
            // `item` is optional so `Sub`'s carrier conformance (#107 — a
            // pre-existing, unrelated obligation: does `Sub`'s own Lua code
            // literally provide every non-optional inherited member? — it
            // does not here, since `S` is a bare table) imposes no
            // obligation, isolating the export-seam question this test is
            // actually about. Confirmed pre-existing by reproducing the same
            // conformance diagnostic on this exact shape checked SAME-FILE.
            "---@class Base<U>\n---@field item? U\nlocal M = {}\nreturn M\n",
        );
        write(
            tmp.path(),
            "src/sub.lua",
            "---@class Sub : Base\nlocal S = {}\nreturn S\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "local s = require(\"sub\")\nlocal _ = s.nope\n",
        );
        check(tmp.path(), None, Format::Human)
            .expect("an unbound cross-file ancestor's carrier must erase leniently, not enforce LB0306 on a name it never declared");
    }

    #[test]
    fn a_cross_file_bound_generic_ancestor_still_types_its_member() {
        // The one-variable control for the test above: the SAME two files,
        // with the parent reference BOUND (`Base<number>`) instead of bare.
        // A fix that makes the erasure check unconditionally lenient for any
        // cross-file ancestor (rather than correctly detecting "still
        // unbound") would pass the sibling test above by accident while
        // losing real type-checking here — `item` must resolve as `number`,
        // genuinely, not as `unknown`.
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "src/base.lua",
            // `item` optional — see the sibling test's comment (#107, unrelated).
            "---@class Base<U>\n---@field item? U\nlocal M = {}\nreturn M\n",
        );
        write(
            tmp.path(),
            "src/sub.lua",
            "---@class Sub : Base<number>\nlocal S = {}\nreturn S\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "---@param s string\nlocal function want(s) end\nlocal m = require(\"sub\")\nwant(m.item)\n",
        );
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error, "check failed with 1 error(s)",
            "`item` must be genuinely `number` (rejected against `string`), not leniently `unknown`"
        );
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
        // Both 5.1-illegal constructs are reported — the `::top::` label and
        // the `goto` — each once.
        let error = check(tmp.path(), Some("5.1"), Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 2 error(s)");
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

    /// The pass list both legality passes share: edition first, target only
    /// when it differs (Shockwave round 2 — control flow used to ignore it).
    #[test]
    fn the_dialect_pass_list_adds_the_target_only_when_it_differs() {
        assert_eq!(dialect_passes(Dialect::Lua52, None), vec![Dialect::Lua52]);
        assert_eq!(
            dialect_passes(Dialect::Lua52, Some(Dialect::Lua52)),
            vec![Dialect::Lua52]
        );
        assert_eq!(
            dialect_passes(Dialect::Lua52, Some(Dialect::Lua54)),
            vec![Dialect::Lua52, Dialect::Lua54]
        );
    }

    #[test]
    fn a_label_shadow_legal_in_the_edition_is_reported_for_a_5_4_target() {
        // `checkrepeated` tightened in 5.4: `::a:: do ::a:: end` loads on
        // 5.2/5.3/LuaJIT and is `label 'a' already defined` on 5.4
        // (`luac5.4 -p`). `--target 5.4` has to say so.
        for edition in ["5.2", "5.3", "luajit"] {
            let tmp = project(&manifest(edition, ""));
            write(tmp.path(), "src/main.lua", "::a:: do ::a:: end\n");
            check(tmp.path(), None, Format::Human)
                .unwrap_or_else(|e| panic!("legal in edition {edition}: {e}"));
            let error = check(tmp.path(), Some("5.4"), Format::Human)
                .unwrap_err()
                .to_string();
            assert_eq!(error, "check failed with 1 error(s)", "edition {edition}");
        }
    }

    #[test]
    fn a_looser_target_does_not_resurrect_a_finding_the_edition_cleared() {
        // The reverse direction: legal under 5.4, and 5.2's looser rule can
        // only accept more, so nothing is reported.
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", "do ::a:: end ::a::\n");
        check(tmp.path(), Some("5.2"), Format::Human).expect("legal under both");
    }

    #[test]
    fn a_control_flow_finding_in_both_passes_is_reported_once() {
        // `break` outside a loop is illegal in every edition, so both the
        // edition pass and the target pass produce it for the same range.
        let tmp = project(&manifest("5.1", ""));
        write(tmp.path(), "src/main.lua", "local x = 1 break\n");
        let with_target = check(tmp.path(), Some("5.4"), Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(with_target, "check failed with 1 error(s)");
    }

    #[test]
    fn a_goto_on_a_5_1_target_stays_the_dialect_finding_alone() {
        // 5.1 has no `goto` at all, so the control-flow pass skips goto/label
        // for it (`luabox_hir::validate::control_flow`) and the reader gets
        // the two `LB0010` dialect findings, not a third complaint.
        let tmp = project(&manifest("5.4", ""));
        write(
            tmp.path(),
            "src/main.lua",
            "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\n",
        );
        let error = check(tmp.path(), Some("5.1"), Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 2 error(s)");
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
        // unqualified check sees the broken emitted file — and it is that one
        // file's syntax error it trips on, nothing else...
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
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

    // -- types harvested from a bare luarocks tree (#30) --------------------
    //
    // The tree walk is `luabox_manifest::layout`'s and the surface harvest is
    // `luabox_types::rocks`'; both are tested there. These pin what `check`
    // adds: the wiring — no manifest declaration needed, project-side
    // declarations winning, and a rock body never being checked.

    /// An annotated rock installed the way luarocks installs one.
    const ROCK: &str = "\
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
";

    #[test]
    fn a_bare_rock_tree_types_a_require_with_no_manifest_declaration() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(tmp.path(), "lua_modules/share/lua/5.4/mylib/init.lua", ROCK);
        // No `[dependencies]`, no `[types] defs`, no `lua_modules/mylib/luabox.toml`.
        write(
            tmp.path(),
            "src/main.lua",
            "local mylib = require(\"mylib\")\nlocal p = mylib.point(1, 2)\nreturn p.x\n",
        );
        check(tmp.path(), None, Format::Human).expect("rock types resolve and check clean");
    }

    #[test]
    fn misusing_a_harvested_rock_type_is_reported_in_the_consumer() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(tmp.path(), "lua_modules/share/lua/5.4/mylib/init.lua", ROCK);
        // The rock's `---@return mylib.Point` flows through the `require`, so
        // both misuses are the consumer's: a field the rock's class does not
        // declare, and the class where a string is wanted.
        write(
            tmp.path(),
            "src/main.lua",
            "local mylib = require(\"mylib\")\n\
             local p = mylib.point(1, 2)\n\
             ---@param s string\n\
             local function want(s) end\n\
             want(p)\n\
             return p.nope\n",
        );
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 2 error(s)");
    }

    #[test]
    fn a_type_error_inside_a_rock_source_is_never_a_project_diagnostic() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        // Annotated (so it *is* harvested) and wrong (so it would fail if it
        // were checked): the rock misuses its own signature.
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/mylib/init.lua",
            &format!("{ROCK}\n---@type mylib.Point\nlocal bad = {{ x = \"nope\" }}\n"),
        );
        write(tmp.path(), "src/main.lua", "return 1\n");
        check(tmp.path(), None, Format::Human).expect("vendored bodies are never checked");
    }

    #[test]
    fn an_unparseable_rock_source_is_skipped_silently() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "lua_modules/share/lua/5.4/broken/init.lua",
            "---@class broken.Thing\nlocal = = =\n",
        );
        write(tmp.path(), "src/main.lua", "return 1\n");
        check(tmp.path(), None, Format::Human).expect("a broken rock cannot fail the project");
    }

    #[test]
    fn an_explicit_defs_declaration_wins_a_collision_with_a_harvested_rock_class() {
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"mylib\"]\n",
        ));
        write(tmp.path(), "lua_modules/share/lua/5.4/mylib/init.lua", ROCK);
        // The project's own def declares `mylib.Point` with only `x`. If the
        // rock's two-field version won, `{ x = 1 }` would be missing `y`.
        write(
            tmp.path(),
            "defs/mylib.d.lua",
            "---@meta\n---@class mylib.Point\n---@field x number\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "---@type mylib.Point\nlocal p = { x = 1 }\nreturn p\n",
        );
        check(tmp.path(), None, Format::Human).expect("explicit defs win — `x` alone is complete");
    }

    #[test]
    fn a_rock_tree_for_another_version_directory_is_not_harvested() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        // Installed for 5.1; this project targets 5.4, so the tree is not
        // this build's and `require` would not resolve into it either.
        write(tmp.path(), "lua_modules/share/lua/5.1/mylib/init.lua", ROCK);
        write(
            tmp.path(),
            "src/main.lua",
            "---@type mylib.Point\nlocal p = { x = 1, y = 2 }\nreturn p\n",
        );
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        // LB0305 — the class is undeclared, exactly as before #30.
        assert!(error.contains("check failed with"), "{error}");
    }

    #[test]
    fn an_explicit_dependencies_entry_alongside_a_rock_tree_neither_breaks_nor_doubles() {
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\n\n[dependencies]\nmylib = \"1.0\"\n",
        ));
        write(tmp.path(), "lua_modules/share/lua/5.4/mylib/init.lua", ROCK);
        write(
            tmp.path(),
            "src/main.lua",
            "---@type mylib.Point\nlocal p = { x = 1, y = 2 }\nreturn p\n",
        );
        // The entry finds no `lua_modules/mylib/luabox.toml`, so it contributes
        // no defs; the harvest supplies the class once. No LB0307.
        check(tmp.path(), None, Format::Human).expect("declared dependency plus harvest is clean");
    }

    #[test]
    fn a_project_file_shadowing_a_rock_module_keeps_its_own_export_type() {
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(tmp.path(), "lua_modules/share/lua/5.4/mylib.lua", ROCK);
        // `<root>/src/mylib.lua` is earlier in the resolution order than the
        // rock tree, so `require("mylib")` is this file — with no `point`.
        write(
            tmp.path(),
            "src/mylib.lua",
            "local M = {}\n---@return string\nfunction M.shadow() return \"me\" end\nreturn M\n",
        );
        write(
            tmp.path(),
            "src/main.lua",
            "local mylib = require(\"mylib\")\nreturn mylib.shadow()\n",
        );
        check(tmp.path(), None, Format::Human).expect("the project file wins resolution");
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
