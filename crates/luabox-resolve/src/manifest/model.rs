//! Typed model for `luabox.toml` (SPEC.md §5, §6, §15).

use std::collections::BTreeMap;

use serde::Serialize;

/// Dialects accepted for `[package] edition`, `[build] target`, and
/// `[package] lua-versions` entries.
///
/// Distribution never parses syntax (SPEC.md §16): this is a local,
/// string-only allow-list, not a dependency on `luabox-syntax::Dialect`.
pub const ALLOWED_DIALECTS: &[&str] = &["5.1", "5.2", "5.3", "5.4", "luajit"];

/// Bundler embedding modes for `[build] mode` (SPEC.md §7): `plain` (a bare
/// chunk, the default), `love` (LÖVE `.love` packaging), and `nvim-plugin`
/// (a Neovim `lua/<name>/init.lua` runtimepath layout).
pub const ALLOWED_BUNDLE_MODES: &[&str] = &["plain", "love", "nvim-plugin"];

/// `[package]` (SPEC.md §5, §6, §15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Package {
    pub name: String,
    pub version: String,
    /// Dialect you write. One of [`ALLOWED_DIALECTS`].
    pub edition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// SPEC.md §6: dialects this package declares itself compatible with.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lua_versions: Vec<String>,
    /// SPEC.md §15: minimum toolchain version the resolver must respect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_luabox_version: Option<String>,
}

/// `[build]` (SPEC.md §5, §7) — the single `luabox build` command's
/// configuration (flying-dice/luabox#4). CLI flags override every field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Types {
    pub strict: bool,
    /// Ambient definition packages (`*.d.lua` / `---@meta` modules).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub defs: Vec<String>,
}

/// One `[dependencies]` / `[dev-dependencies]` entry.
///
/// TOML shape: a bare version-requirement string, or an inline table with
/// exactly one of `git`, `path`, or `workspace = true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
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
    /// `pkg = { workspace = true }`
    Workspace(WorkspaceDependency),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct GitDependency {
    pub git: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PathDependency {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `pkg = { url = "…", sha256 = "…" }` — an http(s) tarball dependency
/// (SPEC.md §6). `sha256` is REQUIRED: integrity is non-negotiable, so a `url`
/// source with no digest is a parse error, never a silent unverified fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct UrlDependency {
    pub url: String,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct WorkspaceDependency {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// One `[tasks]` entry: a single shell command, or a sequence run in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum TaskValue {
    Single(String),
    Multiple(Vec<String>),
}

/// A lint severity level in `[lint]` (SPEC.md §9): the analog of clippy's
/// `allow` / `warn` / `deny`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
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
pub const LINT_TIERS: &[&str] = &["correctness", "suspicious", "perf", "style", "pedantic"];

/// `[lint]` (SPEC.md §9): per-rule and per-tier level overrides plus a global
/// allow-list for the `global-write` rule.
///
/// Rule ids live in `luabox-lint` (which this crate must not depend on), so
/// keys that are neither `globals` nor a [`LINT_TIERS`] name are recorded as
/// rule-id overrides without validating the id here; `luabox-lint` resolves
/// (and can diagnose) unknown ids when it consumes the config.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Lint {
    /// Extra names the `global-write` rule treats as intentional globals.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub globals: Vec<String>,
    /// Tier-level overrides, keyed by a [`LINT_TIERS`] name.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tiers: BTreeMap<String, LintLevel>,
    /// Rule-id overrides (`unused-local = "allow"`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<String, LintLevel>,
}

/// `[workspace]` (SPEC.md §5).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Workspace {
    /// Member globs. Only a whole-segment `*` wildcard is supported
    /// (`packages/*`), matching cargo's common case.
    pub members: Vec<String>,
}

/// The typed, validated contents of a `luabox.toml`.
///
/// Construct via [`crate::manifest::Manifest::parse`]. Carries the parsed
/// [`toml_edit::DocumentMut`] alongside the typed view so edits (e.g.
/// [`crate::manifest::Manifest::set_dependency`]) preserve comments and
/// formatting for everything they don't touch.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub package: Package,
    pub build: Build,
    pub types: Types,
    pub dependencies: BTreeMap<String, Dependency>,
    pub dev_dependencies: BTreeMap<String, Dependency>,
    pub tasks: BTreeMap<String, TaskValue>,
    pub workspace: Option<Workspace>,
    /// `[lint]` configuration (SPEC.md §9).
    pub lint: Lint,
    pub(super) document: toml_edit::DocumentMut,
}

impl Manifest {
    /// The lossless, comment-preserving document this manifest was parsed
    /// from (or last serialized to). Mutating it directly is valid; prefer
    /// typed helpers like [`Manifest::set_dependency`] where available.
    #[must_use]
    pub fn document(&self) -> &toml_edit::DocumentMut {
        &self.document
    }
}

impl std::fmt::Display for Manifest {
    /// Renders the manifest as TOML text, preserving comments/formatting of
    /// everything untouched since parse.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.document)
    }
}
