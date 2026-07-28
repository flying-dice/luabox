//! Typed model for `luabox.toml` (SPEC.md §5, §6, §15).

use std::collections::BTreeMap;

/// Dialects accepted for `[package] edition`, `[build] target`, and
/// `[package] lua-versions` entries.
///
/// Distribution never parses syntax (SPEC.md §16): this is a local,
/// string-only allow-list, not a dependency on `luabox-syntax::Dialect`.
/// Crate-internal — the allowed set reaches callers inside the validation
/// error's message, never as a list they re-check.
pub(crate) const ALLOWED_DIALECTS: &[&str] = &["5.1", "5.2", "5.3", "5.4", "luajit"];

/// Bundler embedding modes for `[build] mode` (SPEC.md §7): `plain` (a bare
/// chunk, the default), `love` (LÖVE `.love` packaging), and `nvim-plugin`
/// (a Neovim `lua/<name>/init.lua` runtimepath layout).
pub const ALLOWED_BUNDLE_MODES: &[&str] = &["plain", "love", "nvim-plugin"];

/// `[package]` (SPEC.md §5, §6, §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// Dialect you write: one of `5.1`, `5.2`, `5.3`, `5.4`, `luajit`.
    pub edition: String,
    pub description: Option<String>,
    pub license: Option<String>,
    /// SPEC.md §6: dialects this package declares itself compatible with.
    pub lua_versions: Vec<String>,
    /// SPEC.md §15: minimum toolchain version the resolver must respect.
    pub min_luabox_version: Option<String>,
}

/// `[build]` (SPEC.md §5, §7) — the single `luabox build` command's
/// configuration (flying-dice/luabox#4). CLI flags override every field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Dialect you ship. Defaults to `[package] edition` when absent.
    pub target: String,
    /// Output directory: tree-mode emit and multi-entry bundles land here.
    /// Defaults to `"dist"`.
    pub out: String,
    /// Bundler embedding mode (SPEC.md §7). Defaults to `"plain"`. One of
    /// [`ALLOWED_BUNDLE_MODES`]; `luabox build --mode` overrides it.
    pub mode: String,
    /// Bundle entry points, one single-file bundle each. Defaults to
    /// `["src/main.lua"]`. Only consulted when bundling (`bundle = true`, or
    /// a non-`plain` `mode`).
    pub entry: Vec<String>,
    /// Single-entry bundle output path override (esbuild's `--outfile`).
    /// Valid only with exactly one entry, and never with a non-`plain`
    /// `mode`. `None` derives each bundle name from its entry basename under
    /// `out`.
    pub outfile: Option<String>,
    /// Emit one single-file bundle per entry instead of mirroring the source
    /// tree under `out`. Defaults to `false`. A non-`plain` `mode` implies
    /// bundling regardless.
    pub bundle: bool,
    /// Emit a `.map` alongside each bundle for `luabox unmap`. Defaults to
    /// `false`. Only meaningful when bundling.
    pub sourcemap: bool,
    /// Scope-aware identifier mangling + whitespace collapse on each bundle.
    /// Defaults to `false`. Only meaningful when bundling.
    pub minify: bool,
}

/// The conventional default bundle entry point when `[build] entry` is
/// absent (binary/script-project convention, SPEC.md §7).
pub const DEFAULT_ENTRY: &str = "src/main.lua";

/// `[types]` (SPEC.md §5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Types {
    pub strict: bool,
    /// Ambient definition packages (`*.d.lua` / `---@meta` modules).
    pub defs: Vec<String>,
}

/// One `[dependencies]` / `[dev-dependencies]` entry.
///
/// TOML shape: a bare version-requirement string, or an inline table with
/// exactly one of `git`, `path`, or `url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dependency {
    /// `pkg = "1.2.3"`
    Version(String),
    /// `pkg = { git = "…", rev|tag|branch = "…" }`
    Git(GitDependency),
    /// `pkg = { path = "…" }`
    Path(PathDependency),
    /// `pkg = { url = "…", sha256 = "…" }` — an http(s) (or `file://`/local)
    /// tarball, pinned by its SHA-256.
    Url(UrlDependency),
}

