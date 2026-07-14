//! Bundler: require-graph resolution, tree-shaking, minify, sourcemaps —
//! **Emit** bounded context (SPEC.md §7, §16).
//!
//! [`bundle`] turns one entry file plus its static `require` graph into a
//! single target-lowered `.lua` file. Pipeline, per reachable module:
//!
//! 1. **Lower** `edition → target` via [`luabox_lower::lower_bare`] — the
//!    per-module `__luabox_rt` prelude is *not* emitted; helper sets are
//!    unioned across the bundle and one shared prelude is hoisted to
//!    bundle top ([`luabox_lower::rt_prelude`]).
//! 2. **Extract the require graph** from the lowered text (`luabox-hir`):
//!    static string-literal `require`s become edges; non-literal calls are
//!    collected and, if any survive on a reachable module, fail the bundle
//!    ([`BundleError::DynamicRequires`] — an allowlist override is a
//!    follow-up, SPEC.md §7).
//! 3. **Resolve** each edge to a file ([`resolve`] module docs hold the
//!    search-path algorithm); resolved call sites are rewritten to
//!    `__luabox_require("name")`, unresolved ones are left as runtime
//!    `require` (external modules, e.g. C libraries).
//! 4. **Tree-shake** at module level: only files reachable from the entry
//!    are bundled — unreachable project files simply never enter the walk.
//! 5. **Minify** (opt-in): scope-aware identifier mangling + whitespace
//!    collapse ([`minify`] module docs); property names never mangled.
//! 6. **Emit** the module map + require shim + inlined entry chunk, and
//!    (opt-in) a line-based `.lua.map` ([`sourcemap`] module docs). The
//!    whole bundle is reparsed under the target as a mechanical guarantee.
//!
//! # `require` semantics fidelity
//!
//! The emitted `__luabox_require` shim reproduces Lua 5.x `require` over
//! the *real* `package.loaded` table (when the runtime has one; a private
//! table otherwise):
//!
//! - a truthy `package.loaded[name]` short-circuits (a `false` entry
//!   reloads, exactly like real `require`);
//! - the module chunk runs with the module name as its `...`;
//! - a non-`nil` chunk return is stored in `package.loaded[name]`;
//!   otherwise, if the chunk did not itself write `package.loaded[name]`,
//!   `true` is stored — Lua 5.1–5.4 loader protocol;
//! - the cache is written **after** the chunk runs; a re-entrant require
//!   during load returns whatever is in `package.loaded` at that moment.
//!   Cycles therefore behave like real Lua: a module that publishes its
//!   (partial) table early — `package.loaded[...] = M` before requiring
//!   back — hands that partial table to its requirer; a cycle between
//!   modules that never publish early recurses, exactly as stock
//!   `require` does on 5.2+ (5.1's dedicated "loop … loading module"
//!   sentinel error is not reproduced).
//!
//! Modules initialize **lazily on first require** — relative to the
//! multi-file layout this preserves load order, because real `require`
//! also runs a module's body at its first require site.
//!
//! An entry that is itself `require`d by a bundled module is rejected
//! ([`BundleError::EntryRequired`]) — supporting that shape is a
//! follow-up.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use luabox_lower::{Helper, LowerDiagnostic};
use luabox_syntax::{Dialect, lua};
use rowan::TextRange;

mod minify;
mod resolve;
mod sourcemap;

pub use resolve::{resolve as resolve_module, resolve_candidates};
pub use sourcemap::{BundleMap, unmap_traceback};

/// Everything [`bundle`] needs to know.
#[derive(Debug, Clone)]
pub struct BundleRequest<'a> {
    /// Project root; module resolution and display paths are rooted here.
    pub root: &'a Path,
    /// The entry file (absolute or root-relative).
    pub entry: &'a Path,
    /// Dialect the sources are written in (`[package] edition`).
    pub edition: Dialect,
    /// Dialect the bundle ships as (`[build] target`).
    pub target: Dialect,
    /// Output file name (e.g. `app.lua`) — used in the banner and the map.
    pub name: &'a str,
    /// Minify module texts (SPEC.md §7; see [`minify`] module docs).
    pub minify: bool,
    /// Also produce the `.lua.map` JSON payload.
    pub sourcemap: bool,
}

/// A successful bundle.
#[derive(Debug)]
pub struct Bundle {
    /// The single-file bundle text.
    pub text: String,
    /// The `.lua.map` JSON payload, when requested.
    pub map: Option<String>,
    /// Number of modules inlined (the entry chunk not counted).
    pub modules: usize,
    /// Warn-tier lowering diagnostics, paired with the root-relative file
    /// they came from (same tier `luabox build` renders and proceeds on).
    pub warnings: Vec<(String, LowerDiagnostic)>,
}

