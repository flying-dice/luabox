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

pub use resolve::{resolve as resolve_module, resolve_candidates, rocks_version_dir};
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
                    // `LowerDiagnostic::code` is the bare number; render the
                    // `LBnnnn` spelling the registry and `luabox explain` use.
                    write!(f, "\n  LB{:04}: {}", d.code, d.message)?;
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
    /// Lowered (bare) text, **with its file prefix already cut** — see
    /// [`split_file_prefix`]. Require rewrites and minify are applied to it.
    text: String,
    /// This module's own `#!` line, verbatim and without its newline, when
    /// the file had one. Only the *entry*'s is emitted (see [`emit`]).
    shebang: Option<String>,
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
            // The *target* dialect selects the luarocks version directory: a
            // rock tree is materialized for the interpreter the bundle ships
            // against, not for the dialect the sources are written in.
            let Some(path) = resolve::resolve(req.root, &name, req.target) else {
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
    // The file prefix (a UTF-8 BOM and/or a `#!` line) is only *legal* at
    // byte 0 of a chunk, and a module's text is spliced into the middle of
    // the bundle — where `#` is the length operator and a BOM is an illegal
    // character. It is therefore cut here, before any splicing, and the
    // entry's `#!` line is re-emitted at the top of the bundle by `emit`.
    // The cut is the lexer's own (`split_file_prefix`), and it deliberately
    // leaves the newline that *ended* the shebang in place, so every line of
    // the module keeps its original number in the `.lua.map` and in the
    // `require`-rewrite ranges shifted below.
    let (shebang, prefix) = split_file_prefix(&lowered.text, req.target);
    let mut text = lowered.text;
    text.replace_range(..usize::from(prefix), "");

    let edges = hir
        .requires()
        .iter()
        .map(|edge| (shift_back(edge.range, prefix), edge.module.clone()))
        .collect();

    Ok((
        Module {
            name,
            file,
            text,
            shebang,
            helpers: lowered.polyfills,
            rewrites: Vec::new(),
        },
        edges,
    ))
}

/// Reference Lua's *file prefix*: an optional UTF-8 byte-order mark followed
/// by an optional `#`-led first line (`skipBOM` then `skipcomment` in
/// `lauxlib.c`). Returns the `#!` line's text — verbatim, without the
/// newline that ends it — and the byte length of the whole prefix.
///
/// The split is the lexer's, not a guess: [`lua::lex`] recognizes the prefix
/// at byte 0 and nowhere else, emits it as [`lua::SyntaxKind::BOM`] /
/// [`lua::SyntaxKind::SHEBANG`] trivia, and `is_file_prefix` names exactly
/// those two kinds. Every dialect lexes the prefix the same way (whether a
/// BOM is *legal* is the parser's verdict, and it has already been taken by
/// the time this runs), so `dialect` only picks the lexer, never the rule.
fn split_file_prefix(text: &str, dialect: Dialect) -> (Option<String>, rowan::TextSize) {
    let mut end = rowan::TextSize::new(0);
    let mut shebang = None;
    for token in lua::lex(text, dialect) {
        if !token.kind.is_file_prefix() {
            break;
        }
        let start = end;
        end += rowan::TextSize::new(token.len);
        if token.kind == lua::SyntaxKind::SHEBANG {
            shebang = text
                .get(usize::from(start)..usize::from(end))
                .map(str::to_owned);
        }
    }
    (shebang, end)
}

/// Move a range back by the file prefix that was just cut off the front of
/// the text it points into. Every real token starts after the prefix, so the
/// subtraction never underflows; a `0` floor keeps that an invariant rather
/// than a panic.
fn shift_back(range: TextRange, by: rowan::TextSize) -> TextRange {
    let sub = |offset: rowan::TextSize| offset.checked_sub(by).unwrap_or_default();
    TextRange::new(sub(range.start()), sub(range.end()))
}

