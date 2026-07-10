//! Loading and resolving `.luab` shape modules from disk (SHAPES.md §6).
//!
//! `---@use <name>` (and `use` inside `.luab`) resolves, first hit wins:
//!
//! 1. a sibling `<name>.luab` next to the using file;
//! 2. the `[types] shape-paths` directories, in manifest order — more than
//!    one hit *within* this tier is an ambiguity error (`LB2005`);
//! 3. dependency-exported shapes: a dependency exports modules by listing
//!    them in `[types] shapes` in *its own* manifest ([`DepShapeExport`]).
//!    Only exported names are resolvable across the package boundary; the
//!    exported `<name>.luab` is looked up at the dependency root and then on
//!    the dependency's own `[types] shape-paths`. Two dependencies exporting
//!    the same name is an ambiguity error, listing both.
//!
//! A dependency's own `.luab` file resolves *its* nested `use`s within that
//! dependency package only — its sibling directory plus its shape-paths, and
//! **not** the dependency's own dependencies (resolution stops one level
//! deep; a dependency-of-a-dependency's exports are invisible).
//!
//! Parsed modules are cached behind a mutex so rayon workers checking many
//! `.lua` files share one parse per `.luab` file.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use luabox_diag::{Code, Diagnostic, Label, Span};

use super::raw::{self, RawModule};
use super::scope::{ShapeScope, build_scope};

const UNRESOLVED_USE: u16 = 2005;
const SYNTAX_ERROR: u16 = 1;
const BODY_IN_LB: u16 = 2010;

/// A dependency that may export shape modules to the current project
/// (SHAPES.md §6, resolution tier 3). Built by the CLI from the project
/// manifest plus each dependency's own `[types] shapes` declaration — the
/// shape store never parses manifests itself (SHAPES.md §7: distribution
/// ships `.luab` as opaque source; the store only reads `.luab` files).
#[derive(Debug, Clone)]
pub struct DepShapeExport {
    /// The dependency's package name (used to disambiguate candidates and in
    /// diagnostics).
    pub name: String,
    /// The dependency's package root on disk: `lua_modules/<name>/` for an
    /// installed dependency, or the path itself for a path dependency.
    pub root: PathBuf,
    /// The module names the dependency exports (`[types] shapes` in the
    /// dependency's own manifest). Only these names are resolvable across the
    /// package boundary — a `.luab` file the dependency does not export is
    /// invisible to dependents.
    pub exported: Vec<String>,
    /// The dependency's own `[types] shape-paths`, made absolute against its
    /// root. An exported `<mod>.luab` is looked up at the root and then on
    /// these paths; they also bound a dependency module's own nested `use`s
    /// (which resolve within the dependency package only).
    pub shape_paths: Vec<PathBuf>,
}

/// The empty dependency set — the resolution context for a `.luab` file that
/// lives inside a dependency, which never chases further dependency exports.
const NO_DEPS: &[DepShapeExport] = &[];

/// The outcome of resolving one module name.
pub(crate) enum ResolveOutcome {
    /// Exactly one `.luab` file matched.
    Found(PathBuf),
    /// More than one candidate in the same tier.
    Ambiguous(Vec<PathBuf>),
    /// No tier produced a candidate.
    NotFound,
}

/// Resolve a module name per the tier order above.
pub(crate) fn resolve(
    name: &str,
    sibling_dir: &Path,
    shape_paths: &[PathBuf],
    dependencies: &[DepShapeExport],
) -> ResolveOutcome {
    let file_name = format!("{name}.luab");

    // Tier 1: sibling.
    let sibling = sibling_dir.join(&file_name);
    if sibling.is_file() {
        return ResolveOutcome::Found(sibling);
    }

    // Tier 2: shape-paths (all dirs checked; >1 hit is ambiguous).
    let hits: Vec<PathBuf> = shape_paths
        .iter()
        .map(|dir| dir.join(&file_name))
        .filter(|p| p.is_file())
        .collect();
    match hits.len() {
        0 => {}
        1 => return ResolveOutcome::Found(hits.into_iter().next().expect("one hit")),
        _ => return ResolveOutcome::Ambiguous(hits),
    }

    // Tier 3: dependency-exported shapes. A dependency contributes a
    // candidate only when it *exports* this module name and the `<name>.luab`
    // is present at its root or on its own shape-paths; two exporting
    // dependencies are ambiguous, listing both.
    let dep_hits: Vec<PathBuf> = dependencies
        .iter()
        .filter(|dep| dep.exported.iter().any(|m| m == name))
        .filter_map(|dep| locate_in_dep(dep, &file_name))
        .collect();
    match dep_hits.len() {
        0 => ResolveOutcome::NotFound,
        1 => ResolveOutcome::Found(dep_hits.into_iter().next().expect("one hit")),
        _ => ResolveOutcome::Ambiguous(dep_hits),
    }
}

