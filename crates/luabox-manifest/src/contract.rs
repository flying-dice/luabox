//! The `luabox.toml` contract, declared once (SPEC.md §5).
//!
//! The manifest is described by two artifacts that must agree: the hand-rolled
//! parser ([`crate::model::Manifest::parse`]), which collects every error with
//! a span and a did-you-mean nudge, and the published JSON Schema
//! ([`crate::schema`]) that editors, validators and LLM coding assistants read.
//!
//! They used to be written out separately and pinned together by a parity
//! suite. They are now both *derived from this module*: every table below
//! names its keys once — spelling, value type, whether it is required, the
//! default the parser applies, and the prose an outside reader needs — and
//! [`crate::parse`] builds its allow-lists from it while [`crate::schema`]
//! renders the schema document from it. A key that exists in one and not the
//! other is no longer a test failure; it is unrepresentable.
//!
//! What deliberately does **not** live here is the cross-key validation the
//! parser hand-codes (a dependency's mutually exclusive source forms,
//! `sha256`⇔`url`, `rev`/`tag`/`branch`⇔`git`, `[lint]`'s open rule ids) and
//! the handful of JSON Schema shapes a key table cannot express. Those stay
//! hand-written, in the parser and in [`crate::schema`]'s fragments
//! respectively, and are what the surviving corpus tests guard.

use crate::model::{BundleMode, DialectId, LintLevel};

/// A JSON scalar or array, for the `default` a key carries and the `examples`
/// the schema shows.
///
/// Deliberately tiny: these slots only ever hold a string, a boolean, or a
/// list of those. Anything richer belongs in a hand-written fragment, not in
/// a data model that has to be constructible in a `const`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Json {
    Str(&'static str),
    Bool(bool),
    Array(&'static [Json]),
}

/// The shape a key's value may take.
///
/// This is the *schema* view of the value. The parser's matching reader
/// (`get_string`, `get_bool`, `get_string_array`) is chosen by hand at each
/// call site, and `every_declared_value_type_is_the_one_the_parser_enforces`
/// in [`crate::schema`] holds the two together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueTy {
    /// `"type": "string"`, optionally constrained by a regex `pattern`.
    Str { pattern: Option<&'static str> },
    /// `"type": "boolean"`.
    Bool,
    /// `"type": "array"` whose entries take the inner shape.
    ArrayOf {
        item: &'static ValueTy,
        /// Prose for one entry — the schema describes array items too.
        item_description: Option<&'static str>,
    },
    /// `{"$ref": "#/$defs/<name>"}`: a closed vocabulary ([`VOCABULARIES`]) or
    /// another table in this module.
    Ref(&'static str),
    /// A hand-written JSON object spliced in after `description`, for the one
    /// shape this table cannot express — `[package] version`'s "semver, or the
    /// empty string that means absent".
    Fragment(&'static str),
}

/// One key of one table: everything both the parser and the schema need to
/// know about it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KeySpec {
    /// The spelling in `luabox.toml`.
    pub(crate) name: &'static str,
    pub(crate) ty: ValueTy,
    /// Whether omitting the key is an error. The parser's behaviour is the
    /// authority; `the_required_keys_are_exactly_the_ones_the_parser_demands`
    /// checks this flag against it.
    pub(crate) required: bool,
    /// A short label. `None` where the schema carries none — the root tables,
    /// whose `$ref` target supplies its own.
    pub(crate) title: Option<&'static str>,
    /// The prose an outside reader gets. Never empty: this is the whole point
    /// of publishing the schema, so it is a struct field, not an option.
    pub(crate) description: &'static str,
    /// The value the parser substitutes when the key is absent, where there
    /// is one worth publishing.
    pub(crate) default: Option<Json>,
    pub(crate) examples: &'static [Json],
}

/// One table of the manifest, as it appears in the schema's `$defs`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TableSpec {
    /// The `$defs` member name the schema publishes it under.
    pub(crate) def: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) keys: &'static [KeySpec],
}

