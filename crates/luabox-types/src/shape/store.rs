//! Loading `.luab` shape modules from disk (SHAPES-V2.md).
//!
//! There are no imports in v2 — the package's type scope is **ambient**:
//! every `.luab` file under every `[types] shape-paths` directory is loaded,
//! and each module's dot-separated namespace is derived from its path
//! (`shapes/love/graphics.luab` → `love.graphics`).
//!
//! Dependencies contribute their **exported surface**: the `export type`
//! declarations of the entrypoint module named by `[types] entry` in the
//! dependency's own manifest, deep-resolved to self-contained structural
//! types and mounted under the dependency's package name (`geometry.Point`).
//! A dependency's internal module paths are not addressable from outside,
//! and resolution stops one level deep — a dependency-of-a-dependency's
//! exports are invisible.
//!
//! Parsed modules are cached behind a mutex so rayon workers checking many
//! `.lua` files share one parse per `.luab` file.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use luabox_diag::{Code, Diagnostic, Label, Span};

use super::raw::{self, RawModule};
use super::scope::{ShapeScope, TypeShape, build_scope, fq_name};

const SYNTAX_ERROR: u16 = 1;
const BODY_IN_LB: u16 = 2010;

/// A dependency that may export a type surface to the current project.
/// Built by the CLI from the project manifest plus each dependency's own
/// `[types] entry` declaration — the shape store never parses manifests
/// itself (distribution ships `.luab` as opaque source; the store only
/// reads `.luab` files).
#[derive(Debug, Clone)]
pub struct DepShapeExport {
    /// The dependency's package name — the namespace root its exports mount
    /// under (`geometry.Point`).
    pub name: String,
    /// The dependency's `[types] entry` `.luab` file, made absolute against
    /// its package root. `None` when the dependency exports no types.
    pub entry: Option<PathBuf>,
    /// The dependency's own `[types] shape-paths`, made absolute against its
    /// root — the ambient scope the entry's re-exports resolve within.
    pub shape_paths: Vec<PathBuf>,
}

/// Derive a module's dotted namespace from its path relative to the
/// `shape-paths` directory that contains it: `love/graphics.luab` →
/// `love.graphics`.
fn namespace_of(shape_path: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(shape_path).unwrap_or(file);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = parts.last_mut()
        && let Some(stem) = last.strip_suffix(".luab")
    {
        *last = stem.to_string();
    }
    parts.join(".")
}

/// Recursively collect every `.luab` file under `dir`, sorted for
/// deterministic scope construction.
fn collect_luab(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_luab(&path, out);
        } else if path.extension().is_some_and(|e| e == "luab") {
            out.push(path);
        }
    }
}

/// A process-wide cache of parsed shape modules, shared across the per-file
/// check workers. Also the entry point for checking `.luab` files themselves.
#[derive(Debug)]
pub struct ShapeStore {
    /// Project root — diagnostics name files relative to it.
    root: PathBuf,
    cache: Mutex<HashMap<PathBuf, Arc<RawModule>>>,
    /// The ambient scope, built once per `shape_paths` set and shared across
    /// every per-file check (the v2 zero-cost invariant: a file that never
    /// names a shape type pays a lookup miss, not scope construction).
    scopes: Mutex<HashMap<Vec<PathBuf>, Arc<ShapeScope>>>,
}

