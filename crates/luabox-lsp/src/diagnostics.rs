//! Per-file diagnostics for publishing: parse errors, dialect legality, type
//! diagnostics, and lint findings for `.lua` files.
//!
//! Mirrors `luabox check`'s passes over one memoized parse, then runs the
//! `luabox lint` engine (the same one the CLI drives), converting every
//! finding to LSP ranges through the file's [`LineIndex`]. Control-flow
//! legality (#44) rides in with the lint engine — it is the one pass that
//! already holds the file's HIR — and is re-tagged to the toolchain source on
//! the way out, since it is not a lint rule.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use luabox_db::Analysis;
use luabox_lint::{LintConfig, lint_source};
use luabox_syntax::lua::{Dialect, validate};
use luabox_types::{
    CYCLIC_CLASS, ClassGraph, DirectiveScan, RULE_CIRCLE_DOC_CLASS, RockSurfaces, Strictness,
    check_file_with_requires,
};

use crate::line_index::LineIndex;
use crate::merged_ambient::MergedAmbient;
use crate::requires::RequireExports;

/// The `source` field on published type, parse, and dialect diagnostics.
const TYPE_SOURCE: &str = "luabox";

/// The code a recovered parse error is published under. `luabox_syntax`'s
/// parse errors carry a message and a range but no code of their own — the
/// toolchain assigns them all `LB0001`.
const PARSE_ERROR: luabox_diag::Code = luabox_diag::Code::new(1);

/// The `source` on published lint diagnostics, distinct from [`TYPE_SOURCE`]
/// so the editor — and the code-action matcher in [`crate::server`] — can tell
/// lint findings apart from type diagnostics. The `LB05xx` code carries the
/// specific rule.
pub(crate) const LINT_SOURCE: &str = "luabox-lint";

/// The `source` a diagnostic is published under, decided by its code.
///
/// `lint_source` also carries the control-flow legality errors (#44) —
/// `LB0020`-`LB0022` from `luabox_hir::validate`, which are *not* lint rules:
/// they have no tier and no `---@luabox-ignore` id, and the runtime refuses to
/// load the file either way. Those go out under the toolchain source alongside
/// the parse, dialect and type diagnostics; only the lint band gets
/// [`LINT_SOURCE`]. The band is `luabox_diag`'s to define
/// ([`luabox_diag::Code::is_lint`]) — an open-coded `number() / 100 == 5` here
/// was a contract nothing asserted.
///
/// Every publisher goes through this — the type pass, the parse and dialect
/// passes, the lint pass, and the code-action matcher, which re-converts the
/// originating diagnostic to pair it with its quick fix. That last site
/// hardcoded [`LINT_SOURCE`], which happened to be right for every fix that
/// exists today (all of them come from lint rules) and would silently mis-pair
/// the first fix-carrying diagnostic that does not (Shockwave round 5); the
/// first three hardcoded [`TYPE_SOURCE`], which is right for every code they
/// can emit today and would be wrong for the first one outside the band
/// (Shockwave round 6). "Every publisher" is now true rather than nearly true,
/// which is cheaper than keeping the exceptions enumerated and correct.
pub(crate) const fn source_for(code: luabox_diag::Code) -> &'static str {
    if code.is_lint() {
        LINT_SOURCE
    } else {
        TYPE_SOURCE
    }
}

/// The project's type-checking and lint context: strictness, the ambient
/// definition-package layer, and the lint configuration/known-globals baseline.
/// Owned by the server.
pub struct CheckCtx<'a> {
    pub strictness: Strictness,
    /// The merged ambient layer to check against — defs + workspace-global
    /// project types + rock types, as [`crate::server`]'s revision-keyed
    /// cache builds it — so a dependency's classes resolve in the editor
    /// exactly as they do under `luabox check`, and one merge serves every
    /// surface instead of being rebuilt per published file.
    ///
    /// [`MergedAmbient`] rather than a bare [`luabox_types::Ambient`] on
    /// purpose (#62): the two are the same Rust type once built
    /// (`with_project_types`/`with_rock_types` both return `Ambient`), so a
    /// caller that skips the merge — `&self.ambient`, the unmerged base
    /// layer `server.rs` passed here before the merge existed — type-checks
    /// identically and silently drops every cross-file class. Routing
    /// through the newtype (built only by
    /// [`MergedAmbient::build`]) makes that mistake a compile error instead
    /// of a doc comment nobody re-reads at the call site.
    pub ambient: &'a MergedAmbient,
    /// The type surfaces harvested from the project's vendored luarocks tree
    /// (#30): rock classes/enums/aliases, plus each rock module's
    /// `require`-export type. Merged *after* the project's own types, so a name
    /// the project declares wins over a rock's (explicit beats implicit).
    pub rocks: &'a RockSurfaces,
    /// The resolved `[lint]` configuration (tiers/rules/allowed globals), built
    /// from the manifest the same way `luabox lint` builds it.
    pub lint: &'a LintConfig,
    /// The `undefined-global` known-globals baseline (dialect stdlib + project
    /// and dependency defs), built the same way `luabox lint` builds it.
    pub known_globals: &'a HashSet<String>,
    /// The workspace's declared-`---@class` cycle pass
    /// ([`class_cycle_diagnostics`]), derived by the caller and cached on the
    /// host revision exactly as [`Self::ambient`] is.
    ///
    /// Owned by the caller for the same reason the merge is (round 13 review,
    /// noted cost): every pass over every file derives the SAME answer from
    /// the same workspace, so computing it inside [`diagnostics`] meant
    /// re-collecting and re-Tarjan-ing every class the project declares on
    /// every keystroke of every open buffer. It is a function of the analysis
    /// and the strictness alone — both of which move the host revision — so
    /// the revision is a complete cache key, with none of the manual
    /// invalidation `merged_ambient` needs for its unrevisioned base layers.
    pub cycles: &'a ClassCycles,
}