/// A closed value vocabulary, published as a `$defs` `enum`.
///
/// `names` is the Rust enum's own `NAMES` slice, so a new variant reaches the
/// schema by existing — there is nothing to keep in step.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VocabularySpec {
    pub(crate) def: &'static str,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) names: &'static [&'static str],
}

/// The key spellings of `keys`, in declaration order — the allow-list
/// [`crate::parse`] matches an unknown key against, and the `(valid: …)` set
/// the resulting error prints.
pub(crate) fn names(keys: &[KeySpec]) -> Vec<&'static str> {
    keys.iter().map(|key| key.name).collect()
}

// ---------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------

/// The top-level tables of `luabox.toml`.
///
/// Every one is a table, so each `ty` is a [`ValueTy::Ref`] to the table's own
/// definition and the parser's matching reader is `get_table`.
pub(crate) const ROOT: &[KeySpec] = &[
    KeySpec {
        name: "package",
        ty: ValueTy::Ref("package"),
        required: true,
        title: None,
        description: "Package identity and the dialect you write (SPEC.md §5, §6, §15). The only required table: a manifest with no `[package]` is rejected.",
        default: None,
        examples: &[],
    },
    KeySpec {
        name: "build",
        ty: ValueTy::Ref("build"),
        required: false,
        title: None,
        description: "Configuration for the single `luabox build` command — the dialect you ship, where output lands, and whether to bundle (SPEC.md §5, §7). Every field has a default, so the table may be omitted entirely; every field is also overridable by a `luabox build` flag.",
        default: None,
        examples: &[],
    },
    KeySpec {
        name: "types",
        ty: ValueTy::Ref("types"),
        required: false,
        title: None,
        description: "Typechecker configuration for `luabox check` (SPEC.md §5): strictness, and the ambient LuaCATS definition packages that join this project's scope.",
        default: None,
        examples: &[],
    },
    KeySpec {
        name: "dependencies",
        ty: ValueTy::Ref("dependencyMap"),
        required: false,
        title: None,
        description: "Packages this project requires at runtime, keyed by package name (SPEC.md §5, §6).\n\nluabox resolves nothing: there is no solver, no lockfile and no download in v1. The table is still read — each entry names a package whose `lua_modules/<name>/luabox.toml` is consulted for its `[types] defs`, so a dependency's ambient LuaCATS definitions join this project's scope (the luals `workspace.library` model). `require` resolution does not consult this table at all: it searches `lua_modules/` by path, so a rock is requirable and bundlable whether or not it is listed here.",
        default: None,
        examples: &[],
    },
    KeySpec {
        name: "dev-dependencies",
        ty: ValueTy::Ref("dependencyMap"),
        required: false,
        title: None,
        description: "Packages needed only for development and tests, keyed by package name. Identical in shape and handling to `dependencies` (SPEC.md §5, §6).",
        default: None,
        examples: &[],
    },
    KeySpec {
        name: "lint",
        ty: ValueTy::Ref("lint"),
        required: false,
        title: None,
        description: "Lint configuration for `luabox lint` (SPEC.md §9): per-tier and per-rule severity levels, plus the deliberate-global allow-list.",
        default: None,
        examples: &[],
    },
];