/// Locate an exported `file_name` within one dependency: at its root first,
/// then on its own `[types] shape-paths`, first hit wins.
fn locate_in_dep(dep: &DepShapeExport, file_name: &str) -> Option<PathBuf> {
    let at_root = dep.root.join(file_name);
    if at_root.is_file() {
        return Some(at_root);
    }
    dep.shape_paths
        .iter()
        .map(|dir| dir.join(file_name))
        .find(|p| p.is_file())
}

/// The resolution context for a module already located on disk. A module
/// physically inside a dependency's root resolves its nested `use`s within
/// that dependency alone (the dependency's shape-paths, no further
/// dependency exports — one level, SHAPES.md §6). Every other module is a
/// project file and sees the full project context (tiers 1–3).
fn context_for<'a>(
    path: &Path,
    project_paths: &'a [PathBuf],
    dependencies: &'a [DepShapeExport],
) -> (&'a [PathBuf], &'a [DepShapeExport]) {
    for dep in dependencies {
        if path.starts_with(&dep.root) {
            return (dep.shape_paths.as_slice(), NO_DEPS);
        }
    }
    (project_paths, dependencies)
}

/// A process-wide cache of parsed shape modules, shared across the per-file
/// check workers. Also the entry point for checking `.luab` files themselves.
#[derive(Debug)]
pub struct ShapeStore {
    /// Project root — diagnostics name files relative to it.
    root: PathBuf,
    cache: Mutex<HashMap<PathBuf, Arc<RawModule>>>,
}

impl ShapeStore {
    /// A store for the project rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ShapeStore {
            root: root.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Root-relative display path with forward slashes (stable diagnostics).
    fn display_rel(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        rel.to_string_lossy().replace('\\', "/")
    }