/// One check pass's LSP diagnostics, split by the file each one's primary
/// span actually belongs to.
///
/// Almost everything lands in [`Self::own`]: a parse error, a dialect
/// violation, a lint finding and the overwhelming majority of type
/// diagnostics all point inside the very file that was checked. The
/// exception is the cross-file `---@class` ancestry pair `LB0317`/`LB0318`,
/// whose `cross_file_class_decl_span` attribution tier
/// (`luabox_types::check`) deliberately points at the offending class's
/// **declaration**, which routinely lives in a different project file than
/// the one whose pass tripped the guard.
#[derive(Default)]
pub struct FileDiagnostics {
    /// Diagnostics whose primary span is in the checked file itself,
    /// converted through that file's own [`LineIndex`].
    pub own: Vec<Diagnostic>,
    /// Diagnostics whose primary span names a *different* project file,
    /// grouped by that file's path and converted through **its** own
    /// [`LineIndex`] — never the checked file's.
    ///
    /// A caller publishes each group under its own document URI. Ordered
    /// (a `BTreeMap`) so a batch of publishes is deterministic.
    pub foreign: BTreeMap<PathBuf, Vec<Diagnostic>>,
    /// Findings about the **workspace**, not about the checked file: the
    /// declared-`---@class` cycle pass ([`class_cycle_diagnostics`]) walks
    /// every project file's class graph, so every pass over any file derives
    /// the same answer for the same declarations.
    ///
    /// Kept apart from [`Self::foreign`] because the two have different
    /// lifetimes in the ledger, and mixing them was measurably wrong:
    /// `foreign` is a *contribution* — "what MY pass found about your file"
    /// — and the ledger merges every contributor's, which for a
    /// workspace-derived answer means one cycle appearing once per file the
    /// session ever checked, each copy going stale independently (fix one
    /// file and the other contributors' copies survive until they are
    /// themselves re-checked). This half is the whole answer as of the last
    /// pass, and the ledger REPLACES it wholesale — see
    /// `crate::server::ForeignLedger`.
    ///
    /// Grouped by the file each finding's declaration lives in and converted
    /// through **that** file's [`LineIndex`], exactly like `foreign`; the
    /// checked file's own share is in here too, not in `own`.
    pub workspace: BTreeMap<PathBuf, Vec<Diagnostic>>,
}

/// Diagnostics for one `.lua` file known to `analysis`, split by the file
/// each one really belongs to. `None` when the file is unknown.
///
/// **Every diagnostic is converted through its own span file's line index**
/// (production readiness review, finding 4). The type pass is handed one
/// file to check but can hand back a diagnostic whose primary span belongs
/// to another (see [`FileDiagnostics`]); this used to convert the whole
/// batch through the checked file's [`LineIndex`] on the reasoning that
/// "LSP diagnostics are already per-document, so the lossy path is fine".
/// That reasoning holds only while every span is same-file. Once one is not,
/// a foreign byte offset resolved against the wrong text is not merely
/// imprecise — [`LineIndex::range`] clamps an out-of-bounds offset, so a
/// cycle declared deep in a long file rendered at the *end* of a short one,
/// under a message about a class that file never mentions.
#[must_use]
pub fn diagnostics(
    analysis: &Analysis,
    path: &Path,
    dialect: Dialect,
    ctx: &CheckCtx<'_>,
) -> Option<FileDiagnostics> {
    let text = analysis.file_text(path)?;
    let index = LineIndex::new(text);
    let parsed = analysis.parse(path)?;
    let mut out = FileDiagnostics::default();

    // 1. Parse errors (the tree is recovered; later passes still run).
    for err in parsed.errors() {
        out.own.push(diagnostic(
            &index,
            usize::from(err.range.start())..usize::from(err.range.end()),
            DiagnosticSeverity::ERROR,
            PARSE_ERROR,
            err.message.clone(),
        ));
    }

    // 2. Dialect legality against the project edition.
    for err in validate::validate(parsed.parse(), dialect) {
        out.own.push(diagnostic(
            &index,
            usize::from(err.range.start())..usize::from(err.range.end()),
            DiagnosticSeverity::ERROR,
            // `DialectError::code` is the bare number; `Code` is what carries
            // the `LBnnnn` spelling — and what `source_for` reads.
            luabox_diag::Code::new(err.code),
            err.message,
        ));
    }

    // 3. Types against the ambient definition-package layer — the same pass
    // as `luabox check`, so classes resolve identically in the editor and in
    // CI. The file's cross-file `require` exports are in reach (#85), so a
    // `require("mod")` result types from the module's annotations in the
    // editor exactly as under `luabox check`; the project's workspace-global
    // classes (declared in any checked file, member attachments included —
    // luals parity) merge beneath the defs layer the same way `check_cmd`
    // merges them. This is the one pass that can produce a foreign span, so
    // it routes each diagnostic through `push_routed` rather than converting
    // the batch against `index` — see [`FileDiagnostics`].
    let rel = path.to_string_lossy();
    let mut foreign_indices: HashMap<PathBuf, LineIndex> = HashMap::new();
    // The project's and the rock tree's `require` answers, merged by the one
    // resolver every surface shares (#54) — hover and completion read the very
    // same map, so a `require` binding cannot type one way here and another
    // way under the cursor.
    let requires = RequireExports::resolve(analysis, path, ctx.rocks);
    // The declaration-driven `---@class` cycle pass, derived by the caller
    // (once per host revision, not once per publish) but consumed BEFORE the
    // type pass, so its `covered` set can drop the resolver's rediscovery of
    // a cycle it has already named — the same hand-off `luabox check` makes
    // between its pre-check and `env::note_cyclic`'s drain (R12-1).
    let cycles = ctx.cycles;
    for diag in check_file_with_requires(
        parsed.parse(),
        &rel,
        ctx.strictness,
        dialect,
        Some(ctx.ambient.get()),
        requires.by_module(),
    ) {
        if cycles.covers(&diag) {
            continue;
        }
        // Type diagnostics are all `LB03xx`, so this publishes under
        // `TYPE_SOURCE` today — but through the band authority inside
        // `convert`, so a code moving band moves its source with it.
        push_routed(
            &mut out,
            analysis,
            &rel,
            &index,
            &mut foreign_indices,
            &diag,
        );
    }
    // Routed the same way every other cross-file ancestry finding is — a
    // cycle member declared in ANOTHER project file lands under that file's
    // own URI, at its own line, whether or not it is open — but filed under
    // `workspace`, because that is what it is: see `FileDiagnostics`.
    out.workspace = route_workspace(analysis, cycles, Some((&rel, &index)), &mut foreign_indices);

    // 4. Lint findings — the `luabox lint` engine (SPEC.md §9), published
    // alongside the type diagnostics. `lint_source` applies the `[lint]`
    // tiers/config and `---@luabox-ignore` suppression itself. It re-parses
    // internally (the double-parse cost is per keystroke, but lint only runs
    // over one small file at a time). Skipped when the parse is not clean:
    // lint's own parse-error diagnostics would duplicate pass 1's, and lint
    // findings over a recovered tree are transient editor noise.
    if parsed.errors().is_empty() {
        let outcome = lint_source(&rel, index.text(), dialect, ctx.lint, ctx.known_globals);
        for diag in &outcome.diagnostics {
            // `lint_source` also carries the control-flow legality errors
            // (#44) — `LB0020`-`LB0022` from `luabox_hir::validate`, which are
            // not lint rules: they have no tier and no `---@luabox-ignore` id,
            // and the runtime refuses to load the file either way. They are
            // published under the toolchain source alongside the parse,
            // dialect and type diagnostics above; only the `LB05xx` rule
            // findings get the lint source, which is what the code-action
            // matcher keys its quick fixes off. The band is `luabox_diag`'s
            // to define ([`luabox_diag::Code::is_lint`]) — an open-coded
            // `number() / 100 == 5` here was a contract nothing asserted.
            //
            // Lint spans are always this file's: `lint_source` is handed
            // `rel` and this file's text and never resolves anything across
            // the project, so there is nothing here to route.
            out.own.push(convert(&index, diag));
        }
    }

    Some(out)
}