pub(crate) const PACKAGE: TableSpec = TableSpec {
    def: "package",
    title: "[package]",
    description: "Package identity (SPEC.md §5, §6, §15). Only `edition` is required here: `name` and `version` are optional in `luabox.toml` because the project's rockspec is the package manifest luarocks reads, and it supplies them. Commands that need a name substitute one (the project directory name, or `bundle`) rather than demanding it.",
    keys: &[
        KeySpec {
            name: "name",
            ty: ValueTy::Str {
                pattern: Some("^$|^[a-z-][a-z0-9-]*$|^@[a-z-][a-z0-9-]*/[a-z-][a-z0-9-]*$"),
            },
            required: false,
            title: Some("Package name"),
            description: "The package name: a plain segment (`penlight`), or a scoped `@org/pkg` form (SPEC.md §19). Each segment must be lowercase ASCII alphanumeric or `-`, must not be empty, and must not start with a digit. Optional — the rockspec supplies it — and an empty string is treated as absent.",
            default: None,
            examples: &[
                Json::Str("my-lib"),
                Json::Str("hello-luabox"),
                Json::Str("@acme/widgets"),
            ],
        },
        KeySpec {
            name: "version",
            ty: ValueTy::Fragment(
                r##"{ "anyOf": [{ "$ref": "#/$defs/semver" }, { "const": "" }] }"##,
            ),
            required: false,
            title: Some("Package version"),
            description: "This package's own version, semver-shaped (`X.Y.Z` with optional `-pre-release`/`+build`). Optional — the rockspec supplies it — and an empty string is treated as absent.",
            default: None,
            examples: &[Json::Str("1.2.0"), Json::Str("0.1.0")],
        },
        KeySpec {
            name: "edition",
            ty: ValueTy::Ref("dialect"),
            required: true,
            title: Some("Dialect you write"),
            description: "The Lua dialect this project's sources are written in — the language `luabox check`, `luabox fmt` and `luabox lint` accept. Required: it is the one manifest key the toolchain cannot infer. Also the default for `build.target`, so a project that writes and ships the same dialect need only set this.",
            default: None,
            examples: &[Json::Str("5.4"), Json::Str("luajit")],
        },
        KeySpec {
            name: "description",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Package description"),
            description: "A one-line human description of the package. Metadata only — nothing in the toolchain reads it.",
            default: None,
            examples: &[Json::Str("Geometry primitives for 2D games")],
        },
        KeySpec {
            name: "license",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("License"),
            description: "The package's license, conventionally an SPDX identifier. Metadata only — nothing in the toolchain reads it, and it is not validated against the SPDX list.",
            default: None,
            examples: &[Json::Str("MIT"), Json::Str("Apache-2.0")],
        },
        KeySpec {
            name: "lua-versions",
            ty: ValueTy::ArrayOf {
                item: &ValueTy::Ref("dialect"),
                item_description: None,
            },
            required: false,
            title: Some("Compatible dialects"),
            description: "The dialects this package declares itself compatible with (SPEC.md §6). Parsed, validated, and inert: an entry that is not a known dialect is a manifest error, but nothing evaluates the list — it fed the dependency dialect-set check, which was parked with the solver. Kept so a manifest written for the return of dependency management still validates.",
            default: Some(Json::Array(&[])),
            examples: &[Json::Array(&[Json::Str("5.1"), Json::Str("5.4")])],
        },
        KeySpec {
            name: "min-luabox-version",
            ty: ValueTy::Ref("semver"),
            required: false,
            title: Some("Minimum toolchain version"),
            description: "The minimum luabox toolchain version this package needs (SPEC.md §15). Parsed, validated, and inert: a non-semver string is a manifest error, but nothing enforces the bound — it fed the resolver's toolchain gate, which was parked with the solver.",
            default: None,
            examples: &[Json::Str("0.3.0")],
        },
    ],
};

