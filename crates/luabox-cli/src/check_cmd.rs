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

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use luabox_diag::{Code, Diagnostic, Format, Label, Severity, Span};
use luabox_manifest::layout::{self, DefFiles, DefSource};
use luabox_manifest::model::{Build, DialectId, Manifest};
use luabox_syntax::{Dialect, LineIndex, lua, luacats};
use luabox_types::ty::Ty;
use luabox_types::{
    Ambient, DefFile, MAX_ANCESTRY_DEPTH, Strictness, build_ambient_checked, stdlib_defs,
};
use rayon::prelude::*;

use layout::display_rel;

use crate::emit::errln;

/// What a manifest-less directory is checked as: Lua 5.4, warn mode — least
/// surprise. Held in both vocabularies because a manifest-less project still
/// needs a `[build]` config (`Build::defaults`) to build with.
const DEFAULT_DIALECT_ID: DialectId = DialectId::Lua54;
const DEFAULT_DIALECT: Dialect = Dialect::Lua54;

// The class-ancestry-ledger codes this file names on its own account: the
// syntactic `LB0317` pre-check ([`deep_class_chain_diagnostics`]) and the
// cross-file dedup below both need to name them. Re-exported from
// `luabox-types` rather than re-declared as `Code::new(317)` literals here
// (local merge-gate finding G4): a code number written on both sides of a
// crate boundary is one constant with two owners and nothing to catch a
// renumbering, which is exactly how the *suppression-name* half of this
// same pair drifted once already.
use luabox_types::{
    CLASS_COST_LIMIT as LB0319_CLASS_ANCESTRY_TOO_COSTLY,
    CLASS_DEPTH_LIMIT as LB0317_CLASS_ANCESTRY_TOO_DEEP,
    CYCLIC_CLASS as LB0318_CYCLIC_CLASS_ANCESTRY,
};

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
    let (diags, file_count) = collect_diagnostics(project, passes)?;
    finish(&diags, format, &project.root, file_count)
}

/// [`run_passes`]'s diagnostic-gathering half, split out so a caller that
/// wants the raw [`Diagnostic`]s themselves — `crate::build_cmd`'s check
/// gate does not (it wants `run_passes`'s exit-code translation via
/// [`finish`]), but the test suite does, to assert on which codes/messages a
/// project produces rather than only on pass/fail — does not have to
/// re-implement this whole pipeline (discovery, ambient assembly, the
/// cross-file surface pre-pass, per-file check, cross-file dedup) a second
/// time just to get diagnostics back instead of an exit code. `run_passes`
/// itself is now exactly this plus [`finish`], so the two can never drift
/// apart the way a hand-duplicated test-only copy of this pipeline would.
fn collect_diagnostics(
    project: &Project,
    passes: TargetPasses,
) -> anyhow::Result<(Vec<Diagnostic>, usize)> {
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

    // A `---@class` chain past the ancestry ceiling is reported HERE,
    // syntactically, before the surface pass below resolves anything — see
    // `deep_class_chain_diagnostics`. It no longer *short-circuits* the run:
    // it used to `return` the moment it found anything, which silently
    // disabled every other pass in the command (#61 local merge-gate, the
    // short-circuit finding) — a 201-link chain in one file hid the syntax errors in
    // another, an `Error`-severity `LB1002` was discarded because a
    // `Warning`-severity depth diagnostic got there first, and adding a
    // `---@diagnostic disable` comment therefore made `check` report MORE
    // findings, not fewer. The pinned-stack protection that early return was
    // reasoned from is not this pre-check's to give: `MAX_ANCESTRY_DEPTH`'s
    // `DiamondGuard` cap refuses to recurse past 200 links inside the
    // resolution walk itself, for every caller. Re-measured on this branch
    // before the early return was deleted, with the full pipeline running
    // over the chain: a 2,500-link chain and a 50,000-link chain (the latter
    // with `---@type C50000` + a field read, so the resolver actually walks
    // it) both complete and report, no SIGABRT.
    let chains = deep_class_chain_diagnostics(&files, project.strictness);

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
    // via `scripts/peak-rss.py`, release build) — the same table
    // `build_file_env`'s doc comment carries and `scripts/perf-gate.sh`'s
    // "RETAINED-TYPEENV REGRESSION GATE" leg re-measures on every CI run:
    //
    //   N      2 builds/file (transient)   1 build/file (R14, retained)
    //   100    11 MiB                       51 MiB   (4.6x)
    //   200    15 MiB                      145 MiB   (9.3x)
    //   300    20 MiB                      290 MiB  (14.5x)
    //   500    29 MiB                      734 MiB  (25.3x)
    //
    // A ~25x memory blowup at N=500 for a ~1.76x CPU win is the wrong side
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
    // One index for the whole run, not a linear scan per lookup: `check_one`
    // resolves a *foreign* file's source when a cross-file ancestry
    // diagnostic has to be suppression-checked against that file's own
    // directives, and it runs once per project file inside this `par_iter`.
    // Scanning `files` on each lookup made that O(F) inside an O(F) loop —
    // quadratic on exactly the many-files-one-shared-deep-hierarchy shape
    // the cross-file suppression fix exists to serve (local merge-gate
    // re-run finding). Built once here, borrowed by every worker.
    let by_rel: HashMap<&str, &SourceFile> = files.iter().map(|f| (f.rel.as_str(), f)).collect();
    let per_file: Vec<Vec<Diagnostic>> = files
        .par_iter()
        .map(|file| {
            let mut diags = Vec::new();
            check_one(
                file, &by_rel, ambient, project, passes, &exports, &mut diags,
            );
            diags
        })
        .collect();
    let mut diags: Vec<Diagnostic> = def_diags;
    for file_diags in per_file {
        diags.extend(file_diags);
    }
    merge_depth_chain_diagnostics(&mut diags, chains);
    // A cross-file `---@class` cycle is one project-wide finding, not one
    // per file that happens to resolve it: `luabox_types::TypeEnv` is built
    // fresh per file (deliberately — see `build_file_env`'s doc comment), so
    // every file whose own check pass resolves a class caught in an N-file
    // cycle independently rediscovers the WHOLE cycle and reports every
    // member of it, not just its own (production readiness review G2). A
    // 2-class mutual cycle split across two files therefore emitted 4
    // `LB0318`s where the identical cycle written in one file emits 2.
    // Deduped here, after every file's diagnostics have converged into one
    // list — the one place that can see across files — by `(code, primary
    // span, message)`: `TypeEnv::class_decl_span`/`cross_file_class_decl_span`
    // always resolve a given class name to the SAME declaration span
    // regardless of which file's pass queried it, so that key collapses every
    // file's rediscovery back to the one true diagnostic per class. The
    // message is in the key because the ambient tier has no distinguishing
    // span at all — see [`dedupe_ancestry_diagnostics`]'s own doc comment for
    // the measurement. `LB0317` (the depth-limit sibling) shares the same
    // per-file-TypeEnv architecture and so shares the same theoretical
    // exposure — confirmed by test, not merely assumed — so it rides the same
    // dedup rather than only the `LB0318` case that is guaranteed to hit it.
    dedupe_ancestry_diagnostics(&mut diags);
    sort_for_render(&mut diags);

    Ok((diags, lua_files.len()))
}

/// Fold the syntactic pre-check's `LB0317`s into the converged diagnostic
/// list, so an over-limit chain costs ONE diagnostic across both mechanisms
/// that can see it.
///
/// The resolver-side drain (`luabox_types::check::report_depth_limit_hits`)
/// reports whichever class's resolution tripped the guard, in different words
/// and — measured on a 400-link chain with `---@type C400` — at a different
/// class of the SAME chain: the pre-check's `C200` and the drain's `C400`.
/// Neither [`dedupe_ancestry_diagnostics`]'s `(code, span, message)` key nor
/// an aligned message would collapse those. [`DeepChains::covered`] carries
/// every over-limit class's declaration site, so the drain's rediscoveries of
/// a chain the pre-check already named are dropped and the pre-check's own —
/// the first link over the limit, the actionable one — is what survives.
///
/// Deliberately unconditional on suppression: a `---@diagnostic disable` that
/// silences the pre-check silences the whole chain, rather than silencing the
/// first link and leaving the drain's copy of the same chain standing. One
/// rule disagreeing with itself across two mechanisms is exactly what the
/// production readiness review's finding 3 was about.
///
/// A class declared only in an ambient `[types] defs` package has no
/// in-project declaration for the pre-check to see (it scans project files
/// only, `DefFiles::Exclude`), so it is never covered and the drain's
/// diagnostic for it is untouched — that tier is the one thing keeping
/// `luabox check` from going green on a project the LSP reports red on
/// (round 6 review M5).
fn merge_depth_chain_diagnostics(diags: &mut Vec<Diagnostic>, chains: DeepChains) {
    if !chains.covered.is_empty() {
        diags.retain(|d| {
            d.code != LB0317_CLASS_ANCESTRY_TOO_DEEP || !chains.covers(d.primary_label())
        });
    }
    diags.extend(chains.diags);
}