/// The workspace-derived half **alone**, for a publish that has no file to
/// check but must still leave the ledger's workspace slot correct.
///
/// The `didClose` of a document with no disk backing is that publish (round
/// 13 review R13-A): the overlay is dropped, so the declarations it was
/// contributing to the workspace class graph are gone, and a cycle that only
/// existed because of them is genuinely over. Recording "not recomputed"
/// there left the last answer standing — an `LB0318` naming a class nothing
/// declares any more, pinned to a valid open file, until any next pass
/// anywhere.
///
/// Same routing as [`diagnostics`]'s workspace half, over the same
/// caller-derived pass: this is the one answer about the whole workspace, so
/// publishing it needs no checked file at all.
#[must_use]
pub fn workspace_diagnostics(
    analysis: &Analysis,
    cycles: &ClassCycles,
) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    route_workspace(analysis, cycles, None, &mut HashMap::new())
}

/// File the workspace pass's diagnostics under the document each one's
/// declaration lives in, converted through **that** file's line index.
///
/// `checked` is the in-flight file's `(path, index)` when there is one, so a
/// cycle declared in the very file being checked reuses the index that pass
/// already built; `None` when the caller has no checked file
/// ([`workspace_diagnostics`]).
///
/// A finding whose declaring file `analysis` cannot resolve to text is
/// dropped, for the reason [`line_index_for`] gives: there is no index that
/// could place it and no document to place it on.
fn route_workspace(
    analysis: &Analysis,
    cycles: &ClassCycles,
    checked: Option<(&str, &LineIndex)>,
    foreign_indices: &mut HashMap<PathBuf, LineIndex>,
) -> BTreeMap<PathBuf, Vec<Diagnostic>> {
    let mut out: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for diag in &cycles.diags {
        let Some(label) = diag.primary_label() else {
            continue;
        };
        let named = label.span.file.clone();
        let Some(target_index) = line_index_for(analysis, checked, foreign_indices, &named) else {
            continue;
        };
        out.entry(PathBuf::from(named))
            .or_default()
            .push(convert(target_index, diag));
    }
    out
}

/// One workspace cycle pass: the `LB0318`s the declared `---@class` graph
/// alone proves, and the declaration sites they account for.
///
/// Public because the caller owns it now: it is derived once per host
/// revision and handed to every publish through [`CheckCtx::cycles`], rather
/// than recomputed inside each [`diagnostics`] call. The contents stay
/// private — a caller only ever passes one back in.
pub struct ClassCycles {
    /// One diagnostic per declaration that carries a cycle edge, its primary
    /// span naming the **declaring** file — routed by [`push_routed`] like
    /// every other cross-file ancestry finding.
    diags: Vec<luabox_diag::Diagnostic>,
    /// `(declaring file, span)` of every cycle member's declaration, the
    /// reported and the merely-accounted-for alike.
    covered: HashSet<(String, usize, usize)>,
}

impl ClassCycles {
    /// Whether `diag` is the resolver's rediscovery of a cycle this pass has
    /// already named at the same declaration.
    fn covers(&self, diag: &luabox_diag::Diagnostic) -> bool {
        diag.code == CYCLIC_CLASS
            && diag.primary_label().is_some_and(|label| {
                self.covered.contains(&(
                    label.span.file.clone(),
                    label.span.range.start,
                    label.span.range.end,
                ))
            })
    }
}