pub(crate) const BUILD: TableSpec = TableSpec {
    def: "build",
    title: "[build]",
    description: "The `luabox build` configuration (SPEC.md §5, §7): one tsc/esbuild-style emit that lowers `package.edition` to `build.target` (goto, bitwise operators, `<close>`, `_ENV`, …) with tree-shaken polyfills, then either mirrors the source tree under `out` or bundles the require graph into one file per entry. Every `luabox build` flag overrides the matching key here.",
    keys: &[
        KeySpec {
            name: "target",
            ty: ValueTy::Ref("dialect"),
            required: false,
            title: Some("Dialect you ship"),
            description: "The Lua dialect `luabox build` lowers to and emits. Defaults to `package.edition` — i.e. no lowering — so set it only when you ship a different dialect than you write. Overridden by `luabox build --target`.",
            default: None,
            examples: &[Json::Str("5.1")],
        },
        KeySpec {
            name: "out",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Output directory"),
            description: "The directory tree-mode emit and multi-entry bundles are written to, relative to the project root. Overridden by `luabox build --out`.",
            default: Some(Json::Str("dist")),
            examples: &[Json::Str("dist"), Json::Str("build")],
        },
        KeySpec {
            name: "mode",
            ty: ValueTy::Ref("bundleMode"),
            required: false,
            title: Some("Embedding mode"),
            description: "How the build output is packaged (SPEC.md §7). A mode other than `plain` implies bundling and is incompatible with `outfile`. Overridden by `luabox build --mode`.",
            default: Some(Json::Str("plain")),
            examples: &[Json::Str("love")],
        },
        KeySpec {
            name: "entry",
            ty: ValueTy::ArrayOf {
                item: &ValueTy::Str { pattern: None },
                item_description: Some("A project-relative path to one Lua entry point."),
            },
            required: false,
            title: Some("Bundle entry points"),
            description: "The bundle entry points, one single-file bundle each, as project-relative paths. Only consulted when bundling (`bundle = true`, or a non-`plain` `mode`). Omit the key to get the conventional single entry point; write an explicit empty list for a library with nothing to bundle. Overridden by repeated `luabox build --entry` flags.",
            default: Some(Json::Array(&[Json::Str("src/main.lua")])),
            examples: &[
                Json::Array(&[Json::Str("src/main.lua")]),
                Json::Array(&[Json::Str("src/cli.lua"), Json::Str("src/worker.lua")]),
            ],
        },
        KeySpec {
            name: "outfile",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Single-entry bundle output path"),
            description: "Write the single bundle to exactly this path (esbuild's `--outfile`), instead of deriving its name from the entry basename under `out`. Valid only with exactly one entry, and never with a non-`plain` `mode`. Overridden by `luabox build --outfile`.",
            default: None,
            examples: &[Json::Str("dist/app.lua")],
        },
        KeySpec {
            name: "bundle",
            ty: ValueTy::Bool,
            required: false,
            title: Some("Bundle instead of mirroring the tree"),
            description: "Inline the require graph into one single-file bundle per entry, instead of mirroring the source tree under `out`. A non-`plain` `mode` implies bundling regardless. Overridden by `luabox build --bundle` / `--no-bundle`.",
            default: Some(Json::Bool(false)),
            examples: &[],
        },
        KeySpec {
            name: "sourcemap",
            ty: ValueTy::Bool,
            required: false,
            title: Some("Emit source maps"),
            description: "Write a `<bundle>.map` beside each bundle so `luabox unmap` can decode a production traceback back to source lines. Only meaningful when bundling, and only recorded at build time — a map cannot be reconstructed after the fact. Overridden by `luabox build --sourcemap`.",
            default: Some(Json::Bool(false)),
            examples: &[],
        },
        KeySpec {
            name: "minify",
            ty: ValueTy::Bool,
            required: false,
            title: Some("Minify each bundle"),
            description: "Scope-aware identifier mangling plus whitespace collapse on each bundle. Only meaningful when bundling. Overridden by `luabox build --minify`.",
            default: Some(Json::Bool(false)),
            examples: &[],
        },
    ],
};

pub(crate) const TYPES: TableSpec = TableSpec {
    def: "types",
    title: "[types]",
    description: "Typechecker configuration for `luabox check` (SPEC.md §5).",
    keys: &[
        KeySpec {
            name: "strict",
            ty: ValueTy::Bool,
            required: false,
            title: Some("Strict typechecking"),
            description: "Report LuaCATS type mismatches as errors rather than warnings. `false` is the friendly on-ramp for an existing annotated codebase: mismatches surface as warnings until you turn the dial.",
            default: Some(Json::Bool(false)),
            examples: &[],
        },
        KeySpec {
            name: "defs",
            ty: ValueTy::ArrayOf {
                item: &ValueTy::Str { pattern: None },
                item_description: Some(
                    "A definition package name — the stem of a `defs/<name>.d.lua` file under this package's root.",
                ),
            },
            required: false,
            title: Some("Ambient definition packages"),
            description: "Ambient LuaCATS definition packages (`*.d.lua` / `---@meta` modules) whose declarations join this project's scope (SPEC.md §3). Each entry is a file stem resolved under this package's own `defs/` directory — `\"love2d\"` resolves `defs/love2d.d.lua`. A dependency that publishes its own `[types] defs` contributes them automatically; you do not list a dependency's defs here.",
            default: Some(Json::Array(&[])),
            examples: &[Json::Array(&[Json::Str("love2d")])],
        },
    ],
};