/// The order `check` renders its findings in: by file, then by position in
/// that file, then — for findings that genuinely share a span — by code and
/// message.
///
/// The last two keys are not decoration. `luabox_types`' ancestry drains
/// attribute a class with no in-project declaration to `Span::new(file, 0..0)`
/// (`check::report_ancestry_hits`'s third arm), so every ambient-tier hit in
/// one file carries the IDENTICAL span, and the ledger they are drained from
/// is hash-ordered. Sorting on `range.start` alone therefore left their
/// relative order down to hashmap iteration: measured on a project with two
/// over-limit `[types] defs` chains, 12 consecutive runs of the same binary
/// over the same unchanged sources printed the two `LB0317`s in one order 10
/// times and the other order twice. A diff-based CI gate cannot tell that
/// apart from a real change (production readiness review, finding 5). Code
/// and message are what remain to separate two findings at one span, and both
/// are deterministic.
///
/// Applied once, here, over the whole converged list — not per pass and not
/// per file. `finish`'s only other caller passes a single diagnostic, which is
/// already sorted.
fn sort_for_render(diags: &mut [Diagnostic]) {
    /// Where a diagnostic points, borrowed rather than cloned: this runs
    /// `O(n log n)` times, and the file name is the whole key's first
    /// component. A project-assembly finding carries no label at all
    /// (`LB1002`, `LB0307`) and sorts to the front, where it already
    /// rendered.
    fn place(diag: &Diagnostic) -> (&str, usize, usize) {
        diag.primary_label().map_or(("", 0, 0), |l| {
            (l.span.file.as_str(), l.span.range.start, l.span.range.end)
        })
    }
    diags.sort_by(|a, b| {
        place(a)
            .cmp(&place(b))
            .then_with(|| a.code.number().cmp(&b.code.number()))
            .then_with(|| a.message.cmp(&b.message))
    });
}

/// Hard ceiling on a `---@class` inheritance chain, checked syntactically —
/// from each file's already-harvested annotations
/// ([`luabox_types::FileArtifacts::items`], not a fresh
/// [`luacats::harvest`] — see the round 6 review M19 note below), not the
/// type env — by [`deep_class_chain_diagnostics`], before
/// [`luabox_types::module_surface_with_artifacts`] gets anywhere near it.
///
/// Round 5 review N2 originally gave this its own, looser ceiling (2,000),
/// reasoned from this CLI's *own* pinned-thread crash floor. That stopped
/// being the right number the day `luabox-types` grew a durable, general fix
/// for the same finding: `env::MAX_ANCESTRY_DEPTH`'s `DiamondGuard` cap,
/// which refuses to recurse past 200 links inside the resolution walk itself
/// — unconditionally, for every caller, not just this CLI's pinned
/// dispatcher thread. A chain between the two numbers (200 < depth ≤ 2,000)
/// used to sail past this pre-check's "fine" verdict and then have its shape
/// silently truncated the moment anything resolved it, which is exactly
/// backwards: a check that certifies a depth as safe must not be looser than
/// the resolver that actually walks it (production readiness review, finding
/// 1). This now imports [`luabox_types::MAX_ANCESTRY_DEPTH`] rather than
/// keeping its own figure, so the two can no longer drift apart — one number,
/// enforced twice: cheaply and syntactically here, before any class is
/// resolved (and even for a chain nothing in the project ever references —
/// see below); durably inside the resolver itself for every caller,
/// including ones this pre-check never runs ahead of (the LSP request path,
/// an embedder calling `luabox_lsp::run_stdio` directly).
///
/// [`MAX_ANCESTRY_DEPTH`]'s check: **the first link over the limit in every
/// over-limit chain** — the class whose own parent is still within bounds,
/// which is the one place a reader can act on. Round 6 review M17 replaced a
/// `max_by_key` that let `HashMap` iteration order pick one arbitrary chain
/// and silently dropped every other; the fix reported every over-limit
/// *class*, which on a single long chain is a different failure: one 2,500-link
/// chain produced 2,300 separate errors and 835 KB of output, all of them
/// restating one mistake (round 6 review M17, re-measured by the #61 local
/// merge-gate's per-link-flood finding). Reporting the frontier keeps M17's actual
/// requirement — N independent chains still produce N diagnostics, none of
/// them dropped — while a chain costs one diagnostic however long it is. The
/// message carries the measured total: the chain's deepest class and its
/// depth. Sorted by declaring span, so the order is deterministic regardless
/// of hashmap or harvest order.
///
/// Also returned: the declaration spans of every over-limit class, not only
/// the reported frontier ones ([`DeepChains::covered`]). That is what lets
/// [`collect_diagnostics`] drop the resolver-side drain's own `LB0317` for a
/// chain already named here, so one chain is one diagnostic across BOTH
/// mechanisms.
///
/// Empty when every declared chain is within bounds.
///
/// A class declared with an empty name — what a bare `---@class` typo
/// harvests as — is skipped outright, mirroring the `!c.name.is_empty()`
/// guards `luabox_types::env`'s own harvest sites apply. Folding those into
/// one synthetic `""` node made unrelated files collide: measured, a bare
/// `---@class` in `a.lua` won the first-wins `parents_of[""]` entry with an
/// empty parent list, which pinned `""`'s depth at 0 and made a genuinely
/// over-limit chain ending in `---@class : C200` in `z.lua` VANISH from the
/// report entirely (production readiness review, finding 4). The same
/// collapse also mis-attributed one file's chain to another file's typo, and
/// produced a diagnostic whose message named the class as `` ` ` ``.
///
/// Each diagnostic carries the same code as the durable resolver-side guard
/// (`LB0317`, [`Code::new`]`(317)` — round 6 review M4(b): this pre-check
/// used to hard-code `LB0001` "syntax error", so `luabox explain` on the
/// code it actually printed described a parser failure, not an ancestry
/// limit) and the same severity ladder every other type diagnostic gets —
/// `Error` under `[types] strict = true`, `Warning` otherwise, nothing at
/// all under [`Strictness::None`] — rather than the fixed `Error` the old
/// hard-coded [`Diagnostic::error`] call gave unconditionally regardless of
/// the manifest (M4(b): "`[types] strict = false` does not downgrade it").
/// `---@diagnostic disable[-line|-next-line]: class-ancestry-too-deep` in
/// the declaring file suppresses one chain's diagnostic the same way
/// (M4(b)'s suppression half) — through [`luabox_types::DirectiveScan`]
/// itself, the same scanner the checker-side `LB0317` is filtered by, rather
/// than a look-alike of it (see [`suppressed`]).
///
/// A parent expressed as anything other than a bare name (a union, a table
/// literal, ...) does not extend a chain this walk follows — the heuristic
/// under-counts rather than duplicates `luabox-types`' own resolution.
/// **Every** named parent is walked, not just a lone one (round 6 review
/// M4(c): the previous `if let [parent] = class.parents.as_slice()` treated
/// any multi-parent class as a root, so the identical depth flipped from
/// `error`/exit 1 to `warning`/exit 0 the moment a second, unrelated parent
/// was added) — [`class_depths`] folds every parent's own depth into a
/// class's, so a multi-parent class's depth is `1 +` the deepest of its
/// parents, matching what [`DiamondGuard`]'s real `on_path` walk would do to
/// the same shape (`crate::codes::CLASS_DEPTH_LIMIT`'s doc comment).
/// Depth does not require the chain to be *used* anywhere in the file —
/// declaring it is already enough to make `luabox-types` walk it once
/// something in the project resolves any class near the bottom, and a
/// declared-but-dead chain this deep is not a real project shape worth
/// letting through to find out.
///
/// Depth is computed iteratively — an explicit stack, not a recursive walk
/// — precisely so this check cannot itself overflow on the input it exists
/// to reject; see [`class_depths`].
fn deep_class_chain_diagnostics(files: &[SourceFile], strictness: Strictness) -> DeepChains {
    // `Strictness::None` disables every type diagnostic project-wide
    // (`luabox_types`'s own `strictness != Strictness::None` gate, `lib.rs`)
    // — `LB0317` is one of them, so this pre-check honors the same gate
    // rather than scanning for a result nothing would ever report.
    if strictness == Strictness::None {
        return DeepChains::default();
    }
    let severity = if strictness == Strictness::Strict {
        Severity::Error
    } else {
        Severity::Warning
    };

    let ClassGraph {
        declared_at,
        parents_of,
    } = harvest_class_graph(files);
    let depth = class_depths(&parents_of);
    // [`class_depths`] counts EDGES to the deepest root; the limit counts
    // CLASSES on the resolution path, and `DiamondGuard`'s path includes the
    // class being resolved (`on_path.len() >= MAX_ANCESTRY_DEPTH` refuses the
    // class that would take the path PAST the limit, so its first trip is a
    // chain of `MAX_ANCESTRY_DEPTH + 1` classes —
    // `env::tests::a_class_chain_one_class_past_the_ancestry_limit_already_truncates`).
    // Converting to the limit's own unit here is what makes this module's
    // "one number, enforced twice" claim true (PR #61 round-8 F3): comparing
    // the raw edge count with `>` put this side's first trip at
    // `MAX_ANCESTRY_DEPTH + 2` classes, one whole chain size later than the
    // resolver's. At exactly `MAX_ANCESTRY_DEPTH + 1` classes WITH a live
    // reference, this pre-check certified the chain as in-bounds, `covered`
    // stayed empty, and the resolver's non-actionable "`C200`'s ancestry is
    // too deep to resolve safely" reached the user unsuppressed in place of
    // the actionable first-link report — measured at 825c61c by
    // `the_smallest_over_limit_chain_reports_the_pre_checks_actionable_link_once`.
    let chain_classes = |name: &str| depth.get(name).map(|&d| d + 1);
    let over_limit = |name: &str| chain_classes(name).is_some_and(|c| c > MAX_ANCESTRY_DEPTH);
    // Every over-limit class's declaration site, whether or not it is the
    // link this reports: that is what `collect_diagnostics` matches the
    // resolver-side drain's own `LB0317` against.
    let covered: HashSet<(String, usize, usize)> = depth
        .keys()
        .filter(|name| over_limit(name))
        .filter_map(|name| {
            let &(file_idx, span) = declared_at.get(name)?;
            Some((files[file_idx].rel.clone(), span.start, span.end))
        })
        .collect();

    // The FIRST link over the limit in each chain: over-limit, but with no
    // over-limit parent. One diagnostic per chain however long that chain
    // runs, and still one per chain when a project declares several — round 6
    // review M17's actual requirement, which the "every over-limit class"
    // reading of it overshot.
    let mut frontier: Vec<&str> = depth
        .keys()
        .map(String::as_str)
        .filter(|name| {
            over_limit(name)
                && !parents_of
                    .get(*name)
                    .into_iter()
                    .flatten()
                    .any(|parent| over_limit(parent))
        })
        .collect();
    if frontier.is_empty() {
        return DeepChains {
            diags: Vec::new(),
            covered,
        };
    }
    // Deterministic regardless of the `HashMap` iteration order above (M17):
    // sort by declaring span, matching the whole-file sort
    // `luabox_types::check::run` already applies to its own `LB0317`
    // diagnostics.
    frontier.sort_by_key(|name| declared_at.get(*name).map(|&(idx, span)| (idx, span.start)));

    let deepest = deepest_descendants(&parents_of, &depth);
    // One [`luabox_types::DirectiveScan`] + [`LineIndex`] per file that has a
    // reported chain, not one per class: both are O(file size) to build, and
    // the old shape rebuilt them for every over-limit class — 2,300 rebuilds
    // of one 2,500-line file on the per-link-flood fixture below.
    let mut directives: HashMap<usize, (luabox_types::DirectiveScan, LineIndex)> = HashMap::new();
    let diags = frontier
        .into_iter()
        .filter_map(|name| {
            let &(file_idx, span) = declared_at.get(name)?;
            let file = &files[file_idx];
            let (rules, lines) = directives.entry(file_idx).or_insert_with(|| {
                let text = file.parse.syntax().text().to_string();
                (
                    luabox_types::DirectiveScan::scan(&text),
                    LineIndex::new(&text),
                )
            });
            if suppressed(rules, lines, span) {
                return None;
            }
            let link_classes = chain_classes(name)?;
            // The measured total this link commits the chain to, not just
            // this link's own depth: a reader who flattens here needs to know
            // how far below the limit the chain still runs. Same edge->class
            // conversion as `over_limit`, so every number in the message is
            // in the unit the limit is stated in.
            let tail = match deepest.get(name) {
                Some(&(deepest_depth, deepest_name)) if deepest_name != name => format!(
                    "; it is the first link over the limit in a chain that runs \
                     {} classes deep, down to `{deepest_name}`",
                    deepest_depth + 1
                ),
                _ => String::new(),
            };
            Some(
                Diagnostic::new(
                    LB0317_CLASS_ANCESTRY_TOO_DEEP,
                    severity,
                    format!(
                        "`{name}`'s `---@class` ancestry is {link_classes} classes deep, over \
                         the {MAX_ANCESTRY_DEPTH}-class limit this checker enforces to avoid a \
                         stack overflow while resolving it{tail} — flatten the hierarchy or use \
                         composition instead of a long inheritance chain"
                    ),
                )
                .with_label(Label::primary(
                    Span::new(file.rel.as_str(), span.start..span.end),
                    "first class over the limit",
                )),
            )
        })
        .collect();
    DeepChains { diags, covered }
}