/// A dynamic (non-literal) `require` call site on a reachable module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicRequireSite {
    /// Root-relative file, forward slashes.
    pub file: String,
    /// 1-based line of the call site.
    pub line: u32,
}

/// Why a bundle could not be produced.
#[derive(Debug)]
pub enum BundleError {
    /// A module file could not be read.
    Io { path: PathBuf, message: String },
    /// A module failed to parse, or its lowered output failed residual
    /// validation under the target.
    Parse { file: String, message: String },
    /// Lowering `edition → target` failed with hard diagnostics.
    Lower {
        file: String,
        diagnostics: Vec<LowerDiagnostic>,
    },
    /// Reachable modules contain `require(<non-literal>)` calls the
    /// bundler cannot resolve statically.
    DynamicRequires(Vec<DynamicRequireSite>),
    /// A bundled module requires the entry module itself.
    EntryRequired { file: String, module: String },
    /// A `.lua.map` sourcemap payload is not valid JSON.
    SourceMap(String),
    /// A `.lua.map` sourcemap declares a version this luabox cannot read.
    SourceMapVersion(u32),
    /// An internal invariant broke (minify or bundle output failed the
    /// mechanical reparse check) — a bundler bug, not a user error.
    Internal(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Io { path, message } => {
                write!(f, "cannot read `{}`: {message}", path.display())
            }
            BundleError::Parse { file, message } => write!(f, "`{file}`: {message}"),
            BundleError::Lower { file, diagnostics } => {
                write!(f, "cannot lower `{file}` for bundling:")?;
                for d in diagnostics {
                    write!(f, "\n  {}: {}", d.code, d.message)?;
                }
                Ok(())
            }
            BundleError::DynamicRequires(sites) => {
                write!(
                    f,
                    "cannot bundle dynamic require: the argument must be a string literal \
                     so the module graph is statically known"
                )?;
                for site in sites {
                    write!(f, "\n  {}:{}: dynamic `require(...)`", site.file, site.line)?;
                }
                write!(
                    f,
                    "\nrewrite each call as `require \"exact.module.name\"`; an allowlist \
                     override (`[bundle] allow-dynamic`) is planned (SPEC.md §7)"
                )
            }
            BundleError::EntryRequired { file, module } => write!(
                f,
                "`{file}` requires \"{module}\", which is the entry module; bundling an \
                 entry that is itself required is not supported yet"
            ),
            BundleError::SourceMap(message) => write!(f, "invalid .lua.map: {message}"),
            BundleError::SourceMapVersion(version) => write!(
                f,
                "unsupported .lua.map version {version} (this luabox reads version 1)"
            ),
            BundleError::Internal(message) => write!(f, "internal bundler error: {message}"),
        }
    }
}

impl std::error::Error for BundleError {}

/// One reachable module (or the entry chunk) after lowering.
struct Module {
    /// Map key (`None` for the entry chunk).
    name: Option<String>,
    /// Root-relative display path, forward slashes.
    file: String,
    /// Lowered (bare) text; require rewrites and minify are applied to it.
    text: String,
    /// `__luabox_rt` helper names this module's lowered text uses.
    helpers: Vec<&'static str>,
    /// Pending `require` call rewrites: range (in `text`) → module name.
    rewrites: Vec<(TextRange, String)>,
}