/// The inline-table form of a dependency. The *combinations* of these keys
/// that are legal are not expressible here — they are hand-coded in
/// [`crate::parse`] and mirrored by the `oneOf` fragment in [`crate::schema`].
pub(crate) const DEPENDENCY: TableSpec = TableSpec {
    def: "dependencyTable",
    title: "Dependency table entry",
    description: "An inline-table dependency. Exactly one source key may be present — `path`, `git`, or `url` — or none at all, in which case `version` alone is required and the entry means exactly what the bare-string form means. `sha256` is valid only alongside `url` (and mandatory there); `rev`/`tag`/`branch` require a `git` source — with no source, or with a `path` or `url` source, they are an error rather than a silently ignored key — and at most one of the three may be given.",
    keys: &[
        KeySpec {
            name: "git",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Git source"),
            description: "A git repository URL to take the package from. Pin it with at most one of `rev`, `tag` or `branch`.",
            default: None,
            examples: &[Json::Str("https://github.com/flying-dice/luabox")],
        },
        KeySpec {
            name: "rev",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Git revision"),
            description: "An exact commit to check out. Requires a `git` source, and is mutually exclusive with `tag` and `branch`.",
            default: None,
            examples: &[Json::Str("9f2c1ab")],
        },
        KeySpec {
            name: "tag",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Git tag"),
            description: "A tag to check out. Requires a `git` source, and is mutually exclusive with `rev` and `branch`.",
            default: None,
            examples: &[Json::Str("v1.0.0")],
        },
        KeySpec {
            name: "branch",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Git branch"),
            description: "A branch to track. Requires a `git` source, and is mutually exclusive with `rev` and `tag`.",
            default: None,
            examples: &[Json::Str("main")],
        },
        KeySpec {
            name: "path",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Path source"),
            description: "A filesystem path to a sibling package, read in place — luabox never copies or fetches it. Relative paths are resolved against this manifest's directory. This is how a monorepo-style tree shares a library, and how the depended-on package's `[types] defs` reach this project.",
            default: None,
            examples: &[Json::Str("../geometry")],
        },
        KeySpec {
            name: "url",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Tarball source"),
            description: "An http(s) (or `file://`) tarball to take the package from. Requires `sha256`: integrity is non-negotiable, so a `url` with no digest is a hard error rather than a silent unverified fetch.",
            default: None,
            examples: &[Json::Str("https://example.com/pkg-1.0.tar.gz")],
        },
        KeySpec {
            name: "sha256",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Tarball digest"),
            description: "The SHA-256 digest that pins the `url` tarball. Mandatory alongside `url`, and meaningless — an error — without one.",
            default: None,
            examples: &[Json::Str(
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            )],
        },
        KeySpec {
            name: "version",
            ty: ValueTy::Str { pattern: None },
            required: false,
            title: Some("Version requirement"),
            description: "The declared version expectation for this package. On its own it is the bare-string form spelled longhand — `pkg = { version = \"1.0\" }` means exactly `pkg = \"1.0\"`. Alongside a source key it is recorded, not dropped: it is metadata reserved for when resolution returns, like `rev`/`tag`/`branch`.",
            default: None,
            examples: &[Json::Str("1.0"), Json::Str("1.2.3")],
        },
    ],
};