/// Assemble the bundle text plus its map. `modules[0]` is the entry chunk
/// (inlined last, so it runs after every definition is registered);
/// non-entry modules are registered in discovery order and initialize
/// lazily on first `__luabox_require`.
fn emit(req: &BundleRequest<'_>, modules: &[Module]) -> (String, BundleMap) {
    let mut out = Emitter::new(req.name);

    // The entry's `#!` line leads the bundle, ahead of even the banner: a
    // bundled program is still a program, and the kernel only honours a
    // shebang at byte 0. Dependencies' shebangs are dropped — they only ever
    // meant "run *this* file", and the dependency is no longer a file.
    //
    // A BOM is never emitted, even when the entry had one. The bundle is a
    // *new* file rather than a rewrite of the entry, and `lua5.1` has no
    // `skipBOM` (Dialect::skips_bom): a 5.1-targeted bundle carrying a mark
    // would simply fail to load. Dropping it costs nothing — the mark is not
    // load-bearing for UTF-8 — and keeps one rule for every target.
    if let Some(shebang) = modules.first().and_then(|m| m.shebang.as_deref()) {
        out.raw(shebang);
    }

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

#[cfg(test)]
mod tests {
    // test code — panics document assumptions
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn quote_escapes_everything_lua_would_misread() {
        assert_eq!(quote("plain.name"), "\"plain.name\"");
        assert_eq!(quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote("back\\slash"), "\"back\\\\slash\"");
        assert_eq!(quote("a\nb"), "\"a\\nb\"");
        assert_eq!(quote("a\rb"), "\"a\\rb\"");
        // Other control characters take Lua's decimal `\ddd` escape.
        assert_eq!(quote("a\tb"), "\"a\\9b\"");
        assert_eq!(quote("\u{0}"), "\"\\0\"");
        // Non-ASCII is legal inside a Lua string literal and stays verbatim.
        assert_eq!(quote("café"), "\"café\"");
        assert_eq!(quote(""), "\"\"");
    }

    #[test]
    fn line_of_is_one_based_and_clamps_out_of_range_offsets() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_of(text, 0.into()), 1);
        assert_eq!(line_of(text, 3.into()), 1, "offset of the newline itself");
        assert_eq!(line_of(text, 4.into()), 2);
        assert_eq!(line_of(text, 8.into()), 3);
        // Past the end clamps to the last line rather than panicking.
        assert_eq!(line_of(text, 9_999.into()), 3);
        assert_eq!(line_of("", 0.into()), 1);
    }

    #[test]
    fn display_rel_strips_the_root_and_normalizes_separators() {
        let root = Path::new("/project");
        assert_eq!(
            display_rel(Path::new("/project/src/a.lua"), root),
            "src/a.lua"
        );
        // A path outside the root cannot be made relative: kept whole.
        assert_eq!(
            display_rel(Path::new("/elsewhere/a.lua"), root),
            "/elsewhere/a.lua"
        );
    }

    #[test]
    fn canonical_falls_back_to_the_raw_path_when_the_file_is_gone() {
        let missing = Path::new("/definitely/not/here/a.lua");
        assert_eq!(canonical(missing), missing.to_path_buf());
    }

    // --- file prefix ------------------------------------------------------

    /// `split_file_prefix` returns the prefix length as a plain byte count.
    fn prefix_len(text: &str) -> usize {
        usize::from(split_file_prefix(text, Dialect::Lua54).1)
    }

    #[test]
    fn split_file_prefix_finds_bom_and_shebang_only_at_byte_zero() {
        // Nothing to cut.
        assert_eq!(
            split_file_prefix("return 1\n", Dialect::Lua54),
            (None, 0.into())
        );
        // A shebang: reported verbatim, newline excluded.
        let (shebang, len) = split_file_prefix("#!/usr/bin/env lua\nreturn 1\n", Dialect::Lua54);
        assert_eq!(shebang.as_deref(), Some("#!/usr/bin/env lua"));
        assert_eq!(usize::from(len), "#!/usr/bin/env lua".len());
        // Any `#`-led first line, not just `#!` — that is `skipcomment`.
        assert_eq!(prefix_len("# plain hash line\nreturn 1\n"), 17);
        // BOM alone, and BOM then shebang (the order `lauxlib.c` accepts).
        assert_eq!(
            split_file_prefix("\u{feff}return 1\n", Dialect::Lua54).0,
            None
        );
        assert_eq!(prefix_len("\u{feff}return 1\n"), 3);
        let (shebang, len) = split_file_prefix("\u{feff}#!/bin/lua\nreturn 1\n", Dialect::Lua54);
        assert_eq!(
            shebang.as_deref(),
            Some("#!/bin/lua"),
            "the BOM is not part of it"
        );
        assert_eq!(usize::from(len), 3 + "#!/bin/lua".len());
        // A `#` on any later line is the length operator, never a prefix.
        assert_eq!(prefix_len("\nreturn #t\n"), 0);
        // A CRLF shebang keeps its `\r`, exactly as the lexer reports it.
        assert_eq!(
            split_file_prefix("#!x\r\nreturn 1\n", Dialect::Lua54)
                .0
                .as_deref(),
            Some("#!x\r")
        );
        // Degenerate inputs must not panic.
        assert_eq!(prefix_len(""), 0);
        assert_eq!(prefix_len("#!/usr/bin/env lua"), 18);
        assert_eq!(prefix_len("\u{feff}"), 3);
    }

    #[test]
    fn split_file_prefix_is_dialect_independent() {
        // Whether a BOM is *legal* is the parser's verdict; the lexer cuts
        // the same prefix under every dialect, so the bundler's cut does
        // not change with `[build] target`.
        for dialect in [
            Dialect::Lua51,
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::Lua54,
            Dialect::LuaJit,
        ] {
            let (shebang, len) =
                split_file_prefix("\u{feff}#!/usr/bin/env lua\nreturn 1\n", dialect);
            assert_eq!(
                shebang.as_deref(),
                Some("#!/usr/bin/env lua"),
                "{dialect:?}"
            );
            assert_eq!(usize::from(len), 3 + 18, "{dialect:?}");
        }
    }

    #[test]
    fn cutting_the_prefix_leaves_its_newline_so_line_numbers_do_not_move() {
        // The invariant the `.lua.map` and every `require` rewrite rely on:
        // what is cut is the prefix *only*, never the newline after it, so
        // the text that remains still has the module's original line
        // numbering (line 1 is simply empty).
        let text = "#!/usr/bin/env lua\nlocal x = 1\nreturn x\n";
        let (_, len) = split_file_prefix(text, Dialect::Lua54);
        let mut cut = text.to_owned();
        cut.replace_range(..usize::from(len), "");
        assert_eq!(cut, "\nlocal x = 1\nreturn x\n");
        assert_eq!(line_of(&cut, 1.into()), line_of(text, 19.into()));
    }

    #[test]
    fn shift_back_never_underflows() {
        let by = rowan::TextSize::new(18);
        assert_eq!(
            shift_back(TextRange::new(20.into(), 30.into()), by),
            TextRange::new(2.into(), 12.into())
        );
        // Cannot happen (every real token starts after the prefix) but the
        // floor is what keeps that a `0`, not a panic.
        assert_eq!(
            shift_back(TextRange::new(0.into(), 4.into()), by),
            TextRange::new(0.into(), 0.into())
        );
    }

    /// Bundle a set of `(relative path, contents)` files rooted at a temp
    /// directory, with `src/main.lua` as the entry.
    fn bundle_files(files: &[(&str, &str)], target: Dialect, minify: bool) -> Bundle {
        let dir = tempfile::tempdir().unwrap();
        for (name, contents) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        }
        bundle(&BundleRequest {
            root: dir.path(),
            entry: Path::new("src/main.lua"),
            edition: Dialect::Lua54,
            target,
            name: "main.lua",
            minify,
            sourcemap: true,
        })
        .expect("bundle")
    }

    #[test]
    fn a_dependency_shebang_is_cut_instead_of_breaking_the_bundle() {
        // Regression: the dependency's `#!` line used to be spliced into the
        // middle of the bundle, where `#` is the length operator — the
        // bundle then failed its own reparse with "internal bundler error".
        for minify in [false, true] {
            let out = bundle_files(
                &[
                    ("src/main.lua", "local u = require(\"util\")\nreturn u.n\n"),
                    ("src/util.lua", "#!/usr/bin/env lua\nreturn { n = 41 }\n"),
                ],
                Dialect::Lua54,
                minify,
            );
            assert!(
                !out.text.contains("#!/usr/bin/env lua"),
                "minify={minify}: dependency shebang leaked into the bundle:\n{}",
                out.text
            );
            assert!(out.text.contains("41"), "minify={minify}");
            assert!(
                !out.text.starts_with("#!"),
                "minify={minify}: the entry had no shebang, so the bundle must not grow one"
            );
        }
    }

    #[test]
    fn the_entry_shebang_leads_the_bundle_plain_and_minified() {
        for minify in [false, true] {
            for target in [Dialect::Lua51, Dialect::Lua54] {
                let out = bundle_files(
                    &[
                        (
                            "src/main.lua",
                            "#!/usr/bin/env lua\nlocal u = require(\"util\")\nreturn u.n\n",
                        ),
                        ("src/util.lua", "#!/usr/bin/env lua\nreturn { n = 41 }\n"),
                    ],
                    target,
                    minify,
                );
                assert!(
                    out.text.starts_with("#!/usr/bin/env lua\n"),
                    "target={target:?} minify={minify}: bundle does not start with the entry \
                     shebang:\n{}",
                    out.text
                );
                // Exactly one — the dependency's was cut, not moved.
                assert_eq!(out.text.matches("#!/usr/bin/env lua").count(), 1);
            }
        }
    }

    #[test]
    fn a_bom_is_cut_from_every_module_and_never_re_emitted() {
        // The bundle is a *new* file, so it never inherits a mark: emitting
        // one would break a 5.1 target outright (no `skipBOM`) and buys
        // nothing anywhere else.
        for minify in [false, true] {
            let out = bundle_files(
                &[
                    (
                        "src/main.lua",
                        "\u{feff}local u = require(\"util\")\nreturn u.n\n",
                    ),
                    ("src/util.lua", "\u{feff}return { n = 41 }\n"),
                ],
                Dialect::Lua54,
                minify,
            );
            assert!(
                !out.text.contains('\u{feff}'),
                "minify={minify}: bundle carries a BOM:\n{}",
                out.text
            );
            assert!(out.text.contains("41"), "minify={minify}");
        }
    }

    #[test]
    fn a_bom_under_a_5_1_target_is_still_rejected_by_name() {
        // Unchanged, and deliberately so: `[build] target = "5.1"` means the
        // *sources* must be legal 5.1 too (tree mode ships them verbatim),
        // and 5.1 has no `skipBOM`. Cutting the prefix for the bundle does
        // not soften that verdict.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.lua"), "\u{feff}return 1\n").unwrap();
        let err = bundle(&BundleRequest {
            root: dir.path(),
            entry: Path::new("src/main.lua"),
            edition: Dialect::Lua51,
            target: Dialect::Lua51,
            name: "main.lua",
            minify: false,
            sourcemap: false,
        })
        .expect_err("a BOM is not legal 5.1");
        assert!(
            err.to_string().contains("byte-order mark"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn require_rewrites_still_land_after_a_prefix_is_cut() {
        // The edge ranges are recorded against the *un*cut text; if the
        // shift were forgotten the shim call would be spliced 18 bytes off.
        let out = bundle_files(
            &[
                (
                    "src/main.lua",
                    "#!/usr/bin/env lua\nlocal u = require(\"util\")\nreturn u.n\n",
                ),
                ("src/util.lua", "return { n = 41 }\n"),
            ],
            Dialect::Lua54,
            false,
        );
        assert!(
            out.text.contains("local u = __luabox_require(\"util\")"),
            "{}",
            out.text
        );
    }
}