    /// Load (or fetch from cache) the module at `path`. `None` when the
    /// file cannot be read.
    fn load(&self, path: &Path) -> Option<Arc<RawModule>> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(hit) = self.cache.lock().expect("shape cache poisoned").get(&key) {
            return Some(Arc::clone(hit));
        }
        let source = std::fs::read_to_string(path).ok()?;
        let module = Arc::new(raw::parse_module(
            &source,
            self.display_rel(path),
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
        ));
        self.cache
            .lock()
            .expect("shape cache poisoned")
            .insert(key, Arc::clone(&module));
        Some(module)
    }

    /// Build the merged scope reachable from `roots` (following transitive
    /// `use`s, cycle-safe). Unresolved nested `use`s become `LB2005`
    /// diagnostics *inside the scope* (attributed to the declaring `.luab`
    /// file — surfaced when that file itself is checked).
    pub(crate) fn scope_from(
        &self,
        roots: &[PathBuf],
        shape_paths: &[PathBuf],
        dependencies: &[DepShapeExport],
    ) -> ShapeScope {
        let mut modules: Vec<Arc<RawModule>> = Vec::new();
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut queue: Vec<PathBuf> = roots.to_vec();
        let mut use_diags: Vec<Diagnostic> = Vec::new();

        while let Some(path) = queue.pop() {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !visited.insert(key) {
                continue;
            }
            let Some(module) = self.load(&path) else {
                continue;
            };
            // A dependency's own `.luab` resolves its nested `use`s within that
            // dependency package only; a project `.luab` sees the full project
            // context (tiers 1–3).
            let (ctx_paths, ctx_deps) = context_for(&path, shape_paths, dependencies);
            for import in &module.uses {
                match resolve(&import.path, &module.dir, ctx_paths, ctx_deps) {
                    ResolveOutcome::Found(next) => queue.push(next),
                    outcome => use_diags.push(self.unresolved_use(
                        &import.path,
                        &module.file,
                        import.range.clone(),
                        &outcome,
                    )),
                }
            }
            modules.push(module);
        }

        // Deterministic order: alphabetical by file, roots resolved first
        // by construction of the maps (first declaration of a name wins, so
        // sort for stability).
        modules.sort_by(|a, b| a.file.cmp(&b.file));
        let mut scope = build_scope(&modules);
        scope.diags.extend(use_diags);
        scope
    }

    /// The `LB2005` diagnostic for one unresolved/ambiguous `use`.
    pub(crate) fn unresolved_use(
        &self,
        name: &str,
        file: &str,
        range: Range<usize>,
        outcome: &ResolveOutcome,
    ) -> Diagnostic {
        match outcome {
            ResolveOutcome::Ambiguous(paths) => {
                let listed: Vec<String> = paths.iter().map(|p| self.display_rel(p)).collect();
                Diagnostic::error(
                    Code::new(UNRESOLVED_USE),
                    format!(
                        "ambiguous shape module `{name}`: multiple candidates in the same tier"
                    ),
                )
                .with_label(Label::primary(
                    Span::new(file, range),
                    "resolved by more than one shape source in the same tier",
                ))
                .with_note(format!("candidates: {}", listed.join(", ")))
            }
            _ => Diagnostic::error(
                Code::new(UNRESOLVED_USE),
                format!("cannot resolve shape module `{name}`"),
            )
            .with_label(Label::primary(
                Span::new(file, range),
                format!(
                    "no `{name}.luab` next to this file, on `[types] shape-paths`, or exported by a \
                     dependency"
                ),
            ))
            .with_note(
                "resolution checks, in order: a sibling `.luab`, the `[types] shape-paths` \
                 directories, and shape modules a dependency exports via `[types] shapes` in \
                 its own manifest",
            ),
        }
    }

    /// Check one `.luab` file: parse diagnostics (`LB2010` body rejection,
    /// plain syntax errors as `LB0001`) plus the shape-level diagnostics its
    /// declarations raise (unresolved `use` → `LB2005`, generic bound
    /// violations → `LB2007`).
    ///
    /// # Panics
    ///
    /// Panics if the internal module cache mutex was poisoned by a panic
    /// on another checking thread.
    #[must_use]
    pub fn check_lb_file(
        &self,
        path: &Path,
        source: &str,
        shape_paths: &[PathBuf],
        dependencies: &[DepShapeExport],
    ) -> Vec<Diagnostic> {
        let rel = self.display_rel(path);
        let module = raw::parse_module(
            source,
            rel.clone(),
            path.parent().map(Path::to_path_buf).unwrap_or_default(),
        );

        let mut diags: Vec<Diagnostic> = Vec::new();
        for err in &module.errors {
            let code = match err.code {
                Some("LB2010") => Code::new(BODY_IN_LB),
                _ => Code::new(SYNTAX_ERROR),
            };
            diags.push(
                Diagnostic::error(code, err.message.clone()).with_label(Label::primary(
                    Span::new(rel.clone(), err.range.clone()),
                    "not allowed in a shape file",
                )),
            );
        }

        // Lower with the full reachable scope so cross-module bounds check;
        // keep only diagnostics belonging to *this* file (each `.luab` file
        // reports its own problems exactly once).
        // Seed the cache with the freshly parsed module text.
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.cache
            .lock()
            .expect("shape cache poisoned")
            .insert(key, Arc::new(module));
        let scope = self.scope_from(&[path.to_path_buf()], shape_paths, dependencies);
        diags.extend(
            scope
                .diags
                .into_iter()
                .filter(|d| d.primary_label().is_some_and(|l| l.span.file == rel)),
        );
        diags.sort_by_key(|d| d.primary_label().map_or(0, |l| l.span.range.start));
        diags
    }
}