impl ShapeStore {
    /// A store for the project rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ShapeStore {
            root: root.into(),
            cache: Mutex::new(HashMap::new()),
            scopes: Mutex::new(HashMap::new()),
        }
    }

    /// Root-relative display path with forward slashes (stable diagnostics).
    fn display_rel(&self, path: &Path) -> String {
        let rel = path.strip_prefix(&self.root).unwrap_or(path);
        rel.to_string_lossy().replace('\\', "/")
    }

    /// Load (or fetch from cache) the module at `path` with `namespace`.
    /// `None` when the file cannot be read.
    fn load(&self, path: &Path, namespace: &str) -> Option<Arc<RawModule>> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(hit) = self.cache.lock().expect("shape cache poisoned").get(&key) {
            return Some(Arc::clone(hit));
        }
        let source = std::fs::read_to_string(path).ok()?;
        let module = Arc::new(raw::parse_module(
            &source,
            self.display_rel(path),
            namespace.to_string(),
        ));
        self.cache
            .lock()
            .expect("shape cache poisoned")
            .insert(key, Arc::clone(&module));
        Some(module)
    }

    /// Load every module under `shape_paths`, namespaces derived from paths.
    fn load_all(&self, shape_paths: &[PathBuf]) -> Vec<Arc<RawModule>> {
        let mut modules = Vec::new();
        for shape_path in shape_paths {
            let mut files = Vec::new();
            collect_luab(shape_path, &mut files);
            for file in files {
                let ns = namespace_of(shape_path, &file);
                if let Some(module) = self.load(&file, &ns) {
                    modules.push(module);
                }
            }
        }
        modules.sort_by(|a, b| a.file.cmp(&b.file));
        modules
    }

    /// Build (or fetch) the ambient package scope: every module under
    /// `shape_paths` plus each dependency's exported surface. Built once per
    /// `shape_paths` set and shared.
    ///
    /// # Panics
    ///
    /// Panics if an internal cache mutex was poisoned by a panic on another
    /// checking thread.
    #[must_use]
    pub fn package_scope(
        &self,
        shape_paths: &[PathBuf],
        dependencies: &[DepShapeExport],
    ) -> Arc<ShapeScope> {
        let key = shape_paths.to_vec();
        if let Some(hit) = self.scopes.lock().expect("scope cache poisoned").get(&key) {
            return Arc::clone(hit);
        }
        let modules = self.load_all(shape_paths);
        let mut dep_types: BTreeMap<String, TypeShape> = BTreeMap::new();
        for dep in dependencies {
            dep_types.extend(self.export_surface(dep));
        }
        let scope = Arc::new(build_scope(&modules, dep_types));
        self.scopes
            .lock()
            .expect("scope cache poisoned")
            .insert(key, Arc::clone(&scope));
        scope
    }

    /// A dependency's exported surface: the `export type` declarations of
    /// its entrypoint module, deep-resolved to self-contained structural
    /// types, keyed under `depname.Name`. Resolution runs inside the
    /// dependency's own ambient scope with **no** further dependencies
    /// (one level deep).
    fn export_surface(&self, dep: &DepShapeExport) -> BTreeMap<String, TypeShape> {
        let mut out = BTreeMap::new();
        let Some(entry) = &dep.entry else {
            return out;
        };
        let dep_scope = build_scope(&self.load_all(&dep.shape_paths), BTreeMap::new());
        // The entry module's namespace, derived like any other module's.
        let entry_ns = dep
            .shape_paths
            .iter()
            .find(|p| entry.starts_with(p))
            .map_or_else(
                || {
                    entry
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                },
                |p| namespace_of(p, entry),
            );
        for shape in dep_scope.types.values() {
            if !shape.export {
                continue;
            }
            // Only the entrypoint's exports cross the package boundary.
            let Some(short) = shape.name.strip_prefix(&fq_name(&entry_ns, "")) else {
                continue;
            };
            if short.contains('.') {
                continue;
            }
            let mounted = fq_name(&dep.name, short);
            out.insert(
                mounted.clone(),
                TypeShape {
                    name: mounted,
                    export: true,
                    params: shape.params.clone(),
                    // Deep-resolve so the surface is self-contained: internal
                    // FQ names are not addressable from outside the package.
                    ty: dep_scope.structural(&shape.ty),
                    file: shape.file.clone(),
                    range: shape.range.clone(),
                },
            );
        }
        out
    }

    /// Check one `.luab` file: parse diagnostics (`LB2010` body rejection,
    /// plain syntax errors as `LB0001`) plus the shape-level diagnostics its
    /// declarations raise (duplicate declarations → `LB2005`, bad
    /// instantiations → `LB2007`).
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
        let ns = shape_paths
            .iter()
            .find(|p| path.starts_with(p))
            .map_or_else(
                || {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                },
                |p| namespace_of(p, path),
            );
        let module = raw::parse_module(source, rel.clone(), ns);

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

        // Build the full package scope (with this file's fresh text seeded
        // into the cache) and keep only diagnostics belonging to *this*
        // file — each `.luab` file reports its own problems exactly once.
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.cache
            .lock()
            .expect("shape cache poisoned")
            .insert(key, Arc::new(module));
        // The seeded text supersedes whatever a cached scope was built from.
        self.scopes.lock().expect("scope cache poisoned").clear();
        let scope = self.package_scope(shape_paths, dependencies);
        diags.extend(
            scope
                .diags
                .iter()
                .filter(|d| d.primary_label().is_some_and(|l| l.span.file == rel))
                .cloned(),
        );
        diags.sort_by_key(|d| d.primary_label().map_or(0, |l| l.span.range.start));
        diags
    }
}