/// Bundle the entry's require graph into a single file. See the crate
/// docs for the pipeline and the emitted `require` semantics.
#[allow(
    clippy::missing_panics_doc,
    reason = "the only expect is an internal invariant: non-entry modules always carry a map key"
)]
pub fn bundle(req: &BundleRequest<'_>) -> Result<Bundle, BundleError> {
    let entry_path = canonical(&req.root.join(req.entry));
    let mut warnings = Vec::new();

    // Discovery: BFS over static require edges, entry first. Module
    // identity is the canonical file path; the map key is the first
    // require string that reached the file (tree-shaking is inherent —
    // unreachable files never enter the walk).
    let mut modules: Vec<Module> = Vec::new();
    let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
    let mut queue: Vec<usize> = Vec::new();
    let mut dynamic: Vec<DynamicRequireSite> = Vec::new();

    let (entry_module, entry_edges) =
        load_module(&entry_path, None, req, &mut warnings, &mut dynamic)?;
    modules.push(entry_module);
    by_path.insert(entry_path.clone(), 0);
    let mut pending = vec![(0usize, entry_edges)];

    while let Some((index, edges)) = pending.pop() {
        queue.push(index);
        for (range, name) in edges {
            let Some(path) = resolve::resolve(req.root, &name) else {
                continue; // external: left as a runtime `require`
            };
            if path == entry_path {
                return Err(BundleError::EntryRequired {
                    file: modules[index].file.clone(),
                    module: name,
                });
            }
            let target = if let Some(&existing) = by_path.get(&path) {
                existing
            } else {
                let (module, edges) =
                    load_module(&path, Some(name.clone()), req, &mut warnings, &mut dynamic)?;
                modules.push(module);
                let new_index = modules.len() - 1;
                by_path.insert(path, new_index);
                pending.push((new_index, edges));
                new_index
            };
            #[expect(
                clippy::expect_used,
                reason = "only the entry module (index 0) has no name; dependency targets are always loaded with Some(name)"
            )]
            let canonical_name = modules[target]
                .name
                .clone()
                .expect("non-entry modules always carry a map key");
            modules[index].rewrites.push((range, canonical_name));
        }
    }

    if !dynamic.is_empty() {
        dynamic.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
        return Err(BundleError::DynamicRequires(dynamic));
    }

    // Rewrite resolved `require` calls to the shim, back-to-front.
    for module in &mut modules {
        module.rewrites.sort_by_key(|(range, _)| range.start());
        for (range, name) in module.rewrites.drain(..).rev() {
            let call = format!("__luabox_require({})", quote(&name));
            module
                .text
                .replace_range(usize::from(range.start())..usize::from(range.end()), &call);
        }
    }

    if req.minify {
        for module in &mut modules {
            module.text = minify::minify(&module.text, req.target)
                .map_err(|m| BundleError::Internal(format!("{}: {m}", module.file)))?;
        }
    }

    let bundle = emit(req, &modules);
    let reparse = lua::parse(&bundle.0, req.target);
    if let Some(err) = reparse.errors().first() {
        return Err(BundleError::Internal(format!(
            "bundle output no longer parses under {}: {}",
            req.target.manifest_id(),
            err.message
        )));
    }

    Ok(Bundle {
        text: bundle.0,
        map: req.sourcemap.then(|| bundle.1.to_json()),
        modules: modules.len() - 1,
        warnings,
    })
}

/// Read, lower (bare), residually validate, and extract the require graph
/// of one file. Returns the module plus its static edges in source order.
#[allow(clippy::type_complexity, reason = "edge list is local plumbing")]
fn load_module(
    path: &Path,
    name: Option<String>,
    req: &BundleRequest<'_>,
    warnings: &mut Vec<(String, LowerDiagnostic)>,
    dynamic: &mut Vec<DynamicRequireSite>,
) -> Result<(Module, Vec<(TextRange, String)>), BundleError> {
    let file = display_rel(path, req.root);
    let source = std::fs::read_to_string(path).map_err(|e| BundleError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;

    let lowered =
        luabox_lower::lower_bare(&source, req.edition, req.target).map_err(|diagnostics| {
            BundleError::Lower {
                file: file.clone(),
                diagnostics,
            }
        })?;
    warnings.extend(lowered.warnings.iter().cloned().map(|d| (file.clone(), d)));

    let parse = lua::parse(&lowered.text, req.target);
    if let Some(err) = parse.errors().first() {
        return Err(BundleError::Parse {
            file,
            message: err.message.clone(),
        });
    }
    // Residual validation, as in `luabox build`: constructs with no
    // lowering rule (e.g. hex floats targeting 5.1) must not ship.
    if let Some(finding) = lua::validate::validate(&parse, req.target)
        .into_iter()
        .next()
    {
        return Err(BundleError::Parse {
            file,
            message: format!(
                "not legal under target {}: {} (no lowering rule)",
                req.target.manifest_id(),
                finding.message
            ),
        });
    }

    let hir = luabox_hir::lower(&parse);
    for site in hir.dynamic_requires() {
        dynamic.push(DynamicRequireSite {
            file: file.clone(),
            line: line_of(&lowered.text, site.range.start()),
        });
    }
    let edges = hir
        .requires()
        .iter()
        .map(|edge| (edge.range, edge.module.clone()))
        .collect();

    Ok((
        Module {
            name,
            file,
            text: lowered.text,
            helpers: lowered.polyfills,
            rewrites: Vec::new(),
        },
        edges,
    ))
}

/// Assemble the bundle text plus its map. `modules[0]` is the entry chunk
/// (inlined last, so it runs after every definition is registered);
/// non-entry modules are registered in discovery order and initialize
/// lazily on first `__luabox_require`.
fn emit(req: &BundleRequest<'_>, modules: &[Module]) -> (String, BundleMap) {
    let mut out = Emitter::new(req.name);

    out.raw(&format!(
        "-- bundled by luabox ({} -> {})\n",
        req.edition.manifest_id(),
        req.target.manifest_id()
    ));

    // One hoisted rt prelude for the union of every module's helpers.
    let helpers: std::collections::BTreeSet<Helper> = modules
        .iter()
        .flat_map(|m| m.helpers.iter())
        .filter_map(|name| Helper::from_name(name))
        .collect();
    if let Some(prelude) = luabox_lower::rt_prelude(&helpers, req.edition, req.target) {
        out.raw(&prelude);
    }

    if modules.len() > 1 {
        out.raw(SHIM);
        for module in &modules[1..] {
            #[expect(
                clippy::expect_used,
                reason = "iterating modules[1..] skips the entry module; every remaining module carries Some(name)"
            )]
            let name = module.name.as_deref().expect("non-entry module has a key");
            out.raw(&format!(
                "__luabox_modules[{}] = function(...)\n",
                quote(name)
            ));
            out.mapped(&module.text, &module.file);
            out.raw("end\n");
        }
    }

    let entry = &modules[0];
    out.mapped(&entry.text, &entry.file);
    out.finish()
}