/// `[lint]` is the one half-open table: the keys below are fixed, and any
/// other key is a lint rule id whose *value* is still closed to a level. The
/// parser therefore never runs an unknown-key check over these — they are
/// here for the schema, and for `globals`, which is the one key that is not a
/// level.
pub(crate) const LINT: TableSpec = TableSpec {
    def: "lint",
    title: "[lint]",
    description: "Lint configuration (SPEC.md §9). Apart from `globals`, every key sets a severity level: the five keys named below are *tier* toggles that move a whole family of rules at once, and any other key is read as a lint rule id (`unused-local`, `global-write`, …). Rule ids are open here — an id that names no known rule is reported by `luabox lint` as LB1004, not by manifest parsing — but the level value is closed to `allow`/`warn`/`deny` either way. A per-rule level beats its tier.",
    keys: &[
        KeySpec {
            name: "globals",
            ty: ValueTy::ArrayOf {
                item: &ValueTy::Str { pattern: None },
                item_description: Some("A global name to accept as deliberate."),
            },
            required: false,
            title: Some("Deliberate globals"),
            description: "Extra names the `global-write` rule treats as intentional globals rather than accidents — the escape hatch for a library that deliberately publishes itself as a global, or for a host environment's injected names.",
            default: Some(Json::Array(&[])),
            examples: &[Json::Array(&[Json::Str("vim"), Json::Str("love")])],
        },
        KeySpec {
            name: "correctness",
            ty: ValueTy::Ref("lintLevel"),
            required: false,
            title: Some("correctness tier"),
            description: "Level for the whole `correctness` tier — rules for code that is outright wrong.",
            default: None,
            examples: &[],
        },
        KeySpec {
            name: "suspicious",
            ty: ValueTy::Ref("lintLevel"),
            required: false,
            title: Some("suspicious tier"),
            description: "Level for the whole `suspicious` tier — rules for code that is probably not what was meant.",
            default: None,
            examples: &[],
        },
        KeySpec {
            name: "perf",
            ty: ValueTy::Ref("lintLevel"),
            required: false,
            title: Some("perf tier"),
            description: "Level for the whole `perf` tier — rules for avoidably slow constructs.",
            default: None,
            examples: &[],
        },
        KeySpec {
            name: "style",
            ty: ValueTy::Ref("lintLevel"),
            required: false,
            title: Some("style tier"),
            description: "Level for the whole `style` tier — rules for idiomatic Lua.",
            default: None,
            examples: &[],
        },
        KeySpec {
            name: "pedantic",
            ty: ValueTy::Ref("lintLevel"),
            required: false,
            title: Some("pedantic tier"),
            description: "Level for the whole `pedantic` tier — strict rules that are off or advisory by default.",
            default: None,
            examples: &[],
        },
    ],
};

/// The closed value vocabularies, in the order the schema publishes them.
pub(crate) const VOCABULARIES: &[VocabularySpec] = &[
    VocabularySpec {
        def: "dialect",
        title: "Lua dialect",
        description: "A Lua dialect luabox understands (SPEC.md §2, §6). `luajit` is the LuaJIT dialect, which is 5.1-compatible with extensions.",
        names: DialectId::NAMES,
    },
    VocabularySpec {
        def: "bundleMode",
        title: "Bundler embedding mode",
        description: "How `luabox build` packages its output (SPEC.md §7). `plain` writes a bare single-file `.lua` chunk; `love` produces a LÖVE `.love` zip archive; `nvim-plugin` produces a Neovim `runtimepath` plugin tree (`lua/<name>/init.lua`). Any mode other than `plain` implies bundling regardless of `build.bundle`.",
        names: BundleMode::NAMES,
    },
    VocabularySpec {
        def: "lintLevel",
        title: "Lint severity level",
        description: "The severity a lint tier or rule fires at — the analog of clippy's levels (SPEC.md §9). `allow` turns the rule off entirely; `warn` reports it without failing the command; `deny` reports it as an error and fails the command.",
        names: LintLevel::NAMES,
    },
];