/// What [`deep_class_chain_diagnostics`] hands back: the diagnostics it wants
/// rendered, and the declaration sites of every over-limit class it saw —
/// including the ones it deliberately did not report, since those are exactly
/// the classes the resolver-side `LB0317` drain would name for a chain this
/// pre-check has already covered.
#[derive(Default)]
struct DeepChains {
    diags: Vec<Diagnostic>,
    /// `(file, decl start, decl end)` per over-limit class.
    covered: HashSet<(String, usize, usize)>,
}

impl DeepChains {
    /// Whether `label` points at the declaration of a class this pre-check
    /// already accounted for.
    fn covers(&self, label: Option<&Label>) -> bool {
        label.is_some_and(|l| {
            self.covered
                .contains(&(l.span.file.clone(), l.span.range.start, l.span.range.end))
        })
    }
}

/// For every class in `depth`, the deepest class reachable *below* it along
/// declared `---@class` parent edges, as `(that class's depth, its name)` —
/// the measured total [`deep_class_chain_diagnostics`] puts in the message of
/// the one link it reports per chain.
///
/// Computed by relaxing `depth` in descending order rather than by a second
/// graph walk: a class's depth is `1 +` its deepest parent's, so a child is
/// always processed before the parent it feeds, and one pass over the sorted
/// nodes suffices. The `(depth, name)` maximum makes the answer independent
/// of `HashMap` order when two branches tie.
fn deepest_descendants<'a>(
    parents_of: &'a HashMap<String, Vec<String>>,
    depth: &'a HashMap<String, usize>,
) -> HashMap<&'a str, (usize, &'a str)> {
    let mut by_depth: Vec<(&str, usize)> = depth.iter().map(|(n, &d)| (n.as_str(), d)).collect();
    // Name breaks the tie so the walk order — and so the reported deepest
    // class of a tied chain — cannot depend on hashmap iteration.
    by_depth.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut deepest: HashMap<&str, (usize, &str)> =
        by_depth.iter().map(|&(n, d)| (n, (d, n))).collect();
    for (name, _) in by_depth {
        let Some(&reach) = deepest.get(name) else {
            continue;
        };
        for parent in parents_of.get(name).into_iter().flatten() {
            if let Some(slot) = deepest.get_mut(parent.as_str())
                && *slot < reach
            {
                *slot = reach;
            }
        }
    }
    deepest
}

/// Every `---@class` the project declares, as the two maps
/// [`deep_class_chain_diagnostics`] walks: name -> (declaring file index,
/// declaring span), and name -> every named parent (a bare `Name`, not a
/// union/table/generic-argument expression). First declaration of a name
/// wins, matching this module's other project-wide merges
/// (`with_project_types`'s "defs win").
///
/// An empty name is not a declaration at all and is dropped on both sides —
/// see [`deep_class_chain_diagnostics`]'s doc comment for what folding every
/// bare `---@class` typo in a project into one synthetic node cost.
fn harvest_class_graph(files: &[SourceFile]) -> ClassGraph {
    let mut declared_at: HashMap<String, (usize, luacats::Span)> = HashMap::new();
    let mut parents_of: HashMap<String, Vec<String>> = HashMap::new();
    for (file_idx, file) in files.iter().enumerate() {
        for item in file.artifacts.items() {
            for tag in &item.block.tags {
                let luacats::Tag::Class(class) = tag else {
                    continue;
                };
                if class.name.is_empty() {
                    continue;
                }
                declared_at
                    .entry(class.name.clone())
                    .or_insert((file_idx, class.span));
                parents_of.entry(class.name.clone()).or_insert_with(|| {
                    class
                        .parents
                        .iter()
                        .filter_map(|parent| match &parent.kind {
                            luacats::TypeExprKind::Named { name, .. } if !name.is_empty() => {
                                Some(name.clone())
                            }
                            _ => None,
                        })
                        .collect()
                });
            }
        }
    }
    ClassGraph {
        declared_at,
        parents_of,
    }
}

/// The project's declared `---@class` inheritance graph, as
/// [`harvest_class_graph`] reads it off the harvested annotations.
struct ClassGraph {
    /// Class name -> the file index and span of its first declaration.
    declared_at: HashMap<String, (usize, luacats::Span)>,
    /// Class name -> the names of every parent it declares by bare name.
    parents_of: HashMap<String, Vec<String>>,
}

/// The ancestor-chain depth of every class named as a key or a value in
/// `parents_of` — `depth(root, no named parent) = 0`, otherwise `1 +` the
/// deepest of its own named parents' depths — computed with an explicit
/// stack (a two-phase "expand, then fold" marker per node, the standard
/// iterative-postorder-DFS shape), not native recursion, so a
/// pathologically deep OR wide declared hierarchy cannot overflow the
/// resolving thread's own stack: reintroducing that failure mode via the
/// walk written to reject it would be exactly backwards (round 6 review
/// M4(c)'s multi-parent fix must not cost this pre-check the "explicit
/// stack, not a recursive walk" property its single-parent predecessor already
/// had). A parent cycle (its own, separately reported error, `env.rs`'s
/// `DiamondGuard`) stops the walk at the back-edge — recorded as depth 0 —
/// rather than looping forever.
fn class_depths(parents_of: &HashMap<String, Vec<String>>) -> HashMap<String, usize> {
    let mut depth: HashMap<String, usize> = HashMap::new();
    for start in parents_of.keys() {
        if depth.contains_key(start) {
            continue;
        }
        let mut stack: Vec<(&str, bool)> = vec![(start.as_str(), false)];
        let mut on_stack: HashSet<&str> = HashSet::new();
        while let Some((name, expanded)) = stack.pop() {
            if depth.contains_key(name) {
                continue;
            }
            if expanded {
                let d = parents_of
                    .get(name)
                    .into_iter()
                    .flatten()
                    .map(|parent| depth.get(parent.as_str()).copied().unwrap_or(0) + 1)
                    .max()
                    .unwrap_or(0);
                depth.insert(name.to_owned(), d);
                on_stack.remove(name);
            } else if on_stack.contains(name) {
                // A true cycle on this path: stop here rather than loop
                // forever. `crate::codes`/env.rs' own cycle guard reports it
                // separately; this walk only needs to not hang on it.
                depth.insert(name.to_owned(), 0);
            } else {
                on_stack.insert(name);
                stack.push((name, true));
                for parent in parents_of.get(name).into_iter().flatten() {
                    if !depth.contains_key(parent.as_str()) {
                        stack.push((parent.as_str(), false));
                    }
                }
            }
        }
    }
    depth
}