/// The module map + require shim. See the crate docs for the semantics
/// argument (Lua 5.x loader protocol over the real `package.loaded`).
const SHIM: &str = r#"local __luabox_modules = {}
local __luabox_loaded = type(package) == "table" and type(package.loaded) == "table" and package.loaded or {}
local function __luabox_require(name)
  local hit = __luabox_loaded[name]
  if hit then
    return hit
  end
  local chunk = __luabox_modules[name]
  if chunk == nil then
    error("module '" .. name .. "' is not in the bundle", 2)
  end
  local ret = chunk(name)
  if ret ~= nil then
    __luabox_loaded[name] = ret
  elseif __luabox_loaded[name] == nil then
    __luabox_loaded[name] = true
  end
  return __luabox_loaded[name]
end
"#;

/// Bundle text + line map accumulator.
struct Emitter {
    text: String,
    map: BundleMap,
    file_indices: HashMap<String, usize>,
}

impl Emitter {
    fn new(bundle_name: &str) -> Self {
        Self {
            text: String::new(),
            map: BundleMap {
                version: 1,
                bundle: bundle_name.to_owned(),
                files: Vec::new(),
                lines: Vec::new(),
            },
            file_indices: HashMap::new(),
        }
    }

    /// Append bundler-generated text (lines map to nothing).
    fn raw(&mut self, text: &str) {
        self.push(text, None);
    }

    /// Append module text; each of its lines maps to `file` at the same
    /// 1-based line number within `text`.
    fn mapped(&mut self, text: &str, file: &str) {
        let index = if let Some(&i) = self.file_indices.get(file) {
            i
        } else {
            self.map.files.push(file.to_owned());
            let i = self.map.files.len() - 1;
            self.file_indices.insert(file.to_owned(), i);
            i
        };
        self.push(text, Some(index));
    }

    fn push(&mut self, text: &str, file: Option<usize>) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        if !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        let lines = text.split_terminator('\n').count().max(1);
        for line in 1..=lines {
            self.map
                .lines
                .push(file.map(|f| (f, u32::try_from(line).unwrap_or(u32::MAX))));
        }
    }

    fn finish(self) -> (String, BundleMap) {
        (self.text, self.map)
    }
}

/// Canonicalize for identity comparisons; fall back to the raw path when
/// the file vanished (the read that follows will report it properly).
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Root-relative path with forward slashes — stable display across
/// platforms (canonical paths on Windows carry a `\\?\` prefix, so the
/// root is canonicalized for the strip too).
fn display_rel(path: &Path, root: &Path) -> String {
    let stripped = path
        .strip_prefix(root)
        .or_else(|_| path.strip_prefix(canonical(root)))
        .unwrap_or(path);
    stripped.to_string_lossy().replace('\\', "/")
}

/// Quote a module name as a Lua string literal.
fn quote(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push('\\');
                out.push_str(&(c as u32).to_string());
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 1-based line of a byte offset.
fn line_of(text: &str, offset: rowan::TextSize) -> u32 {
    // Count newlines over the byte prefix directly: works for any clamped
    // offset (no char-boundary requirement) and avoids slicing `text` as a
    // `str`.
    let end = usize::from(offset).min(text.len());
    let newlines = text.bytes().take(end).filter(|&b| b == b'\n').count();
    u32::try_from(newlines + 1).unwrap_or(u32::MAX)
}