/// The editor's half of `LB0318`: every `---@class` in the workspace that is
/// its own ancestor, reported at its own declaration, whether or not
/// anything in the project ever resolves it (round 12 review R12-1).
///
/// # Why this exists
///
/// `LB0318` has two mechanisms. One is resolution-driven — `env::note_cyclic`
/// files a hit when `DiamondGuard` takes a back-edge — and it is the only one
/// this server used to run. Resolution is *reference*-driven, so a types file
/// containing nothing but `---@class Widget : Widget` resolved nothing, filed
/// nothing, and the editor stayed green on the exact fixture the feature
/// targets, while `luabox check` reported it. That is the editor-vs-CLI split
/// this repo treats as blocking (`check.rs`: "rather than leaving `luabox
/// check` green on a project the LSP reports red on"), inverted — and a
/// shipped doc comment claimed the opposite was true.
///
/// The algorithm is not this module's: [`luabox_types::ClassGraph`] owns the
/// strongly-connected-component walk, the singleton-self-edge filter, the
/// per-declaration report rule and the message, and `luabox check` runs the
/// very same seam over the very same graph shape. What is this module's is
/// the workspace the graph is built from and the editor's routing.
///
/// # Cost
///
/// One pass over every project file's **memoized** annotation harvest
/// (`Analysis::annotations` is a salsa query — an unchanged file is a
/// refcount bump, not a re-harvest), plus a `HashMap` walk proportional to
/// the declared class graph — the same order of cost as the merged-ambient
/// rebuild (`Server::merged_ambient`'s own doc measures that one), and paid
/// on the same schedule: once per host revision, cached beside it
/// (`Server::cycle_pass`), not once per publish. The version this replaces
/// ran here, inside every [`diagnostics`] call, so a project's whole class
/// graph was re-collected and re-Tarjan'd on every keystroke of every open
/// buffer for an answer that cannot differ between them (round 13 review,
/// noted cost).
///
/// The `---@diagnostic disable[-line|-next-line]: circle-doc-class` escape
/// hatch is honoured out of the **declaring** file's own text, through
/// [`luabox_types::DirectiveScan`] — the same scanner, the same luals rule
/// name and the same 1-based line convention the CLI pre-check uses, so a
/// directive that silences the finding in CI silences it in the editor.
///
/// Empty under [`Strictness::None`], which disables every type diagnostic
/// project-wide.
#[must_use]
pub fn class_cycle_diagnostics(analysis: &Analysis, strictness: Strictness) -> ClassCycles {
    let mut cycles = ClassCycles {
        diags: Vec::new(),
        covered: HashSet::new(),
    };
    if strictness == Strictness::None {
        return cycles;
    }
    let severity = if strictness == Strictness::Strict {
        luabox_diag::Severity::Error
    } else {
        luabox_diag::Severity::Warning
    };

    // Sorted, so a file's index — and so the order of the sites below — is a
    // function of the workspace and not of the host's file map.
    let mut files: Vec<PathBuf> = analysis.files().map(Path::to_path_buf).collect();
    files.sort();
    let mut graph = ClassGraph::default();
    for (idx, file) in files.iter().enumerate() {
        let Some(items) = analysis.annotations(file) else {
            continue;
        };
        // The per-item rule is the seam's, not this module's (round 13 review
        // R13-C): `Tag::Class`, the empty-name drop and the bare-`Named`
        // parent filter used to be open-coded here AND in `check_cmd`, so a
        // widened parent shape applied to one surface would have moved the
        // editor and not the command line, silently — the very split this
        // pass exists to close.
        graph.declare_file(idx, items.items());
    }

    // Built only for the files a cycle actually touches — a workspace with
    // no cycle scans nothing.
    let mut suppression: HashMap<PathBuf, Option<(DirectiveScan, luabox_syntax::LineIndex)>> =
        HashMap::new();
    for site in graph.cycle_sites() {
        let Some(decl) = files.get(site.file) else {
            continue;
        };
        let named = decl.to_string_lossy().into_owned();
        cycles
            .covered
            .insert((named.clone(), site.span.start, site.span.end));
        if !site.reports {
            continue;
        }
        let scan = suppression.entry(decl.clone()).or_insert_with(|| {
            let text = analysis.file_text(decl)?;
            let lines = luabox_syntax::LineIndex::new(&text);
            Some((DirectiveScan::scan(&text), lines))
        });
        if let Some((rules, lines)) = scan.as_ref()
            && rules.suppresses(RULE_CIRCLE_DOC_CLASS, lines.line_of(site.span.start))
        {
            continue;
        }
        cycles.diags.push(
            luabox_diag::Diagnostic::new(
                CYCLIC_CLASS,
                severity,
                luabox_types::cyclic_class_message(&site.name, &site.others),
            )
            .with_label(luabox_diag::Label::primary(
                luabox_diag::Span::new(named.as_str(), site.span.clone()),
                luabox_types::CYCLIC_CLASS_LABEL,
            )),
        );
    }
    cycles
}

/// Convert `diag` through the line index of the file its **primary span**
/// names, and file it under [`FileDiagnostics::own`] or
/// [`FileDiagnostics::foreign`] accordingly (production readiness review,
/// finding 4).
///
/// See [`line_index_for`] for the memoisation and for what happens to a span
/// naming a file `analysis` cannot resolve to text.
fn push_routed(
    out: &mut FileDiagnostics,
    analysis: &Analysis,
    rel: &str,
    index: &LineIndex,
    foreign_indices: &mut HashMap<PathBuf, LineIndex>,
    diag: &luabox_diag::Diagnostic,
) {
    let Some(label) = diag.primary_label() else {
        // No span at all: `convert` maps it to 0..0, which is as true of
        // this file as of any other, so it stays with the checked file.
        out.own.push(convert(index, diag));
        return;
    };
    let named = label.span.file.clone();
    let Some(target_index) = line_index_for(analysis, Some((rel, index)), foreign_indices, &named)
    else {
        return;
    };
    let converted = convert(target_index, diag);
    if named == rel {
        out.own.push(converted);
    } else {
        out.foreign
            .entry(PathBuf::from(named))
            .or_default()
            .push(converted);
    }
}

/// The [`LineIndex`] a diagnostic naming `file` must be converted through:
/// the checked file's own when it names that, otherwise the named file's,
/// memoised in `foreign_indices`.
///
/// `checked` is `None` for a caller with no file in flight
/// ([`workspace_diagnostics`]), where every index is built from `analysis`.
///
/// `None` when `analysis` cannot resolve `file` to text — an ambient/defs
/// tier span, or a logical name that is not a project path. Such a
/// diagnostic is **dropped** by both callers: there is no line index that
/// could convert it and no document URI to publish it under, and the
/// alternative (convert it against whichever file happened to be checked) is
/// what produced a diagnostic at a clamped position in a file the message was
/// not about.
///
/// Memoised because building one is a full byte scan of that file's text, and
/// a cycle spanning N classes declared in the same file reports once per
/// class.
fn line_index_for<'a>(
    analysis: &Analysis,
    checked: Option<(&str, &'a LineIndex)>,
    foreign_indices: &'a mut HashMap<PathBuf, LineIndex>,
    file: &str,
) -> Option<&'a LineIndex> {
    if let Some((rel, index)) = checked
        && file == rel
    {
        return Some(index);
    }
    match foreign_indices.entry(PathBuf::from(file)) {
        Entry::Occupied(entry) => Some(entry.into_mut()),
        Entry::Vacant(entry) => {
            let text = analysis.file_text(entry.key())?;
            Some(entry.insert(LineIndex::new(text)))
        }
    }
}

/// Convert a toolchain [`luabox_diag::Diagnostic`] to an LSP diagnostic through
/// `index`. Shared by the type pass, the lint pass, and the code-action
/// matcher so a lint diagnostic offered on a quick-fix is byte-identical to
/// the one published for the same finding.
///
/// The `source` is *derived* here, from [`source_for`], rather than taken as a
/// parameter. It used to be a `&str` argument, and all three non-test callers
/// passed exactly `source_for(diag.code)` — an invariant held by convention
/// across two modules, which is the shape of both source-mismatch bugs the
/// previous rounds found (a hardcoded [`LINT_SOURCE`] at the code-action site
/// in round 5, a hardcoded [`TYPE_SOURCE`] at three publishers in round 6).
/// A caller can no longer pass a source that disagrees with the code, because
/// it can no longer pass one at all (Shockwave round 7, issue X).
pub(crate) fn convert(index: &LineIndex, diag: &luabox_diag::Diagnostic) -> Diagnostic {
    let source = source_for(diag.code);
    let range = diag
        .primary_label()
        .map_or(0..0, |label| label.span.range.clone());
    let severity = match diag.severity {
        luabox_diag::Severity::Error => DiagnosticSeverity::ERROR,
        luabox_diag::Severity::Warning => DiagnosticSeverity::WARNING,
    };
    let mut message = diag.message.clone();
    for note in &diag.notes {
        message.push('\n');
        message.push_str(note);
    }
    Diagnostic {
        range: index.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(diag.code.to_string())),
        source: Some(source.to_string()),
        message,
        ..Diagnostic::default()
    }
}

