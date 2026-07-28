//! Typed model for `luabox.toml` (SPEC.md §5, §6, §15).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// The error every closed-vocabulary [`FromStr`] in this module returns: the
/// spelling was not one of the accepted ones.
///
/// It carries the rejected spelling *and* the accepted set, so a caller can
/// render its own phrasing (`luabox-cli`'s `LB1001`) instead of string-matching
/// ours. [`Display`](fmt::Display) renders the manifest-validation phrasing —
/// the rejected value in backticks followed by `(valid: …)` — that
/// [`Manifest::parse`] builds its errors from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownValue {
    /// The spelling that was rejected.
    pub value: String,
    /// Every accepted spelling, in declaration order.
    pub expected: &'static [&'static str],
}

impl fmt::Display for UnknownValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` (valid: {})", self.value, self.expected.join(", "))
    }
}

impl std::error::Error for UnknownValue {}

/// Declare a closed string vocabulary as an enum with `as_str`, `ALL`,
/// `NAMES`, [`Display`](fmt::Display) and [`FromStr`].
///
/// Distribution never parses syntax (SPEC.md §16), so a dialect is a *local*
/// discriminant here, not `luabox_syntax::Dialect`; frontends map it inward
/// with an exhaustive match (which is what makes their old
/// "unknown edition in a validated manifest" bail arms unreachable, and so
/// deletable).
macro_rules! closed_vocabulary {
    (
        $(#[$meta:meta])*
        $name:ident { $( $(#[$vmeta:meta])* $variant:ident => $spelling:literal ),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];
            /// Every accepted spelling, in declaration order.
            pub const NAMES: &'static [&'static str] = &[ $( $spelling ),+ ];

            /// This value's `luabox.toml` spelling.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( $name::$variant => $spelling ),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = UnknownValue;

            fn from_str(raw: &str) -> Result<Self, UnknownValue> {
                match raw {
                    $( $spelling => Ok($name::$variant), )+
                    other => Err(UnknownValue {
                        value: other.to_owned(),
                        expected: $name::NAMES,
                    }),
                }
            }
        }
    };
}

closed_vocabulary! {
    /// Dialects accepted for `[package] edition`, `[build] target`, and
    /// `[package] lua-versions` entries (SPEC.md §2, §6).
    ///
    /// A validated manifest can only carry one of these five, so a frontend
    /// converts it to its own dialect type with an exhaustive match rather
    /// than re-validating a string that already passed.
    DialectId {
        Lua51 => "5.1",
        Lua52 => "5.2",
        Lua53 => "5.3",
        Lua54 => "5.4",
        LuaJit => "luajit",
    }
}

closed_vocabulary! {
    /// Bundler embedding modes for `[build] mode` (SPEC.md §7): `plain` (a
    /// bare chunk, the default), `love` (LÖVE `.love` packaging), and
    /// `nvim-plugin` (a Neovim `lua/<name>/init.lua` runtimepath layout).
    #[derive(Default)]
    BundleMode {
        /// A single-file `.lua` chunk written out verbatim.
        #[default]
        Plain => "plain",
        /// A LÖVE `.love` zip archive.
        Love => "love",
        /// A Neovim `runtimepath` plugin tree.
        NvimPlugin => "nvim-plugin",
    }
}

closed_vocabulary! {
    /// The lint tier names a `[lint]` toggle may target (SPEC.md §9).
    ///
    /// Held as a local vocabulary rather than a dependency on `luabox-lint` —
    /// Distribution never depends on the Semantics/Frontend crates (SPEC.md
    /// §16, acyclic graph). It classifies a `[lint]` key as tier-or-rule while
    /// parsing: the five tier names are *closed* (a key spelled like one is a
    /// tier and nothing else), while rule ids stay open because they live in
    /// `luabox-lint`, which validates them when it consumes the config.
    LintTier {
        Correctness => "correctness",
        Suspicious => "suspicious",
        Perf => "perf",
        Style => "style",
        Pedantic => "pedantic",
    }
}

/// `[package]` (SPEC.md §5, §6, §15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// Dialect you write: one of `5.1`, `5.2`, `5.3`, `5.4`, `luajit`.
    pub edition: DialectId,
    pub description: Option<String>,
    pub license: Option<String>,
    /// SPEC.md §6: dialects this package declares itself compatible with.
    pub lua_versions: Vec<DialectId>,
    /// SPEC.md §15: minimum toolchain version the resolver must respect.
    pub min_luabox_version: Option<String>,
}

/// `[build]` (SPEC.md §5, §7) — the single `luabox build` command's
/// configuration (flying-dice/luabox#4). CLI flags override every field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Build {
    /// Dialect you ship. Defaults to `[package] edition` when absent.
    pub target: DialectId,
    /// Output directory: tree-mode emit and multi-entry bundles land here.
    /// Defaults to `"dist"`.
    pub out: String,
    /// Bundler embedding mode (SPEC.md §7). Defaults to
    /// [`BundleMode::Plain`]; `luabox build --mode` overrides it.
    pub mode: BundleMode,
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

/// The default output directory (`[build] out`, SPEC.md §7).
pub const DEFAULT_OUT: &str = "dist";

impl Build {
    /// The `[build]` config a manifest with no `[build]` table gets, shipping
    /// to `target`.
    ///
    /// This is also what a *manifest-less* project builds with, so the two
    /// cannot drift: `Manifest::parse` and `luabox build`'s no-manifest path
    /// call the same constructor rather than each spelling the defaults out.
    #[must_use]
    pub fn defaults(target: DialectId) -> Build {
        Build {
            target,
            out: DEFAULT_OUT.to_owned(),
            mode: BundleMode::default(),
            entry: vec![DEFAULT_ENTRY.to_owned()],
            outfile: None,
            bundle: false,
            sourcemap: false,
            minify: false,
        }
    }
}

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

closed_vocabulary! {
    /// A lint severity level in `[lint]` (SPEC.md §9): the analog of clippy's
    /// `allow` / `warn` / `deny`.
    LintLevel {
        /// Rule is off — no diagnostics.
        Allow => "allow",
        /// Rule fires at warning severity (does not fail the command).
        Warn => "warn",
        /// Rule fires at error severity (fails the command).
        Deny => "deny",
    }
}

/// `[lint]` (SPEC.md §9): per-rule and per-tier level overrides plus a global
/// allow-list for the `global-write` rule.
///
/// Rule ids live in `luabox-lint` (which this crate must not depend on), so
/// keys that are neither `globals` nor a known tier name are recorded as
/// rule-id overrides without validating the id here; `luabox-lint` resolves
/// (and can diagnose) unknown ids when it consumes the config. Tier keys, by
/// contrast, are closed: they are typed as [`LintTier`], so no unrecognised
/// tier name can reach a consumer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lint {
    /// Extra names the `global-write` rule treats as intentional globals.
    pub globals: Vec<String>,
    /// Tier-level overrides.
    pub tiers: BTreeMap<LintTier, LintLevel>,
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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;

    /// Every closed vocabulary round-trips spelling → variant → spelling, and
    /// keeps `ALL`/`NAMES` index-aligned. Asserting it once per type through a
    /// shared body keeps the macro's contract in one place.
    macro_rules! assert_round_trips {
        ($ty:ty) => {{
            assert_eq!(
                <$ty>::ALL.len(),
                <$ty>::NAMES.len(),
                "ALL and NAMES must stay index-aligned"
            );
            for (value, name) in <$ty>::ALL.iter().zip(<$ty>::NAMES) {
                assert_eq!(value.as_str(), *name);
                assert_eq!(value.to_string(), *name);
                assert_eq!(name.parse::<$ty>().as_ref(), Ok(value));
            }
        }};
    }

    #[test]
    fn every_closed_vocabulary_round_trips_through_its_spelling() {
        assert_round_trips!(DialectId);
        assert_round_trips!(BundleMode);
        assert_round_trips!(LintTier);
        assert_round_trips!(LintLevel);
    }

    #[test]
    fn an_unaccepted_spelling_reports_itself_and_the_accepted_set() {
        let error = "5.9"
            .parse::<DialectId>()
            .expect_err("5.9 is not a dialect");
        assert_eq!(error.value, "5.9");
        assert_eq!(error.expected, DialectId::NAMES);
        assert_eq!(
            error.to_string(),
            "`5.9` (valid: 5.1, 5.2, 5.3, 5.4, luajit)"
        );

        let error = "roblox".parse::<BundleMode>().expect_err("not a mode");
        assert_eq!(
            error.to_string(),
            "`roblox` (valid: plain, love, nvim-plugin)"
        );
    }

    #[test]
    fn the_default_bundle_mode_is_a_bare_chunk() {
        assert_eq!(BundleMode::default(), BundleMode::Plain);
    }
}
