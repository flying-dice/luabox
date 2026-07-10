//! Loading and resolving `.lb` shape modules from disk (SHAPES.md §6).
//!
//! `---@use <name>` (and `use` inside `.lb`) resolves, first hit wins:
//!
//! 1. a sibling `<name>.lb` next to the using file;
//! 2. the `[types] shape-paths` directories, in manifest order — more than
//!    one hit *within* this tier is an ambiguity error (`LB2005`);
//! 3. dependency-exported shapes — **not implemented yet** (P2, `[types]
//!    shapes`); unresolved names get an `LB2005` with a note saying so.
//!
//! Parsed modules are cached behind a mutex so rayon workers checking many
//! `.lua` files share one parse per `.lb` file.

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

/// The outcome of resolving one module name.
pub(crate) enum ResolveOutcome {
    /// Exactly one `.lb` file matched.
    Found(PathBuf),
    /// More than one candidate in the same tier.
    Ambiguous(Vec<PathBuf>),
    /// No tier produced a candidate.
    NotFound,
}

/// Resolve a module name per the tier order above.
pub(crate) fn resolve(name: &str, sibling_dir: &Path, shape_paths: &[PathBuf]) -> ResolveOutcome {
    // Dotted paths are dependency-exported shapes — P2.
    if name.contains('.') {
        return ResolveOutcome::NotFound;
    }
    let file_name = format!("{name}.lb");

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
        0 => ResolveOutcome::NotFound,
        1 => ResolveOutcome::Found(hits.into_iter().next().expect("one hit")),
        _ => ResolveOutcome::Ambiguous(hits),
    }
}

/// A process-wide cache of parsed shape modules, shared across the per-file
/// check workers. Also the entry point for checking `.lb` files themselves.
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
    /// diagnostics *inside the scope* (attributed to the declaring `.lb`
    /// file — surfaced when that file itself is checked).
    pub(crate) fn scope_from(&self, roots: &[PathBuf], shape_paths: &[PathBuf]) -> ShapeScope {
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
            for import in &module.uses {
                match resolve(&import.path, &module.dir, shape_paths) {
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
                    "resolved by more than one `[types] shape-paths` directory",
                ))
                .with_note(format!("candidates: {}", listed.join(", ")))
            }
            _ => Diagnostic::error(
                Code::new(UNRESOLVED_USE),
                format!("cannot resolve shape module `{name}`"),
            )
            .with_label(Label::primary(
                Span::new(file, range),
                format!("no `{name}.lb` next to this file or on `[types] shape-paths`"),
            ))
            .with_note(
                "dependency-exported shapes (`[types] shapes` in a dependency's manifest) are \
                 not searched yet — that resolution tier lands in P2",
            ),
        }
    }

    /// Check one `.lb` file: parse diagnostics (`LB2010` body rejection,
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
        // keep only diagnostics belonging to *this* file (each `.lb` file
        // reports its own problems exactly once).
        // Seed the cache with the freshly parsed module text.
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.cache
            .lock()
            .expect("shape cache poisoned")
            .insert(key, Arc::new(module));
        let scope = self.scope_from(&[path.to_path_buf()], shape_paths);
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