/// A diagnostic assembled from a code and a message rather than converted
/// from a [`luabox_diag::Diagnostic`] — the parse and dialect passes, which
/// carry their own error types.
///
/// The `source` comes from [`source_for`] like every other publisher's.
/// Neither caller can currently produce a code in the lint band, so this is
/// `TYPE_SOURCE` in practice; taking the code rather than a pre-rendered
/// string is what makes that a *derived* fact instead of a hardcoded one
/// (Shockwave round 6).
fn diagnostic(
    index: &LineIndex,
    range: std::ops::Range<usize>,
    severity: DiagnosticSeverity,
    code: luabox_diag::Code,
    message: String,
) -> Diagnostic {
    Diagnostic {
        range: index.range(range),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some(source_for(code).to_string()),
        message,
        ..Diagnostic::default()
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::fmt::Write as _;
    use std::path::PathBuf;

    use luabox_db::{AnalysisHost, Change};
    use luabox_types::build_ambient;

    fn path_for(name: &str) -> PathBuf {
        Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" }).join(name)
    }

    /// Diagnostics for `src` checked as `dialect`.
    fn diagnostics_for(src: &str, dialect: Dialect) -> Vec<Diagnostic> {
        diagnostics_with_rocks(src, dialect, &RockSurfaces::default())
    }

    /// [`diagnostics_for`] with harvested rock surfaces in the context (#30).
    fn diagnostics_with_rocks(
        src: &str,
        dialect: Dialect,
        rocks: &RockSurfaces,
    ) -> Vec<Diagnostic> {
        let mut host = AnalysisHost::new(dialect, Strictness::Warn);
        let path = path_for("main.lua");
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect,
            text: src.to_string(),
        });
        let analysis = host.snapshot();
        // The merged layer, exactly as the server's revision-keyed cache
        // builds it — the merge lives with the caller now, not in
        // `diagnostics()`.
        let base = build_ambient(dialect, &[]);
        let known_globals = base.global_names().clone();
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), rocks.types());
        let lint = LintConfig::new();
        let cycles = class_cycle_diagnostics(&analysis, Strictness::Warn);
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks,
            lint: &lint,
            known_globals: &known_globals,
            cycles: &cycles,
        };
        diagnostics(&analysis, &path, dialect, &ctx)
            .expect("diagnostics")
            .own
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags
            .iter()
            .filter_map(|d| match &d.code {
                Some(NumberOrString::String(code)) => Some(code.as_str()),
                _ => None,
            })
            .collect()
    }

    // === cross-file spans (production readiness review, finding 4) ========

    /// The two-file `---@class` cycle: `a.lua` declares `A : B` and resolves
    /// `B`; `c.lua` declares `B : A`, padded so its declaration sits at a
    /// byte offset `a.lua`'s text cannot even contain.
    ///
    /// Checking `a.lua` produces an `LB0318` about `B`, whose declaration is
    /// in `c.lua`. Converted against `a.lua`'s line index (what this used to
    /// do), that offset clamps to `a.lua`'s end and the diagnostic renders in
    /// `a.lua`'s panel, past its last line, under a message naming a class
    /// `a.lua` never declares. It must instead come back under `c.lua`'s
    /// path, at `c.lua`'s own line.
    ///
    /// `A` is a member of the same cycle and is declared in `a.lua`, so its
    /// own `LB0318` belongs in `a.lua`'s half — luals reports both
    /// declarations of a mutual cycle, one in each file (measured), and as of
    /// round 12 R12-1 so does this server. What this fixture is about is the
    /// ROUTING: `B`'s finding must not be rendered in `a.lua`.
    #[test]
    fn a_cross_file_ancestry_diagnostic_lands_on_its_own_file_at_its_own_line() {
        let (found, a_text, c_text) = cross_file_cycle();

        assert!(
            !found
                .own
                .iter()
                .any(|d| d.message.contains("`B`'s `---@class` ancestry")),
            "the cycle member declared in c.lua does not belong to a.lua: {:?}",
            found.own
        );
        let foreign = found
            .workspace
            .get(&path_for("c.lua"))
            .expect("the LB0318 is filed under the file its span names");
        assert_eq!(codes(foreign), vec!["LB0318"]);
        assert!(
            foreign[0].message.contains("`B`'s `---@class` ancestry"),
            "c.lua's group is c.lua's own declaration: {foreign:?}"
        );
        // `A`'s own half of the same cycle is filed under a.lua, at a.lua's
        // line — the parity behaviour, and the routing this fixture is about
        // works in both directions.
        let mine = found
            .workspace
            .get(&path_for("a.lua"))
            .expect("a.lua declares a member of the same cycle");
        assert_eq!(codes(mine), vec!["LB0318"]);
        assert!(
            mine[0].message.contains("`A`'s `---@class` ancestry"),
            "{mine:?}"
        );

        // `---@class B : A` is the last line of a padded file, and a.lua is
        // far shorter — a conversion through a.lua's index could not have
        // produced this line at all, only a clamp to a.lua's last one.
        let declared_on = u32::try_from(
            c_text
                .lines()
                .position(|line| line.starts_with("---@class B"))
                .expect("c.lua declares B"),
        )
        .expect("a small line count");
        assert_eq!(foreign[0].range.start.line, declared_on);
        assert!(
            declared_on > u32::try_from(a_text.lines().count()).expect("a small line count"),
            "the fixture only proves anything if c.lua's declaration is past \
             a.lua's last line"
        );

        // And a.lua's own panel carries only a.lua's own findings, none of
        // them clamped to its end.
        let last_line = u32::try_from(a_text.lines().count()).expect("a small line count");
        for diag in &found.own {
            assert!(
                diag.range.start.line < last_line,
                "nothing may render at a clamped position: {diag:?}"
            );
        }
    }

    /// A foreign span naming a file `analysis` cannot resolve to text has no
    /// line index to convert through and no document URI to publish under,
    /// so it is dropped rather than rendered somewhere arbitrary. Pinned via
    /// the same fixture with `c.lua` absent from the host: `a.lua` still
    /// checks, and no group is filed for a file that does not exist.
    #[test]
    fn a_foreign_span_naming_an_unknown_file_is_dropped_not_misplaced() {
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        host.set_root(root.to_path_buf());
        host.apply_change(Change::SetFileText {
            path: path_for("a.lua"),
            dialect: Dialect::Lua54,
            text: "---@class A : B\n\n---@type B\nlocal v = nil\nlocal _ = v.whatever\n"
                .to_string(),
        });
        let analysis = host.snapshot();
        let base = build_ambient(Dialect::Lua54, &[]);
        let known_globals = base.global_names().clone();
        let rocks = RockSurfaces::default();
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), rocks.types());
        let lint = LintConfig::new();
        let cycles = class_cycle_diagnostics(&analysis, Strictness::Warn);
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks: &rocks,
            lint: &lint,
            known_globals: &known_globals,
            cycles: &cycles,
        };
        let found =
            diagnostics(&analysis, &path_for("a.lua"), Dialect::Lua54, &ctx).expect("diagnostics");
        assert!(found.foreign.is_empty(), "nothing to file");
    }

    /// The finding-4 fixture: `(a.lua's diagnostics, a.lua text, c.lua text)`.
    fn cross_file_cycle() -> (FileDiagnostics, String, String) {
        let a_text =
            "---@class A : B\n\n---@type B\nlocal v = nil\nlocal _ = v.whatever\n".to_string();
        // Padding, so `B`'s declaration offset is well past a.lua's length —
        // the whole point of the fixture.
        let mut c_text = String::new();
        for i in 0..40 {
            let _ = writeln!(c_text, "-- padding line {i}");
        }
        c_text.push_str("---@class B : A\n---@field id number\n");

        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        host.set_root(root.to_path_buf());
        for (rel, text) in [("a.lua", &a_text), ("c.lua", &c_text)] {
            host.apply_change(Change::SetFileText {
                path: path_for(rel),
                dialect: Dialect::Lua54,
                text: text.clone(),
            });
        }
        let analysis = host.snapshot();
        let base = build_ambient(Dialect::Lua54, &[]);
        let known_globals = base.global_names().clone();
        let rocks = RockSurfaces::default();
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), rocks.types());
        let lint = LintConfig::new();
        let cycles = class_cycle_diagnostics(&analysis, Strictness::Warn);
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks: &rocks,
            lint: &lint,
            known_globals: &known_globals,
            cycles: &cycles,
        };
        let found =
            diagnostics(&analysis, &path_for("a.lua"), Dialect::Lua54, &ctx).expect("diagnostics");
        (found, a_text, c_text)
    }

    // === the declaration-driven cycle pass (round 12 review R12-1) ========

    /// Check `checked` in a workspace made of `files`, at `strictness` — the
    /// whole [`diagnostics`] pass, not just its workspace half (which is
    /// [`workspace_diagnostics`], a different function with a similar job).
    fn check_in_workspace(
        files: &[(&str, &str)],
        checked: &str,
        strictness: Strictness,
    ) -> FileDiagnostics {
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut host = AnalysisHost::new(Dialect::Lua54, strictness);
        host.set_root(root.to_path_buf());
        for (rel, text) in files {
            host.apply_change(Change::SetFileText {
                path: path_for(rel),
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        let analysis = host.snapshot();
        let base = build_ambient(Dialect::Lua54, &[]);
        let known_globals = base.global_names().clone();
        let rocks = RockSurfaces::default();
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), rocks.types());
        let lint = LintConfig::new();
        let cycles = class_cycle_diagnostics(&analysis, strictness);
        let ctx = CheckCtx {
            strictness,
            ambient: &ambient,
            rocks: &rocks,
            lint: &lint,
            known_globals: &known_globals,
            cycles: &cycles,
        };
        diagnostics(&analysis, &path_for(checked), Dialect::Lua54, &ctx).expect("diagnostics")
    }

    /// Every workspace-half message, in publish order.
    fn workspace_messages(found: &FileDiagnostics, rel: &str) -> Vec<String> {
        found
            .workspace
            .get(&path_for(rel))
            .into_iter()
            .flatten()
            .map(|d| d.message.clone())
            .collect()
    }

    /// R12-1, THE fixture: a types file containing nothing but a self-cyclic
    /// `---@class` — no `---@type`, no local, no member read, zero uses
    /// anywhere. `luabox check` has reported `LB0318` here since round 11;
    /// this server reported NOTHING, because its only mechanism was
    /// `env::note_cyclic`, which fires from a resolution walk, and nothing
    /// resolves a class no file references. Measured against the pinned
    /// lua-language-server 3.13.5: `circle-doc-class` fires on exactly this
    /// source. Editor and CI now agree, and both agree with the oracle.
    #[test]
    fn an_unreferenced_self_cycle_is_reported_in_the_editor() {
        let found = check_in_workspace(
            &[("types.lua", "---@class Widget : Widget\n")],
            "types.lua",
            Strictness::Warn,
        );
        let messages = workspace_messages(&found, "types.lua");
        assert_eq!(messages.len(), 1, "{found:?}", found = found.workspace);
        assert!(
            messages[0].contains("`Widget`'s `---@class` ancestry is cyclic"),
            "{messages:?}"
        );
        let published = &found.workspace[&path_for("types.lua")];
        assert_eq!(codes(published), vec!["LB0318"]);
        assert_eq!(published[0].source.as_deref(), Some(TYPE_SOURCE));
        assert_eq!(published[0].range.start.line, 0);
    }

    /// The mutual shape, with zero uses, across two files — a cycle no
    /// single declaration contains, found from the WORKSPACE graph rather
    /// than from whatever the checked file happens to resolve. Both members
    /// are reported, each in its own declaring file, which is what luals
    /// does (measured: one `circle-doc-class` per declaration).
    #[test]
    fn an_unreferenced_mutual_cycle_is_reported_on_both_declaring_files() {
        let found = check_in_workspace(
            &[
                ("a.lua", "---@class RingA : RingB\n"),
                ("b.lua", "---@class RingB : RingA\n"),
            ],
            "a.lua",
            Strictness::Warn,
        );
        assert!(
            workspace_messages(&found, "a.lua")[0].contains("`RingA`'s"),
            "{found:?}",
            found = found.workspace
        );
        assert!(
            workspace_messages(&found, "b.lua")[0].contains("`RingB`'s"),
            "the partner's half lands on the partner's document, whether or not it is open: {:?}",
            found.workspace
        );
    }

    /// The strictness ladder in the editor, and the escape hatch: the same
    /// `---@diagnostic disable: circle-doc-class` that silences the CLI
    /// silences this, scanned out of the DECLARING file's own text through
    /// `luabox_types::DirectiveScan`.
    #[test]
    fn the_editors_cycle_pass_honours_strictness_and_the_declaring_files_directive() {
        let cyclic = [("types.lua", "---@class Widget : Widget\n")];
        let strict = check_in_workspace(&cyclic, "types.lua", Strictness::Strict);
        assert_eq!(
            strict.workspace[&path_for("types.lua")][0].severity,
            Some(DiagnosticSeverity::ERROR)
        );
        let warn = check_in_workspace(&cyclic, "types.lua", Strictness::Warn);
        assert_eq!(
            warn.workspace[&path_for("types.lua")][0].severity,
            Some(DiagnosticSeverity::WARNING)
        );
        let off = check_in_workspace(&cyclic, "types.lua", Strictness::None);
        assert!(off.workspace.is_empty(), "{:?}", off.workspace);

        let suppressed = check_in_workspace(
            &[(
                "types.lua",
                "---@diagnostic disable: circle-doc-class\n---@class Widget : Widget\n",
            )],
            "types.lua",
            Strictness::Warn,
        );
        assert!(
            suppressed.workspace.is_empty(),
            "the declaring file's own directive suppresses it in the editor too: {:?}",
            suppressed.workspace
        );
    }

    /// A cycle something DOES resolve is discovered twice — syntactically by
    /// the workspace pass and again by `env::note_cyclic` during the
    /// resolution walk. One declaration is one diagnostic, exactly as under
    /// `luabox check`: the pass's `covered` set drops the resolver's
    /// rediscovery.
    #[test]
    fn a_resolved_cycle_is_reported_once_not_once_per_mechanism() {
        let found = check_in_workspace(
            &[(
                "main.lua",
                "---@class RingA : RingB\n---@field a string\n\
                 ---@class RingB : RingA\n---@field b string\n\
                 ---@type RingA\nlocal node = nil\nlocal _ = node.a\n",
            )],
            "main.lua",
            Strictness::Warn,
        );
        let cyclic: Vec<&Diagnostic> = found
            .own
            .iter()
            .chain(found.workspace.values().flatten())
            .filter(|d| d.code == Some(NumberOrString::String("LB0318".to_owned())))
            .collect();
        assert_eq!(
            cyclic.len(),
            2,
            "one per declaration, not per mechanism: {cyclic:?}"
        );
        for name in ["RingA", "RingB"] {
            assert_eq!(
                cyclic
                    .iter()
                    .filter(|d| d.message.contains(&format!("`{name}`'s")))
                    .count(),
                1,
                "{name} once: {cyclic:?}"
            );
        }
    }

    /// The same dedup, **across files** — the axis the same-file test above
    /// cannot reach (round 13 review, noted coverage gap).
    ///
    /// `covers` matches the resolver's finding against `(file, start, end)`,
    /// and the two sides derive that file string by different routes: the
    /// workspace pass takes it from `Analysis::files()`, the resolver from
    /// `TypeEnv::cross_file_class_decl_span`, which carries the name the
    /// declaring file was merged under. Same-file, both routes are the
    /// checked path and a mismatch is invisible; cross-file, a formatting
    /// difference between them would make every dedup miss and publish two
    /// `LB0318` for one declaration — one from each mechanism, at the same
    /// span, in different words.
    ///
    /// `a.lua` declares `A : B` and RESOLVES `B` (the `---@type` and the
    /// member read are what make the resolver walk it at all); `c.lua`
    /// declares `B : A`. Two declarations, two diagnostics, both from the
    /// workspace pass.
    #[test]
    fn a_cross_file_resolved_cycle_is_deduped_by_declaration_not_doubled() {
        let (found, _, _) = cross_file_cycle();
        let cyclic = |diags: &[Diagnostic]| {
            diags
                .iter()
                .filter(|d| d.code == Some(NumberOrString::String("LB0318".to_owned())))
                .count()
        };
        assert_eq!(
            cyclic(&found.own),
            0,
            "the resolver's rediscovery of `A`'s cycle is covered by the \
             workspace pass, so a.lua's own half carries none: {:?}",
            found.own
        );
        assert_eq!(
            found
                .foreign
                .values()
                .map(|group| cyclic(group))
                .sum::<usize>(),
            0,
            "and neither does the cross-file contribution half for `B`'s: {:?}",
            found.foreign
        );
        let published: usize = found.workspace.values().map(|g| cyclic(g)).sum();
        assert_eq!(
            published, 2,
            "one per declaration — and exactly one mechanism publishes them: {:?}",
            found.workspace
        );
    }

    /// The over-report guard on the editor path: an acyclic hierarchy —
    /// diamond, chain, and a subclass hanging off nothing cyclic — produces
    /// no cycle diagnostics at all.
    #[test]
    fn an_acyclic_workspace_publishes_no_cycle_diagnostics() {
        let found = check_in_workspace(
            &[
                ("top.lua", "---@class Top\n---@class Left : Top\n"),
                (
                    "bottom.lua",
                    "---@class Right : Top\n---@class Bottom : Left, Right\n",
                ),
            ],
            "top.lua",
            Strictness::Warn,
        );
        assert!(found.workspace.is_empty(), "{:?}", found.workspace);
    }

    #[test]
    fn a_dialect_violation_is_reported_against_the_project_edition() {
        // `<const>` is Lua 5.4 syntax; the same source is legal there.
        let src = "local x <const> = 1\nprint(x)\n";
        let under_51 = diagnostics_for(src, Dialect::Lua51);
        let gated = under_51
            .iter()
            .find(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "LB0013"))
            .unwrap_or_else(|| panic!("expected LB0013: {under_51:?}"));
        assert_eq!(gated.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(gated.source.as_deref(), Some(TYPE_SOURCE));
        assert!(gated.message.contains("Lua 5.1"), "{gated:?}");

        assert!(
            !codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0013"),
            "legal under 5.4"
        );
    }

    #[test]
    fn a_parse_error_is_reported_with_the_syntax_code() {
        let diags = diagnostics_for("local = 1\n", Dialect::Lua54);
        assert!(codes(&diags).contains(&"LB0001"), "{diags:?}");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diags[0].source.as_deref(), Some(TYPE_SOURCE));
    }

    #[test]
    fn lint_findings_carry_the_lint_source() {
        let diags = diagnostics_for("local unused = 1\n", Dialect::Lua54);
        let lint = diags
            .iter()
            .find(|d| d.source.as_deref() == Some(LINT_SOURCE))
            .unwrap_or_else(|| panic!("expected a lint diagnostic: {diags:?}"));
        // Through the same authority the tagging uses, not a string prefix.
        let code: luabox_diag::Code = codes(std::slice::from_ref(lint))[0]
            .parse()
            .unwrap_or_else(|_| panic!("unparseable code: {lint:?}"));
        assert!(code.is_lint(), "{lint:?}");
    }

    /// The other direction: a finding the lint engine carries but that is not
    /// a lint rule (`LB0020`-`LB0022`, control-flow legality) stays on the
    /// toolchain source, so the quick-fix matcher never offers a fix for it.
    #[test]
    fn control_flow_legality_does_not_get_the_lint_source() {
        let diags = diagnostics_for("local x = 1\nbreak\n", Dialect::Lua54);
        let found = diags
            .iter()
            .find(|d| d.code == Some(lsp_types::NumberOrString::String("LB0022".to_owned())))
            .unwrap_or_else(|| panic!("expected LB0022: {diags:?}"));
        assert_eq!(found.source.as_deref(), Some(TYPE_SOURCE));
        assert!(!luabox_diag::Code::new(22).is_lint());
    }

    /// The single decision both publishers share — the diagnostic stream and
    /// the code-action matcher, which re-converts the originating diagnostic
    /// to pair it with its fix. Asserted on the band boundary rather than on
    /// whichever codes happen to carry fixes today, because the point of the
    /// helper is the case that does not exist yet: a fix-carrying diagnostic
    /// outside the lint band, which the hardcoded source would mis-pair.
    #[test]
    fn the_published_source_follows_the_lint_band_in_both_directions() {
        for number in [1_u16, 22, 300, 306, 499, 600, 601, 1001] {
            let code = luabox_diag::Code::new(number);
            assert_eq!(source_for(code), TYPE_SOURCE, "LB{number:04} is not a lint");
        }
        for number in [500_u16, 501, 509, 510, 599] {
            let code = luabox_diag::Code::new(number);
            assert_eq!(source_for(code), LINT_SOURCE, "LB{number:04} is a lint");
        }
    }

    /// The invariant the helper exists for, asserted over the published
    /// stream rather than over the helper: *every* diagnostic this module
    /// emits carries the source its code implies. Two publishers used to
    /// bypass `source_for` and hardcode [`TYPE_SOURCE`] — harmless, since
    /// neither can produce an `LB05xx`, and exactly the kind of "correct for
    /// the codes that exist today" that the round-5 code-action bug was
    /// (Shockwave round 6). Routing them through the helper kills the class;
    /// this is what notices if one grows back.
    #[test]
    fn every_published_diagnostic_carries_the_source_its_code_implies() {
        // Between them these reach all four publishers: a recovered parse
        // error (LB0001), a dialect violation under 5.1 (LB0013), a type
        // diagnostic (LB03xx) and a lint finding (LB05xx). The lint pass is
        // skipped when the parse is dirty, so the parse-error case has to be
        // its own source.
        let sources = [
            "local y: = 3\n",
            "local x <const> = 1\nlocal unused = 2\nreturn x\n",
            "---@type string\nlocal s = 1\nlocal spare = 2\nreturn s\n",
        ];
        let mut bands = HashSet::new();
        let mut seen = 0_usize;
        for src in sources {
            for diag in &diagnostics_for(src, Dialect::Lua51) {
                let Some(NumberOrString::String(spelling)) = &diag.code else {
                    panic!("every diagnostic carries an LBnnnn code: {diag:?}");
                };
                let code: luabox_diag::Code = spelling.parse().expect("a well-formed code");
                assert_eq!(
                    diag.source.as_deref(),
                    Some(source_for(code)),
                    "{spelling} published under the wrong source: {diag:?}"
                );
                bands.insert(code.is_lint());
                seen += 1;
            }
        }
        assert!(seen >= 4, "expected a mixed stream, saw {seen}");
        assert_eq!(bands.len(), 2, "both bands must be represented");
    }

    // --- harvested rock surfaces (#30) -----------------------------------

    /// The surfaces of one annotated rock installed as `mylib`.
    fn mylib_rock() -> RockSurfaces {
        let source = luabox_types::RockModule {
            module: "mylib".to_string(),
            label: "lua_modules/share/lua/5.4/mylib/init.lua".to_string(),
            path: path_for("lua_modules/share/lua/5.4/mylib/init.lua"),
            text: "\
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
"
            .to_string(),
        };
        let ambient = build_ambient(Dialect::Lua54, &[]);
        luabox_types::rocks::harvest(&ambient, &[source])
    }

    #[test]
    fn a_harvested_rock_class_resolves_in_the_editor() {
        let src = "---@type mylib.Point\nlocal p = { x = 1, y = 2 }\nreturn p\n";
        // Without the tree the class is an unknown type name…
        assert!(
            codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0305"),
            "expected LB0305 without a rock tree"
        );
        // …and with it, it resolves and the literal conforms.
        let with_rock = diagnostics_with_rocks(src, Dialect::Lua54, &mylib_rock());
        assert!(codes(&with_rock).is_empty(), "{with_rock:?}");
    }

    #[test]
    fn a_rock_module_export_types_a_require_the_database_cannot_resolve() {
        // `mylib` is not a project file, so the db resolves nothing; the rock's
        // export type is matched by module name instead, and its `---@return`
        // flows into the consumer's use site.
        let src = "\
---@param s string
local function want(s) end
local mylib = require(\"mylib\")
want(mylib.point(1, 2))
";
        let diags = diagnostics_with_rocks(src, Dialect::Lua54, &mylib_rock());
        assert!(
            codes(&diags).contains(&"LB0300"),
            "the rock's return type must reach the consumer: {diags:?}"
        );
        // The same source without the tree cannot know what `mylib` is.
        assert!(!codes(&diagnostics_for(src, Dialect::Lua54)).contains(&"LB0300"));
    }

    #[test]
    fn a_file_the_analysis_does_not_know_has_no_diagnostics() {
        let host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let analysis = host.snapshot();
        let base = build_ambient(Dialect::Lua54, &[]);
        let known_globals = base.global_names().clone();
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), &[]);
        let lint = LintConfig::new();
        let rocks = RockSurfaces::default();
        let cycles = class_cycle_diagnostics(&analysis, Strictness::Warn);
        let ctx = CheckCtx {
            strictness: Strictness::Warn,
            ambient: &ambient,
            rocks: &rocks,
            lint: &lint,
            known_globals: &known_globals,
            cycles: &cycles,
        };
        assert!(diagnostics(&analysis, &path_for("absent.lua"), Dialect::Lua54, &ctx).is_none());
    }
}