/// `version` alongside a source key (`git`/`path`/`url`) is **recorded, not
/// dropped** (#29): it is the declared version expectation for the sourced
/// package, reserved for when resolution returns (post-v1) — like `rev`/
/// `tag`/`branch`, nothing consumes it today. This is deliberate: the same
/// key is authoritative in the version-only form (`pkg = { version = … }`
/// ≡ `pkg = "…"`, #23) and metadata alongside a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDependency {
    pub git: String,
    pub rev: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathDependency {
    pub path: String,
    pub version: Option<String>,
}

/// `pkg = { url = "…", sha256 = "…" }` — an http(s) tarball dependency
/// (SPEC.md §6). `sha256` is REQUIRED: integrity is non-negotiable, so a `url`
/// source with no digest is a parse error, never a silent unverified fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlDependency {
    pub url: String,
    pub sha256: String,
    pub version: Option<String>,
}

/// A lint severity level in `[lint]` (SPEC.md §9): the analog of clippy's
/// `allow` / `warn` / `deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    /// Rule is off — no diagnostics.
    Allow,
    /// Rule fires at warning severity (does not fail the command).
    Warn,
    /// Rule fires at error severity (fails the command).
    Deny,
}

/// The lint tier names a `[lint]` toggle may target (SPEC.md §9). Held as a
/// local list rather than a dependency on `luabox-lint` — Distribution never
/// depends on the Semantics/Frontend crates (SPEC.md §16, acyclic graph).
/// Crate-internal: it exists to classify a `[lint]` key as tier-or-rule while
/// parsing, and consumers read that classification off [`Lint`] instead.
pub(crate) const LINT_TIERS: &[&str] = &["correctness", "suspicious", "perf", "style", "pedantic"];

/// `[lint]` (SPEC.md §9): per-rule and per-tier level overrides plus a global
/// allow-list for the `global-write` rule.
///
/// Rule ids live in `luabox-lint` (which this crate must not depend on), so
/// keys that are neither `globals` nor a known tier name are recorded as
/// rule-id overrides without validating the id here; `luabox-lint` resolves
/// (and can diagnose) unknown ids when it consumes the config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lint {
    /// Extra names the `global-write` rule treats as intentional globals.
    pub globals: Vec<String>,
    /// Tier-level overrides, keyed by tier name (`correctness`, `suspicious`,
    /// `perf`, `style`, `pedantic`).
    pub tiers: BTreeMap<String, LintLevel>,
    /// Rule-id overrides (`unused-local = "allow"`).
    pub rules: BTreeMap<String, LintLevel>,
}

/// The typed, validated contents of a `luabox.toml`.
///
/// Construct via [`Manifest::parse`]. The source text is retained alongside
/// the typed view, so [`Display`](std::fmt::Display) renders the manifest
/// back byte-identically — comments and formatting intact (the
/// comment-preserving round-trip SPEC.md §16 makes this context's contract).
#[derive(Debug, Clone)]
pub struct Manifest {
    pub package: Package,
    pub build: Build,
    pub types: Types,
    pub dependencies: BTreeMap<String, Dependency>,
    pub dev_dependencies: BTreeMap<String, Dependency>,
    /// `[lint]` configuration (SPEC.md §9).
    pub lint: Lint,
    /// The exact text this manifest was parsed from. Private, and all that
    /// remains of the `toml_edit` document the manifest used to carry:
    /// nothing edits a manifest in place, so the only part of "lossless" any
    /// caller ever used was rendering it back unchanged.
    pub(crate) source: String,
}

impl std::fmt::Display for Manifest {
    /// Renders the manifest as the TOML text it was parsed from — comments
    /// and formatting intact.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.source)
    }
}