/// The `---@diagnostic disable[-line|-next-line]: class-ancestry-too-deep`
/// suppression [`deep_class_chain_diagnostics`] honors for the class
/// declared at `span` in `file` (round 6 review M4(b) — the old hard-coded
/// `Diagnostic::error` bypassed every suppression mechanism).
///
/// Both the rule *name* and the *scanner* come from `luabox-types`, the one
/// owner: `RULE_CLASS_ANCESTRY_TOO_DEEP` is the string the checker-side
/// `LB0317` filter also keys on, and [`luabox_types::DirectiveScan`] is the
/// scanner that filter also runs.
///
/// This used to be a hand-rolled walk over already-harvested
/// `Tag::Diagnostic` bodies, on the premise that "no `TypeEnv` exists yet at
/// this point in the pipeline for `DirectiveScan` to run against". That
/// premise was false — [`luabox_types::DirectiveScan::scan`] takes a `&str`
/// and nothing else — and the copy recognised a narrower comment grammar than
/// the original: measured, `--[[@diagnostic disable: class-ancestry-too-deep]]`
/// and a plain `--@diagnostic disable: class-ancestry-too-deep` each silenced
/// the checker-side `LB0317` and left this one standing, because the harvester
/// only yields a `Tag::Diagnostic` for the `---@` form (production readiness
/// review, finding 3). One rule, one scanner, one answer.
fn suppressed(
    rules: &luabox_types::DirectiveScan,
    lines: &LineIndex,
    declared: luacats::Span,
) -> bool {
    rules.suppresses(
        luabox_types::RULE_CLASS_ANCESTRY_TOO_DEEP,
        lines.line_of(declared.start),
    )
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
/// retained `Vec<TypeEnv>` was an O(N²)-peak-memory regression, ~25x at
/// N=500 on the benchmark project, for a ~1.76x CPU win).
fn check_one(
    file: &SourceFile,
    by_rel: &HashMap<&str, &SourceFile>,
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
    // A resolver from project-relative file name to that file's source text
    // (G1): a `---@class` cycle/depth-limit diagnostic can carry a primary
    // label belonging to a DIFFERENT project file than `rel` — the file that
    // actually declares the offending class, when it is not the file whose
    // check pass tripped the resolver's guard — and `---@diagnostic disable`
    // only suppresses it correctly when checked against THAT file's own
    // directives and line numbers (`check_file_with_artifacts_and_sources`'s
    // doc comment). Every project file's parse is already retained for the
    // length of this run (CC-M1), so this materialises one file's source on
    // demand rather than every file's up front — cross-file suppression is
    // the rare case, not the common one — and the lookup itself is a hash
    // hit against the index `run_passes` built once for the whole run.
    let source_of = |name: &str| -> Option<String> {
        by_rel
            .get(name)
            .map(|f| f.parse.syntax().text().to_string())
    };
    diags.extend(luabox_types::check_file_with_artifacts_and_sources(
        parse,
        rel,
        project.strictness,
        project.dialect,
        Some(ambient),
        &requires,
        artifacts,
        Some(&source_of),
    ));
}

/// Dedupe `LB0317`/`LB0318`/`LB0319` — one project file's `TypeEnv` cannot
/// see another file's, so a class-ancestry finding spanning several files (a
/// cross-file cycle, or a class several files resolve directly) is
/// independently rediscovered by every one of those files' own check passes
/// (production readiness review G2). Collapsing those rediscoveries back to
/// one diagnostic per class matches what the identical shape written in a
/// single file already reports.
///
/// The key is `(code, primary span, message)` — **including the message**,
/// which is the only place the class name survives into a `Diagnostic`.
/// Keying on the span alone was wrong for the ambient tier: a class with no
/// in-project declaration is attributed to `Span::new(file, 0..0)` — the
/// *consuming* file at offset zero, the same coordinates whichever class
/// tripped the guard (`check::report_ancestry_hits`'s third arm, added for
/// round 6 review M5). Two different over-budget `[types] defs` classes
/// consumed by one file therefore shared a key, and the second was dropped:
/// reproduced with two 260-deep ambient classes, where only the first
/// reported. A dropped diagnostic here is a **false negative in the type
/// checker**, silent by construction — strictly worse than the duplicate
/// this function exists to remove.
///
/// Run once here, after every file's diagnostics have converged
/// (`run_passes`) — no single file's check pass can do this dedup itself; it
/// never sees another file's findings.
fn dedupe_ancestry_diagnostics(diags: &mut Vec<Diagnostic>) {
    const ANCESTRY_CODES: [Code; 3] = [
        LB0317_CLASS_ANCESTRY_TOO_DEEP,
        LB0318_CYCLIC_CLASS_ANCESTRY,
        LB0319_CLASS_ANCESTRY_TOO_COSTLY,
    ];
    let mut seen: HashSet<(u16, String, usize, usize, String)> = HashSet::new();
    diags.retain(|d| {
        if !ANCESTRY_CODES.contains(&d.code) {
            return true;
        }
        let Some(label) = d.primary_label() else {
            return true;
        };
        seen.insert((
            d.code.number(),
            label.span.file.clone(),
            label.span.range.start,
            label.span.range.end,
            d.message.clone(),
        ))
    });
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

    /// The raw [`Diagnostic`]s `luabox check` would produce for `cwd` —
    /// [`collect_diagnostics`] under the same default [`TargetPasses`]
    /// [`run_once`] uses, skipping [`finish`]'s exit-code translation so a
    /// test can assert on codes/messages/counts directly rather than only on
    /// pass/fail.
    fn check_diagnostics(cwd: &Path) -> Vec<Diagnostic> {
        let project = discover(cwd).expect("discovers");
        let passes = TargetPasses {
            legality: None,
            control_flow: Some(project.build_target),
        };
        collect_diagnostics(&project, passes)
            .expect("collect_diagnostics")
            .0
    }

    /// [`check_diagnostics`] on a background thread with a hard wall-clock
    /// bound, for the k=190 conflicting-diamond fixtures: those finish fast
    /// ONLY because `MAX_ANCESTRY_RESOLUTIONS` trips. Under a mutation that
    /// stops the budget counter advancing, an unbounded check here is the
    /// round-5 exponential — the suite then HANGS and cargo-mutants reads a
    /// timeout instead of a kill (round 8, the one survivor the second
    /// mutants-pr run still reported). The bound makes a dead budget a fast
    /// red.
    fn check_diagnostics_bounded(cwd: &Path) -> Vec<Diagnostic> {
        let cwd = cwd.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check_diagnostics(&cwd));
        });
        rx.recv_timeout(std::time::Duration::from_secs(20)).expect(
            "a budget-capped k=190 diamond check must finish in seconds — an \
             unbounded run means the resolution budget never tripped",
        )
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

    // -- the class-chain depth ceiling (round 5 review N2) ------------------

    /// A single-file `---@class C0`, `---@class C1 : C0`, ..., `Cn : C(n-1)`
    /// chain of length `n`.
    fn class_chain_source(n: usize) -> String {
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        src
    }

    #[test]
    fn a_class_chain_past_the_depth_ceiling_is_a_clean_diagnostic_not_a_crash() {
        // Round 5 review N2: a single-parent `Cn : C(n-1)` chain recurses one
        // Rust stack frame per ancestor inside `luabox-types`' class-shape
        // resolution, and past whatever the pinned stack survives the whole
        // PROCESS aborts — no diagnostic, no file name, not even the nonzero
        // exit code `check` normally reports (see `deep_class_chain_diagnostics`'
        // doc comment for the measured floors that make this a real, not
        // hypothetical, failure mode).
        //
        // 2,500 sits well above `MAX_ANCESTRY_DEPTH` (200) but far below even
        // the *debug* binary's measured un-pinned crash floor (~990, see the
        // constant's own doc comment): unlike a probe at the actual crash
        // depth, this test stays a clean assertion failure — not a process
        // abort — if the guard below it ever regresses, so it is safe to run
        // in every future `cargo test` regardless of what changes around it.
        //
        // `strict = true`: round 6 review M4(b) — the diagnostic now follows
        // the same strictness ladder as every other type diagnostic, so a
        // clean error/exit-1 assertion needs strict mode explicit (see
        // `a_class_chain_past_the_ceiling_is_only_a_warning_outside_strict_mode`
        // for the warn-mode side).
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(tmp.path(), "src/main.lua", &class_chain_source(2_500));
        // ONE error: the first link over the limit. Round 6 review M17's fix
        // reported every over-limit class instead, which on a single chain
        // meant `2_500 - MAX_ANCESTRY_DEPTH` = 2,300 separate errors and
        // 835 KB of output for one mistake (#61 local merge-gate, the
        // per-link-flood finding). N independent chains still produce N diagnostics —
        // `three_over_limit_chains_are_all_reported_in_deterministic_source_order`
        // is the test that keeps this from regressing back to M17's
        // one-per-project `max_by_key`.
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
    }

    #[test]
    fn one_long_chain_reports_its_first_over_limit_link_and_its_measured_total_depth() {
        // Finding 2's other half: the single diagnostic must still carry the
        // measurement the 2,300 individually restated — where the chain
        // crosses the limit AND how far past it the chain actually runs, so
        // "flatten this" has a scope.
        let files = [source_file("main.lua", &class_chain_source(2_500))];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert_eq!(diags.len(), 1, "{diags:?}");
        let message = &diags[0].message;
        assert!(
            message.contains(&format!("`C{MAX_ANCESTRY_DEPTH}`")),
            "must name the first link over the limit — `C{}`, whose own path is \
             `MAX_ANCESTRY_DEPTH + 1` classes long, the smallest the limit refuses \
             (PR #61 round-8 F3 moved this one class up from `C{}`): {message}",
            MAX_ANCESTRY_DEPTH,
            MAX_ANCESTRY_DEPTH + 1
        );
        assert!(
            message.contains("runs 2501 classes deep, down to `C2500`"),
            "must carry the chain's measured total, in classes — `C0..C2500` inclusive: \
             {message}"
        );
    }

    #[test]
    fn a_class_chain_past_the_ceiling_is_only_a_warning_outside_strict_mode() {
        // Round 6 review M4(b): the pre-check used to hard-code
        // `Diagnostic::error`, so `[types] strict = false` never downgraded
        // it — "the CHANGELOG's own migration advice ('strict = false … so
        // this is CI-green again') is false for this code." `LB0317` now
        // follows the same strictness ladder as every other type
        // diagnostic (see `the_same_type_mismatch_is_only_a_warning_outside_strict_mode`
        // above for the LB0300 sibling of this same claim).
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", &class_chain_source(2_500));
        check(tmp.path(), None, Format::Human)
            .expect("a depth-limit warning outside strict mode must not fail the check");
    }

    #[test]
    fn a_class_chain_at_or_under_the_ceiling_checks_normally() {
        // The floor side of the same guard: the LARGEST chain that must not
        // be refused — `MAX_ANCESTRY_DEPTH` classes, `C0..C(MAX - 1)`, the
        // exact size `env::tests::a_class_chain_at_the_ancestry_limit_resolves_fully_and_correctly`
        // pins from the resolver's side.
        //
        // PR #61 round-8 F3, both halves. The size was one class too large
        // (`class_chain_source(MAX)` declares `C0..C200`, MAX + 1 classes —
        // already the resolver's first truncating size), and the fixture
        // declared the chain without ever REFERENCING it, so the resolver
        // never walked it and the test could not tell the two boundaries
        // apart. A `---@param` of the deepest class plus a member read makes
        // resolution happen, which is the only way this asserts anything
        // about the resolver at all.
        use std::fmt::Write as _;

        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        let n = MAX_ANCESTRY_DEPTH - 1;
        let mut src = class_chain_source(n);
        // A `---@param` + member read, not `---@type C{n} local x = {}`:
        // an empty table literal against an IN-BOUNDS class owes the root's
        // `item` field, so the literal shape would report `LB0300`/`LB0306`
        // for the fixture rather than for the boundary under test.
        let _ = write!(
            src,
            "\n---@param v C{n}\nlocal function use(v)\n  return v.item\nend\nreturn use\n"
        );
        write(tmp.path(), "src/main.lua", &src);
        check(tmp.path(), None, Format::Human).expect("a chain at the ceiling is not refused");
        let diags = check_diagnostics(tmp.path());
        assert!(
            !diags
                .iter()
                .any(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP),
            "the largest in-bounds chain, actually resolved, must produce no depth diagnostic \
             from EITHER mechanism: {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == Code::new(306)),
            "nor a truncated-shape `LB0306` for the root field it really inherits: {diags:?}"
        );
    }

    #[test]
    fn the_smallest_over_limit_chain_reports_the_pre_checks_actionable_link_once() {
        // PR #61 round-8 F3. `MAX_ANCESTRY_DEPTH` is one number enforced
        // twice, but the two enforcements counted in different units: the
        // resolver counts CLASSES on the path (`on_path.len() >= MAX`, first
        // trip at MAX + 1 classes) while this pre-check compared an EDGE
        // count (`depth`, one less than the class count) against the same
        // constant with `>` — first trip at MAX + 2 classes. The gap is
        // exactly one chain size wide.
        //
        // Measured at 825c61c on this fixture (MAX + 1 = 201 classes, `C0..C200`,
        // WITH a live reference so the resolver walks it): the pre-check
        // certified the chain as in-bounds, so `covered` stayed empty and
        // nothing suppressed the resolver's drain — the user got
        // "`C200`'s `---@class` ancestry is too deep to resolve safely",
        // which names no link to flatten, instead of the pre-check's
        // actionable first-class-over-the-limit report. Both sides now count
        // classes, so the smallest chain either mechanism objects to is the
        // same chain.
        use std::fmt::Write as _;

        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        let n = MAX_ANCESTRY_DEPTH;
        let mut src = class_chain_source(n);
        // A `---@param` + member read, not `---@type C{n} local x = {}`:
        // an empty table literal against an IN-BOUNDS class owes the root's
        // `item` field, so the literal shape would report `LB0300`/`LB0306`
        // for the fixture rather than for the boundary under test.
        let _ = write!(
            src,
            "\n---@param v C{n}\nlocal function use(v)\n  return v.item\nend\nreturn use\n"
        );
        write(tmp.path(), "src/main.lua", &src);
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
        let diags = check_diagnostics(tmp.path());
        let depth: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
            .collect();
        assert_eq!(
            depth.len(),
            1,
            "one chain, one `LB0317` — the pre-check's `covered` set must swallow the \
             resolver's drain for the same chain: {depth:?}"
        );
        assert!(
            depth[0].message.contains(&format!("`C{n}`'s")),
            "the surviving one names the first link over the limit, `C{n}`: {depth:?}"
        );
        assert!(
            depth[0].message.contains(&format!(
                "ancestry is {} classes deep",
                MAX_ANCESTRY_DEPTH + 1
            )),
            "and measures it in the same unit the limit is stated in — classes on the path, \
             `C0..C{n}` inclusive: {depth:?}"
        );
        assert!(
            !depth[0].message.contains("too deep to resolve safely"),
            "the non-actionable resolver-side wording must not be what reaches the user: \
             {depth:?}"
        );
    }

    #[test]
    fn the_pre_check_ceiling_and_the_resolver_cap_are_the_same_number() {
        // An earlier #61 local merge-gate round: before this pre-check
        // imported `luabox_types::MAX_ANCESTRY_DEPTH` instead of keeping its
        // own, looser, separately-reasoned figure (2,000), a 400-class
        // single-inheritance chain — well inside what this pre-check's
        // message called safe — sailed straight through it, then had its
        // shape silently truncated the moment anything referenced it (the
        // resolver's own cap sat at 200 all along), producing a false
        // `LB0306` `undefined field` for a field the pre-check's own message
        // a moment earlier had certified as within bounds. Now the two
        // share one constant, so the chain is caught HERE, early and cheaply,
        // before resolution ever gets a chance to truncate anything: one
        // depth diagnostic at `C200`, never the pre-check's silence followed
        // by a wrong `LB0306` from the checker.
        //
        // Exactly one, not two: `---@type C400` makes the resolver walk the
        // chain too, so its own `LB0317` drain fires as well — measured, on
        // `C400`, in different words than the pre-check's `C200`. Two
        // diagnostics for one chain from two mechanisms is what
        // `collect_diagnostics`'s coverage filter exists to collapse
        // (production readiness review, finding 1(c)).
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        let mut src = class_chain_source(400);
        src.push_str("\n---@type C400\nlocal x = {}\nlocal y = x.item\n");
        write(tmp.path(), "src/main.lua", &src);
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "check failed with 1 error(s)");
        let diags = check_diagnostics(tmp.path());
        let depth: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
            .collect();
        assert_eq!(depth.len(), 1, "one chain, one LB0317: {depth:?}");
        assert!(
            depth[0]
                .message
                .contains(&format!("`C{MAX_ANCESTRY_DEPTH}`")),
            "the surviving one is the pre-check's actionable first link: {depth:?}"
        );
        assert!(
            !diags.iter().any(|d| d.code == Code::new(306)),
            "no truncated-shape LB0306 for a field the chain really declares: {diags:?}"
        );
    }

    /// A source file with `n` `---@class`-named chains, one per `prefix` in
    /// `prefixes`, each `{prefix}0 .. {prefix}n`, `{prefix}i : {prefix}(i-1)`
    /// — for exercising more than one over-limit chain in one project.
    fn named_chain_source(prefix: &str, n: usize) -> String {
        let mut src = format!("---@class {prefix}0\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class {prefix}{i} : {prefix}{}", i - 1);
        }
        src
    }

    /// The same depth as [`class_chain_source`], but every class from `C1`
    /// on carries a second, unrelated parent (`Extra`, itself a root) —
    /// round 6 review M4(c)'s multi-parent shape.
    fn multi_parent_chain_source(n: usize) -> String {
        let mut src = String::from("---@class C0\n---@class Extra\n");
        for i in 1..=n {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}, Extra", i - 1);
        }
        src
    }

    /// A `k`-level generic diamond, mirroring `luabox-types`' own
    /// `conflicting_diamond_source` test fixture: `A0<T>`, `A1<T> : A0<T>`,
    /// then for `i` in `2..=k`, `Ai<T> : A(i-1)<T>, A(i-2)<number>` — the
    /// disagreeing edge points at the *grandparent* so the two parents keep
    /// distinct names (a same-name pair now dedupes to one edge and stops
    /// being a diamond at all). The shape's re-resolution *cost*, not its
    /// depth, is what trips `LB0319`
    /// (`luabox_types::env::MAX_ANCESTRY_RESOLUTIONS`'s own doc comment
    /// carries the measurement behind the exact `k` this needs).
    fn conflicting_diamond_source(k: usize) -> String {
        let mut src = String::from("---@class A0<T>\n---@field item T\n---@class A1<T> : A0<T>\n");
        for i in 2..=k {
            use std::fmt::Write as _;
            let _ = writeln!(
                src,
                "---@class A{i}<T> : A{parent}<T>, A{grandparent}<number>",
                parent = i - 1,
                grandparent = i - 2
            );
        }
        src
    }

    /// Build a [`SourceFile`] fixture directly, for tests that call
    /// [`deep_class_chain_diagnostics`] without going through the full CLI
    /// entry point (`check`/`discover`) — this crate's own unit-test seam
    /// for a project-wide pre-check that has no manifest-level knob beyond
    /// strictness, which every caller here passes explicitly.
    fn source_file(rel: &str, source: &str) -> SourceFile {
        let parse = lua::parse(source, Dialect::Lua54);
        let artifacts = luabox_types::FileArtifacts::new(&parse);
        SourceFile {
            canonical: PathBuf::from(rel),
            rel: rel.to_owned(),
            parse,
            artifacts,
        }
    }

    #[test]
    fn the_depth_diagnostic_carries_lb0317_not_lb0001() {
        // Round 6 review M4(b): the pre-check used to hard-code
        // `Code::new(1)` — `LB0001`, "syntax error" — so `luabox explain` on
        // the code it actually printed described a missing
        // `end`/`)`/`}`/`then`, not an ancestry limit. `LB0317`, the code
        // this round introduced for exactly this condition, was unreachable
        // from this path.
        let files = [source_file("main.lua", &class_chain_source(2_500))];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(!diags.is_empty());
        for diag in &diags {
            assert_eq!(diag.code, LB0317_CLASS_ANCESTRY_TOO_DEEP, "{diag:?}");
        }
    }

    #[test]
    fn a_matching_diagnostic_disable_comment_suppresses_the_depth_diagnostic() {
        // Round 6 review M4(b): the pre-check's own diagnostic bypassed
        // every suppression mechanism outright — it ran and returned before
        // `luabox_types`' own per-file `---@diagnostic` scan ever got a
        // chance to run. It now honors its own
        // `---@diagnostic disable: class-ancestry-too-deep` mirror of that
        // same syntax.
        let mut src = String::from("---@diagnostic disable: class-ancestry-too-deep\n");
        src.push_str(&class_chain_source(2_500));
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn an_unrelated_diagnostic_disable_comment_does_not_suppress_it() {
        // The other direction of the same claim: suppression is scoped to
        // this pre-check's own rule name, not a bare `disable` naming
        // something else entirely.
        let mut src = String::from("---@diagnostic disable: undefined-field\n");
        src.push_str(&class_chain_source(2_500));
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(!diags.is_empty());
    }

    /// [`class_chain_source`] with `comment` inserted on its own line
    /// directly above `---@class C{target}` — so a `disable-next-line`
    /// written there covers exactly that declaration and nothing else.
    fn class_chain_source_with_comment_above(n: usize, target: usize, comment: &str) -> String {
        let chain = class_chain_source(n);
        let needle = format!("---@class C{target} : C{}\n", target - 1);
        assert!(chain.contains(&needle), "the chain must declare C{target}");
        chain.replacen(&needle, &format!("{comment}\n{needle}"), 1)
    }

    #[test]
    fn a_block_comment_diagnostic_disable_suppresses_the_depth_diagnostic() {
        // Production readiness review, finding 3. This pre-check used to walk
        // already-harvested `Tag::Diagnostic` bodies, which the LuaCATS
        // harvester only produces for the `---@` form — so
        // `--[[@diagnostic disable: class-ancestry-too-deep]]` silenced the
        // CHECKER-side `LB0317` (`luabox_types::DirectiveScan` splits raw text
        // on `@diagnostic`, block comment or not) and left this one standing.
        // One rule, two mechanisms, two grammars, two answers.
        let mut src = String::from("--[[@diagnostic disable: class-ancestry-too-deep]]\n");
        src.push_str(&class_chain_source(2_500));
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_plain_double_dash_diagnostic_disable_suppresses_the_depth_diagnostic() {
        // Finding 3's other reported grammar: a plain `--@diagnostic` line,
        // with no doc-comment `---` prefix. Same story as the block-comment
        // form above — accepted by `DirectiveScan`, invisible to the harvested
        // -tag walk this used to do.
        let mut src = String::from("--@diagnostic disable: class-ancestry-too-deep\n");
        src.push_str(&class_chain_source(2_500));
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_line_scoped_disable_directly_above_the_reported_class_suppresses_it() {
        // The line-scoped path had ZERO tests either side (finding 3): every
        // existing suppression test used the file-wide `disable`, which needs
        // no line arithmetic at all, so nothing pinned that this pre-check
        // resolves the offset of the class it reports to the right line —
        // and nothing would have caught it resolving offsets against a
        // different file's `LineIndex`.
        let src = class_chain_source_with_comment_above(
            2_500,
            MAX_ANCESTRY_DEPTH,
            "---@diagnostic disable-next-line: class-ancestry-too-deep",
        );
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_line_scoped_disable_on_the_wrong_line_does_not_suppress_it() {
        // The negative half, and the one that actually fails if the line
        // arithmetic is replaced by "any line-scoped disable anywhere in the
        // file counts" — which is what the old hand-rolled scanner degraded
        // to whenever its offset->line conversion went wrong. `C0` is nowhere
        // near the class this reports (`C200`).
        let src = class_chain_source_with_comment_above(
            2_500,
            1,
            "---@diagnostic disable-next-line: class-ancestry-too-deep",
        );
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert_eq!(diags.len(), 1, "{diags:?}");
    }

    #[test]
    fn strictness_none_reports_no_depth_diagnostic() {
        // `Strictness::None` disables every type diagnostic project-wide
        // (`luabox_types`' own gate, `lib.rs`) — this pre-check honors it
        // too rather than reporting a diagnostic the rest of the pipeline
        // would never have produced.
        let files = [source_file("main.lua", &class_chain_source(2_500))];
        let diags = deep_class_chain_diagnostics(&files, Strictness::None).diags;
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_multi_parent_chain_gives_the_same_verdict_as_a_single_parent_chain_at_the_same_depth() {
        // Round 6 review M4(c): the pre-check only followed
        // `if let [parent] = class.parents.as_slice()`, so adding *any*
        // second parent turned the identical depth from `error`/exit 1
        // into `warning`/exit 0 purely because a second, unrelated parent
        // was added. It now walks every named parent, so both shapes give
        // the same verdict at the same depth.
        let n = MAX_ANCESTRY_DEPTH + 50;
        let single = [source_file("single.lua", &class_chain_source(n))];
        let multi = [source_file("multi.lua", &multi_parent_chain_source(n))];
        let single_diags = deep_class_chain_diagnostics(&single, Strictness::Strict).diags;
        let multi_diags = deep_class_chain_diagnostics(&multi, Strictness::Strict).diags;
        assert_eq!(single_diags.len(), multi_diags.len());
        assert!(!single_diags.is_empty());
        let deepest = format!("C{n}");
        assert!(single_diags.iter().any(|d| d.message.contains(&deepest)));
        assert!(multi_diags.iter().any(|d| d.message.contains(&deepest)));
    }

    #[test]
    fn three_over_limit_chains_are_all_reported_in_deterministic_source_order() {
        // Round 6 review M17: `HashMap` iteration order used to decide
        // which single over-limit class the diagnostic named
        // (`depth.iter().max_by_key(..)`), so 30 runs on a three-chain
        // fixture gave three different answers and silently dropped the
        // other two chains every time. Every over-limit chain is now
        // reported, sorted by declaring span — deterministic regardless of
        // hashmap or harvest order, matching the whole-diagnostics sort
        // `luabox_types::check::run`'s own `LB0317` path already applies.
        let n = MAX_ANCESTRY_DEPTH + 1;
        let mut src = named_chain_source("A", n);
        src.push_str(&named_chain_source("B", n));
        src.push_str(&named_chain_source("C", n));
        // The reported link is the FIRST over the limit, not the deepest:
        // `{prefix}MAX_ANCESTRY_DEPTH`, whose own path is one class past what
        // the resolver walks (PR #61 round-8 F3).
        let first = MAX_ANCESTRY_DEPTH;
        let files = [source_file("main.lua", &src)];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        let names: Vec<String> = diags
            .iter()
            .map(|d| {
                d.message
                    .split('`')
                    .nth(1)
                    .expect("message names the class in backticks")
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec![
                format!("A{first}"),
                format!("B{first}"),
                format!("C{first}")
            ]
        );
    }

    #[test]
    fn a_depth_diagnostic_does_not_disable_the_rest_of_the_check() {
        // Production readiness review, finding 1. The pre-check used to
        // `return` its diagnostics the moment it found any, which skipped
        // EVERY remaining pass in the command: parse errors, dialect
        // legality, control flow, the type pass and the `[types] defs`
        // resolution report all silently stopped existing for the rest of
        // that run. Measured on this shape before the fix — a 201-link chain
        // in one file, plain syntax errors in another, default (warn)
        // strictness — `luabox check` printed the one depth warning, no
        // `LB0001` at all, and exited 0.
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/deep.lua", &class_chain_source(201));
        write(
            tmp.path(),
            "src/broken.lua",
            "local x = = 1\nfunction f( end\n",
        );
        let diags = check_diagnostics(tmp.path());
        assert!(
            diags
                .iter()
                .any(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP),
            "the depth warning is still reported: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.code == Code::new(1)),
            "and so are the syntax errors it used to hide: {diags:?}"
        );
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.contains("check failed with"), "{error}");
    }

    #[test]
    fn an_error_manifest_diagnostic_is_not_discarded_by_a_warning_depth_diagnostic() {
        // Finding 1's severity-inversion half, the sharpest form of it: the
        // discarded finding was `LB1002` — an `Error` — and what discarded it
        // was a `Warning`. The same project with the deep chain removed
        // reported `LB1002` and failed; with the chain present it reported
        // the depth warning alone and exited 0, so a project could not
        // resolve its own `[types] defs` and `check` said nothing about it.
        let tmp = project(&manifest("5.4", "\n[types]\ndefs = [\"ghost\"]\n"));
        write(tmp.path(), "src/deep.lua", &class_chain_source(201));
        let diags = check_diagnostics(tmp.path());
        assert!(
            diags.iter().any(|d| d.code == Code::new(1002)),
            "the unresolved-defs error survives the depth warning: {diags:?}"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP),
            "and the depth warning is still there too: {diags:?}"
        );
    }

    #[test]
    fn suppressing_the_depth_diagnostic_changes_nothing_else_about_the_report() {
        // Finding 1's inverted-causality half. With the early return in
        // place, adding a `---@diagnostic disable: class-ancestry-too-deep`
        // comment made `luabox check` go from exit 0 with one warning to
        // exit 1 with four errors: silencing a diagnostic *revealed* four
        // others, because it was the pre-check returning non-empty — not the
        // diagnostic's severity — that had been ending the run. A suppression
        // comment may only remove the thing it names.
        let codes_of = |suppression: &str| -> Vec<u16> {
            let tmp = project(&manifest("5.4", ""));
            let mut deep = String::from(suppression);
            deep.push_str(&class_chain_source(201));
            write(tmp.path(), "src/deep.lua", &deep);
            write(
                tmp.path(),
                "src/broken.lua",
                "local x = = 1\nfunction f( end\n",
            );
            let mut codes: Vec<u16> = check_diagnostics(tmp.path())
                .iter()
                .filter(|d| d.code != LB0317_CLASS_ANCESTRY_TOO_DEEP)
                .map(|d| d.code.number())
                .collect();
            codes.sort_unstable();
            codes
        };
        assert_eq!(
            codes_of(""),
            codes_of("---@diagnostic disable: class-ancestry-too-deep\n")
        );
        assert!(
            !codes_of("").is_empty(),
            "the fixture must report something"
        );
    }

    #[test]
    fn a_bare_class_typo_in_one_file_cannot_change_the_verdict_on_another() {
        // Production readiness review, finding 4. A bare `---@class` harvests
        // as a class whose name is the empty string, and the pre-check used
        // to enter it into `declared_at`/`parents_of` under the key `""` like
        // any other name. First-wins then made every empty-named declaration
        // in the project ONE node, shared across files that have nothing to
        // do with each other: measured, `z.lua` alone reported its chain,
        // and adding an unrelated `---@class` typo in `a.lua` — which won the
        // `parents_of[""]` entry with an empty parent list, pinning `""`'s
        // depth at 0 — made that same chain VANISH from the report. The
        // guard failed open, in the one direction a limit check must never
        // fail, and a second symptom of the same collapse attributed one
        // file's chain to the other file's typo.
        //
        // The claim under test is the invariant, not either verdict: adding
        // a file that declares nothing nameable may not change what is
        // reported about a file it never mentions. Asserted for both the
        // shape whose deepest link is itself unnamed and one whose chain is
        // named end to end, since the collapse could only ever be observed
        // through the former.
        let verdict = |chain: &str, typo: bool| -> Vec<String> {
            let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
            write(tmp.path(), "src/z.lua", chain);
            if typo {
                write(
                    tmp.path(),
                    "src/a.lua",
                    "---@class\nlocal q = 1\nreturn q\n",
                );
            }
            let mut reported: Vec<String> = check_diagnostics(tmp.path())
                .iter()
                .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
                .map(|d| {
                    let label = d
                        .primary_label()
                        .expect("an ancestry-guard diagnostic always carries a primary label");
                    format!("{}: {}", label.span.file, d.message)
                })
                .collect();
            reported.sort();
            reported
        };
        let unnamed_tail = format!("{}---@class : C200\n", class_chain_source(200));
        assert_eq!(verdict(&unnamed_tail, false), verdict(&unnamed_tail, true));
        let named = class_chain_source(201);
        assert_eq!(verdict(&named, false), verdict(&named, true));
        assert_eq!(
            verdict(&named, true).len(),
            1,
            "the named chain is over the limit and must be reported either way"
        );
    }

    #[test]
    fn an_empty_named_class_never_produces_a_diagnostic_that_names_no_class() {
        // Finding 4's other symptom: with `""` treated as a class name, a
        // chain ending in `---@class : C200` produced a diagnostic whose
        // message read "``'s `---@class` ancestry is 201 classes deep" —
        // pointing the reader at a class that does not exist and cannot be
        // renamed, flattened or suppressed by name. An unnamed `---@class` is
        // a syntax problem for the harvester to report, not an ancestry one.
        let files = [source_file(
            "z.lua",
            &format!("{}---@class : C200\n", class_chain_source(200)),
        )];
        let diags = deep_class_chain_diagnostics(&files, Strictness::Strict).diags;
        assert!(
            !diags.iter().any(|d| d.message.contains("``")),
            "no diagnostic may name the empty class: {diags:?}"
        );
    }

    #[test]
    fn two_diagnostics_sharing_one_span_always_render_in_the_same_order() {
        use std::fmt::Write as _;
        // Production readiness review, finding 5. `report_ancestry_hits`'s
        // ambient tier attributes every class with no in-project declaration
        // to `Span::new(file, 0..0)`, so two over-limit `[types] defs` chains
        // consumed by one file produce two diagnostics at the IDENTICAL span,
        // drained from a hash-ordered ledger. Sorting on `range.start` alone
        // left their relative order to hashmap iteration: 12 consecutive runs
        // of the same binary over the same sources printed one order 10 times
        // and the other twice. `sort_for_render`'s code/message tie-breakers
        // are what make this assertion meaningful rather than lucky.
        let tmp = project(&manifest("5.4", "\n[types]\ndefs = [\"deep\"]\n"));
        let n = MAX_ANCESTRY_DEPTH + 50;
        let mut defs_src = String::from("---@meta\n");
        for prefix in ["A", "B"] {
            let _ = writeln!(defs_src, "---@class {prefix}0\n---@field f{prefix} string");
            for i in 1..=n {
                let _ = writeln!(defs_src, "---@class {prefix}{i} : {prefix}{}", i - 1);
            }
        }
        write(tmp.path(), "defs/deep.d.lua", &defs_src);
        write(
            tmp.path(),
            "src/main.lua",
            &format!("---@type A{n}\nlocal a\n---@type B{n}\nlocal b\nprint(a.fA, b.fB)\n"),
        );
        let named: Vec<String> = check_diagnostics(tmp.path())
            .iter()
            .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
            .map(|d| d.message.clone())
            .collect();
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(named[0].contains(&format!("A{n}")), "{named:?}");
        assert!(named[1].contains(&format!("B{n}")), "{named:?}");
        // Same sources, same binary, again: the claim is *stability*, so one
        // observation of the right order proves nothing on its own.
        for _ in 0..8 {
            let again: Vec<String> = check_diagnostics(tmp.path())
                .iter()
                .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
                .map(|d| d.message.clone())
                .collect();
            assert_eq!(again, named, "the order must not vary between runs");
        }
    }

    #[test]
    fn an_ambient_defs_class_past_the_ceiling_still_reports_lb0317_in_strict_mode() {
        // Round 6 review M5: past the limit, a class supplied by a
        // `[types] defs` ambient file had its `LB0317` silently dropped —
        // "there is simply nothing here to point a diagnostic at" — while
        // the LSP reported it on the identical project: editor red, CI
        // green, same commit. The CLI's own syntactic pre-check only scans
        // project files (`DefFiles::Exclude`), not `defs/`, so this
        // exercises the *other* half of the mechanism — `luabox-types`'
        // real, resolver-driven `LB0317` (`luabox_types::check::run`), now
        // attributed to the consuming file when the class itself has no
        // in-project declaration to point at, and — since it is still
        // `[types] strict = true` — unable to leave `check` exit 0 while
        // doing so.
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"deep\"]\n",
        ));
        let n = MAX_ANCESTRY_DEPTH + 50;
        let mut defs_src = String::from("---@meta\n");
        defs_src.push_str(&class_chain_source(n));
        write(tmp.path(), "defs/deep.d.lua", &defs_src);
        write(
            tmp.path(),
            "src/main.lua",
            &format!("---@type C{n}\nlocal c = {{}}\nlocal _ = c.totally_undefined_field\n"),
        );
        let error = check(tmp.path(), None, Format::Human)
            .unwrap_err()
            .to_string();
        assert!(error.contains("check failed with"), "{error}");
        assert!(error.contains("error(s)"), "{error}");
    }

    #[test]
    fn two_over_limit_ambient_classes_each_report_rather_than_collapsing_into_one() {
        use std::fmt::Write as _;
        // Local merge-gate re-run finding on G2's own remedy. An ambient
        // class has no in-project declaration to point at, so
        // `check::report_ancestry_hits` attributes it to `Span::new(file,
        // 0..0)` — the *consuming* file at offset zero, identical
        // coordinates whichever class tripped the guard. The cross-file
        // dedup added for G2 keyed on `(code, span.file, span.range)` alone,
        // so two different over-limit `[types] defs` classes consumed by one
        // file shared a key and the SECOND WAS DROPPED: a real, distinct
        // type error silently absent from `luabox check`.
        //
        // Dropping a finding is strictly worse than the duplicate the dedup
        // exists to remove, and it is invisible by construction — there is
        // no output to notice. The key now carries the message, the only
        // place a `Diagnostic` retains the class name. Two classes, two
        // diagnostics; keep it that way.
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"deep\"]\n",
        ));
        let n = MAX_ANCESTRY_DEPTH + 50;
        let mut defs_src = String::from("---@meta\n");
        for prefix in ["A", "B"] {
            let _ = writeln!(defs_src, "---@class {prefix}0\n---@field f{prefix} string");
            for i in 1..=n {
                let _ = writeln!(defs_src, "---@class {prefix}{i} : {prefix}{}", i - 1);
            }
        }
        write(tmp.path(), "defs/deep.d.lua", &defs_src);
        write(
            tmp.path(),
            "src/main.lua",
            &format!("---@type A{n}\nlocal a\n---@type B{n}\nlocal b\nprint(a.fA, b.fB)\n"),
        );
        let diags = check_diagnostics(tmp.path());
        let over_limit: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == LB0317_CLASS_ANCESTRY_TOO_DEEP)
            .collect();
        assert_eq!(
            over_limit.len(),
            2,
            "both ambient classes must report, not just whichever won the span key: {over_limit:?}"
        );
        assert!(
            over_limit
                .iter()
                .any(|d| d.message.contains(&format!("A{n}"))),
            "{over_limit:?}"
        );
        assert!(
            over_limit
                .iter()
                .any(|d| d.message.contains(&format!("B{n}"))),
            "{over_limit:?}"
        );
    }

    #[test]
    fn a_generic_class_referenced_directly_by_name_still_reports_the_cost_budget_trip() {
        // Production readiness measurement R1: `LB0319` (the ancestry
        // resolution-cost budget, `MAX_ANCESTRY_RESOLUTIONS`) used to be
        // silently bypassed whenever an over-budget generic class was
        // referenced directly by name (`---@type A190<string>`). Root cause:
        // `TypeEnv::build_from_items` resolves a `---@class Name<T>`
        // reference through a template pre-built by
        // `collect_generic_classes` on its own throwaway `discovery:
        // TypeEnv` — the walk that builds that template DOES trip
        // `DiamondGuard`'s cost budget on this fixture, but the trip used to
        // be recorded onto `discovery`'s own ledgers, which were dropped the
        // moment the template map was returned, so nothing was left for
        // `check::report_cost_limit_hits` to drain. The sibling shape —
        // `---@class Leaf : A190<string>`, `---@type Leaf` — always reported
        // correctly, because a non-generic leaf has no template of its own,
        // so the checker falls back to walking the REAL env directly and
        // trips that env's own ledger the ordinary way; only the direct-name
        // path, which never needs that fallback, stayed silent.
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        write(
            tmp.path(),
            "src/diamond.lua",
            &conflicting_diamond_source(190),
        );
        write(
            tmp.path(),
            "src/main.lua",
            "---@type A190<string>\nlocal a\nprint(a.item)\n",
        );
        let diags = check_diagnostics_bounded(tmp.path());
        assert!(
            diags
                .iter()
                .any(|d| d.code == LB0319_CLASS_ANCESTRY_TOO_COSTLY),
            "a generic class referenced directly by name must still report the cost-budget \
             trip its own discovery-pass ancestry resolution hit, exactly as the same \
             class reached through a non-generic leaf already does: {diags:?}"
        );
    }

    #[test]
    fn a_cost_limit_hit_is_only_a_warning_outside_strict_mode() {
        // R2 (production readiness issue): `LB0317`'s warn-mode downgrade is
        // pinned above
        // (`a_class_chain_past_the_ceiling_is_only_a_warning_outside_strict_mode`);
        // `LB0319` shares the exact same severity plumbing
        // (`report_ancestry_hits`'s `severity` parameter, common to all
        // three ancestry-guard codes), but nothing asserted that for the
        // cost-budget code specifically — a regression that hard-coded
        // `Severity::Error` for just this one code (as the pre-`M4(b)`
        // depth-limit diagnostic once did) would pass every existing test.
        let tmp = project(&manifest("5.4", ""));
        write(tmp.path(), "src/main.lua", &conflicting_diamond_source(190));
        // Bounded like `check_diagnostics_bounded`: a dead resolution budget
        // makes this k=190 check the round-5 exponential.
        let cwd = tmp.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check(&cwd, None, Format::Human).is_ok());
        });
        let ok = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("a budget-capped k=190 check must finish in seconds");
        assert!(
            ok,
            "a cost-limit warning outside strict mode must not fail the check"
        );
    }

    #[test]
    fn a_matching_diagnostic_disable_comment_suppresses_the_cost_limit_diagnostic() {
        // R2: the `---@diagnostic disable: class-ancestry-too-costly`
        // escape hatch (`directive::RULE_CLASS_ANCESTRY_TOO_COSTLY`) is wired
        // through the same `directive::rule_for_code` table LB0317/LB0318
        // use (pinned above by `a_matching_diagnostic_disable_comment_suppresses_the_depth_diagnostic`),
        // but nothing pinned it for LB0319 itself.
        let tmp = project(&manifest("5.4", "\n[types]\nstrict = true\n"));
        let mut src = String::from("---@diagnostic disable: class-ancestry-too-costly\n");
        src.push_str(&conflicting_diamond_source(190));
        write(tmp.path(), "src/main.lua", &src);
        // Bounded for the same reason as its two k=190 siblings above.
        let cwd = tmp.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check(&cwd, None, Format::Human).is_ok());
        });
        let ok = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("a budget-capped k=190 check must finish in seconds");
        assert!(
            ok,
            "a matching `class-ancestry-too-costly` disable comment must suppress LB0319"
        );
    }

    #[test]
    fn a_cyclic_ambient_defs_class_reports_lb0318_attributed_to_the_consuming_file() {
        // R4 (production readiness issue): `report_ancestry_hits`'s third
        // attribution arm (`Span::new(file, 0..0)` — no in-project
        // declaration to point at) is pinned for `LB0317` by
        // `an_ambient_defs_class_past_the_ceiling_still_reports_lb0317_in_strict_mode`
        // and `two_over_limit_ambient_classes_each_report_rather_than_collapsing_into_one`
        // above; nothing pinned the identical tier for `LB0318` (cyclic
        // class) or `LB0319` (cost budget, sibling test below it). Both
        // classes of the cycle live only in `defs/`, never in-project, so
        // the same third-arm attribution is exercised, not either of the
        // two spanned tiers `report_ancestry_hits` also has to choose
        // between.
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"cyclic\"]\n",
        ));
        write(
            tmp.path(),
            "defs/cyclic.d.lua",
            "---@meta\n---@class A : B\n---@class B : A\n",
        );
        // A field read, not a bare `---@type`, forces the walk that trips
        // the guard — mirroring
        // `an_ambient_defs_class_past_the_ceiling_still_reports_lb0317_in_strict_mode`'s
        // own `c.totally_undefined_field` probe: nothing resolves a
        // declared-but-unused type's ancestry.
        write(
            tmp.path(),
            "src/main.lua",
            "---@type A\nlocal a = {}\nlocal _ = a.whatever\n",
        );
        let diags = check_diagnostics(tmp.path());
        let cyclic: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == LB0318_CYCLIC_CLASS_ANCESTRY)
            .collect();
        assert!(!cyclic.is_empty(), "must report LB0318: {diags:?}");
        for d in &cyclic {
            let label = d
                .primary_label()
                .expect("an ancestry-guard diagnostic always carries a primary label");
            assert_eq!(
                label.span.range,
                0..0,
                "an ambient-only class with no in-project declaration must attribute to \
                 the consuming file at offset zero (report_ancestry_hits's third arm): \
                 {d:?}"
            );
            assert_eq!(
                label.span.file, "src/main.lua",
                "must attribute to the file that consumed it: {d:?}"
            );
        }
    }

    #[test]
    fn an_over_budget_ambient_defs_generic_class_reports_lb0319_attributed_to_the_consuming_file() {
        // R4's other missing case: the same third-arm attribution tier, for
        // `LB0319` this time. Also independently re-confirms R1's fix on
        // the ambient path specifically — `defs/diamond.d.lua`'s generic
        // classes only ever get a template built via the discovery pass
        // `TypeEnv::build_from_items` runs for the CONSUMING project file
        // (`build_ambient` itself never calls `collect_generic_classes`),
        // so this exercises the exact same silent-bypass path R1 fixed,
        // through an ambient defs package rather than an in-project file.
        let tmp = project(&manifest(
            "5.4",
            "\n[types]\nstrict = true\ndefs = [\"diamond\"]\n",
        ));
        let mut defs_src = String::from("---@meta\n");
        defs_src.push_str(&conflicting_diamond_source(190));
        write(tmp.path(), "defs/diamond.d.lua", &defs_src);
        write(
            tmp.path(),
            "src/main.lua",
            "---@type A190<string>\nlocal a\nprint(a.item)\n",
        );
        let diags = check_diagnostics_bounded(tmp.path());
        let cost: Vec<&Diagnostic> = diags
            .iter()
            .filter(|d| d.code == LB0319_CLASS_ANCESTRY_TOO_COSTLY)
            .collect();
        assert!(!cost.is_empty(), "must report LB0319: {diags:?}");
        for d in &cost {
            let label = d
                .primary_label()
                .expect("an ancestry-guard diagnostic always carries a primary label");
            assert_eq!(
                label.span.range,
                0..0,
                "an ambient-only class with no in-project declaration must attribute to \
                 the consuming file at offset zero (report_ancestry_hits's third arm): \
                 {d:?}"
            );
            assert_eq!(
                label.span.file, "src/main.lua",
                "must attribute to the file that consumed it: {d:?}"
            );
        }
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
