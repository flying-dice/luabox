//! `luabox.toml` parsing and validation (SPEC.md §5).
//!
//! All errors are collected — a single `Manifest::parse` call reports every
//! problem in the file, not just the first (cargo/rustc-style batch
//! diagnostics, SPEC.md §14).

use std::collections::BTreeMap;
use std::ops::Range;

use toml_edit::{ImDocument, Item, Table, TableLike};

use super::error::ManifestError;
use super::model::{
    ALLOWED_BUNDLE_MODES, ALLOWED_DIALECTS, Build, DEFAULT_ENTRY, Dependency, GitDependency,
    LINT_TIERS, Lint, LintLevel, Manifest, Package, PathDependency, Types, UrlDependency,
};

const TOP_LEVEL_KEYS: &[&str] = &[
    "package",
    "build",
    "types",
    "dependencies",
    "dev-dependencies",
    "lint",
];
/// Top-level tables 0.1.4 accepted and 0.2.0 dropped (#18): they only ever
/// served the removed `run` command and the parked solver. A manifest written
/// against the old release carries them across the upgrade untouched, so
/// their unknown-table error also says they *were* real — the did-you-mean
/// nudge alone would leave the reader hunting for a typo that isn't there.
const REMOVED_TOP_LEVEL_TABLES: &[&str] = &["tasks", "workspace"];
const REMOVED_TABLE_NOTE: &str = "removed in 0.2.0, see CHANGELOG.md";
const LINT_LEVELS: &[&str] = &["allow", "warn", "deny"];
const PACKAGE_KEYS: &[&str] = &[
    "name",
    "version",
    "edition",
    "description",
    "license",
    "lua-versions",
    "min-luabox-version",
];
const BUILD_KEYS: &[&str] = &[
    "target",
    "out",
    "mode",
    "entry",
    "outfile",
    "bundle",
    "sourcemap",
    "minify",
];
const TYPES_KEYS: &[&str] = &["strict", "defs"];
const DEPENDENCY_KEYS: &[&str] = &[
    "git", "rev", "tag", "branch", "path", "url", "sha256", "version",
];

impl Manifest {
    /// Parse and validate a `luabox.toml` document.
    ///
    /// Collects *every* validation error rather than stopping at the first;
    /// on success, the returned [`Manifest`] carries the source
    /// [`DocumentMut`] for lossless, comment-preserving edits.
    pub fn parse(text: &str) -> Result<Manifest, Vec<ManifestError>> {
        // Parsed as an `ImDocument` first (not `DocumentMut` directly):
        // `DocumentMut::from_str` despans on construction, so item/key spans
        // (needed for span-rich errors below) would already be gone by the
        // time validation runs. `ImDocument::into_mut` despans too, but only
        // *after* we're done reading spans from it.
        let im_document = match ImDocument::<String>::parse(text.to_owned()) {
            Ok(document) => document,
            Err(error) => return Err(vec![ManifestError::new(error.to_string(), error.span())]),
        };

        let mut errors = Vec::new();
        let root = im_document.as_table();
        check_top_level_keys(root, &mut errors);

        let package = parse_package(root, &mut errors);
        let build = parse_build(root, &package.edition, &mut errors);
        let types = parse_types(root, &mut errors);
        let dependencies = parse_dependencies(root, "dependencies", &mut errors);
        let dev_dependencies = parse_dependencies(root, "dev-dependencies", &mut errors);
        let lint = parse_lint(root, &mut errors);

        if errors.is_empty() {
            Ok(Manifest {
                package,
                build,
                types,
                dependencies,
                dev_dependencies,
                lint,
                document: im_document.into_mut(),
            })
        } else {
            Err(errors)
        }
    }
}

// ---------------------------------------------------------------------
// Generic TOML-shape helpers (work over both real tables and inline tables)
// ---------------------------------------------------------------------

fn key_span(table: &dyn TableLike, key: &str) -> Option<Range<usize>> {
    table.get_key_value(key).and_then(|(k, _)| k.span())
}

fn item_span(table: &dyn TableLike, key: &str) -> Option<Range<usize>> {
    table.get(key).and_then(Item::span)
}

fn check_unknown_keys(
    table: &dyn TableLike,
    what: &str,
    valid: &[&str],
    errors: &mut Vec<ManifestError>,
) {
    for (key, _) in table.iter() {
        if !valid.contains(&key) {
            errors.push(ManifestError::unknown_key(
                what,
                key,
                valid,
                key_span(table, key),
            ));
        }
    }
}

/// [`check_unknown_keys`] for the root table, plus the removal nudge for the
/// two tables 0.2.0 dropped ([`REMOVED_TOP_LEVEL_TABLES`]).
fn check_top_level_keys(root: &Table, errors: &mut Vec<ManifestError>) {
    for (key, _) in root {
        if TOP_LEVEL_KEYS.contains(&key) {
            continue;
        }
        let error =
            ManifestError::unknown_key("top-level table", key, TOP_LEVEL_KEYS, key_span(root, key));
        errors.push(if REMOVED_TOP_LEVEL_TABLES.contains(&key) {
            error.with_note(REMOVED_TABLE_NOTE)
        } else {
            error
        });
    }
}

fn get_table<'a>(
    table: &'a dyn TableLike,
    key: &str,
    errors: &mut Vec<ManifestError>,
) -> Option<&'a dyn TableLike> {
    match table.get(key) {
        None => None,
        Some(item) => {
            if let Some(inner) = item.as_table_like() {
                Some(inner)
            } else {
                errors.push(ManifestError::new(
                    format!("`[{key}]` must be a table"),
                    item.span(),
                ));
                None
            }
        }
    }
}

fn get_string(
    table: &dyn TableLike,
    ctx: &str,
    key: &str,
    required: bool,
    errors: &mut Vec<ManifestError>,
) -> Option<String> {
    if let Some(item) = table.get(key) {
        if let Some(s) = item.as_str() {
            Some(s.to_owned())
        } else {
            errors.push(ManifestError::new(
                format!("`{ctx}.{key}` must be a string"),
                item.span(),
            ));
            None
        }
    } else {
        if required {
            errors.push(ManifestError::new(
                format!("missing required key `{ctx}.{key}`"),
                None,
            ));
        }
        None
    }
}

fn get_bool(
    table: &dyn TableLike,
    ctx: &str,
    key: &str,
    default: bool,
    errors: &mut Vec<ManifestError>,
) -> bool {
    match table.get(key) {
        None => default,
        Some(item) => {
            if let Some(b) = item.as_bool() {
                b
            } else {
                errors.push(ManifestError::new(
                    format!("`{ctx}.{key}` must be a boolean"),
                    item.span(),
                ));
                default
            }
        }
    }
}

fn get_string_array(
    table: &dyn TableLike,
    ctx: &str,
    key: &str,
    errors: &mut Vec<ManifestError>,
) -> Vec<String> {
    let Some(item) = table.get(key) else {
        return Vec::new();
    };
    let Some(array) = item.as_array() else {
        errors.push(ManifestError::new(
            format!("`{ctx}.{key}` must be an array of strings"),
            item.span(),
        ));
        return Vec::new();
    };
    let mut out = Vec::with_capacity(array.len());
    for value in array {
        match value.as_str() {
            Some(s) => out.push(s.to_owned()),
            None => errors.push(ManifestError::new(
                format!("`{ctx}.{key}` entries must be strings"),
                value.span(),
            )),
        }
    }
    out
}

/// Like [`get_string_array`] but each entry is validated against
/// [`ALLOWED_DIALECTS`] (SPEC.md §2, §6).
fn get_dialect_array(
    table: &dyn TableLike,
    ctx: &str,
    key: &str,
    errors: &mut Vec<ManifestError>,
) -> Vec<String> {
    let Some(item) = table.get(key) else {
        return Vec::new();
    };
    let Some(array) = item.as_array() else {
        errors.push(ManifestError::new(
            format!("`{ctx}.{key}` must be an array of strings"),
            item.span(),
        ));
        return Vec::new();
    };
    let mut out = Vec::with_capacity(array.len());
    for value in array {
        match value.as_str() {
            Some(s) => {
                validate_dialect(&format!("{ctx}.{key} entry"), s, value.span(), errors);
                out.push(s.to_owned());
            }
            None => errors.push(ManifestError::new(
                format!("`{ctx}.{key}` entries must be strings"),
                value.span(),
            )),
        }
    }
    out
}

// ---------------------------------------------------------------------
// Value-level validation
// ---------------------------------------------------------------------

fn validate_dialect(
    what: &str,
    value: &str,
    span: Option<Range<usize>>,
    errors: &mut Vec<ManifestError>,
) {
    if !ALLOWED_DIALECTS.contains(&value) {
        errors.push(ManifestError::new(
            format!(
                "invalid {what} `{value}` (valid: {})",
                ALLOWED_DIALECTS.join(", ")
            ),
            span,
        ));
    }
}

/// Validates a package name: a plain segment (`penlight`), or a scoped
/// `@org/pkg` form (SPEC.md §19 — namespaces, scoped proposal).
fn validate_package_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("`package.name` must not be empty".to_owned());
    }
    if let Some(rest) = name.strip_prefix('@') {
        let Some((scope, pkg)) = rest.split_once('/') else {
            return Some(format!(
                "`package.name` \"{name}\" is scoped but not of the form `@scope/name`"
            ));
        };
        return validate_name_segment(name, scope).or_else(|| validate_name_segment(name, pkg));
    }
    validate_name_segment(name, name)
}

fn validate_name_segment(name: &str, segment: &str) -> Option<String> {
    if segment.is_empty() {
        return Some(format!(
            "`package.name` \"{name}\" must not have an empty scope or name segment"
        ));
    }
    if segment.starts_with(|c: char| c.is_ascii_digit()) {
        return Some(format!(
            "`package.name` \"{name}\" must not start with a digit"
        ));
    }
    if !segment
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some(format!(
            "`package.name` \"{name}\" must be lowercase ASCII alphanumeric or `-`"
        ));
    }
    None
}

/// Light semver-shaped check: `X.Y.Z` with an optional `-pre-release` and/or
/// `+build` suffix. Not a full semver parser.
///
/// Shape-checking is all v1 needs: luabox never compares or matches versions
/// (it resolves nothing). A full `semver`-crate validation is only warranted
/// if version *ordering* ever becomes a luabox concern.
fn looks_like_semver(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------
// Section parsers
// ---------------------------------------------------------------------

fn parse_package(root: &Table, errors: &mut Vec<ManifestError>) -> Package {
    let Some(table) = get_table(root, "package", errors) else {
        errors.push(ManifestError::new(
            "missing required table `[package]`",
            None,
        ));
        return Package {
            name: String::new(),
            version: String::new(),
            edition: String::new(),
            description: None,
            license: None,
            lua_versions: Vec::new(),
            min_luabox_version: None,
        };
    };
    check_unknown_keys(table, "[package] key", PACKAGE_KEYS, errors);

    // `name` and `version` are optional in `luabox.toml`: the project's
    // rockspec is the package manifest luarocks reads and supplies them
    // (SPEC.md §6). A value that *is* written here is still shape-checked;
    // the commands that need a name substitute one instead of demanding it
    // (`luabox-cli::build_cmd::run` falls back to `"bundle"`,
    // `doc_cmd::manifest_facts` to the project directory name).
    let name = get_string(table, "package", "name", false, errors).unwrap_or_default();
    if !name.is_empty()
        && let Some(message) = validate_package_name(&name)
    {
        errors.push(ManifestError::new(message, item_span(table, "name")));
    }

    let version = get_string(table, "package", "version", false, errors).unwrap_or_default();
    if !version.is_empty() && !looks_like_semver(&version) {
        errors.push(ManifestError::new(
            format!(
                "`package.version` \"{version}\" doesn't look like semver (expected X.Y.Z, optional -pre-release/+build)"
            ),
            item_span(table, "version"),
        ));
    }

    let edition = get_string(table, "package", "edition", true, errors).unwrap_or_default();
    if !edition.is_empty() {
        validate_dialect(
            "package.edition",
            &edition,
            item_span(table, "edition"),
            errors,
        );
    }

    let description = get_string(table, "package", "description", false, errors);
    let license = get_string(table, "package", "license", false, errors);
    let lua_versions = get_dialect_array(table, "package", "lua-versions", errors);

    let min_luabox_version = get_string(table, "package", "min-luabox-version", false, errors);
    if let Some(v) = &min_luabox_version
        && !looks_like_semver(v)
    {
        errors.push(ManifestError::new(
            format!("`package.min-luabox-version` \"{v}\" doesn't look like semver"),
            item_span(table, "min-luabox-version"),
        ));
    }

    Package {
        name,
        version,
        edition,
        description,
        license,
        lua_versions,
        min_luabox_version,
    }
}

fn parse_build(root: &Table, edition_fallback: &str, errors: &mut Vec<ManifestError>) -> Build {
    let Some(table) = get_table(root, "build", errors) else {
        return Build {
            target: edition_fallback.to_owned(),
            out: "dist".to_owned(),
            mode: "plain".to_owned(),
            entry: vec![DEFAULT_ENTRY.to_owned()],
            outfile: None,
            bundle: false,
            sourcemap: false,
            minify: false,
        };
    };
    check_unknown_keys(table, "[build] key", BUILD_KEYS, errors);

    let target = get_string(table, "build", "target", false, errors)
        .unwrap_or_else(|| edition_fallback.to_owned());
    if !target.is_empty() {
        validate_dialect("build.target", &target, item_span(table, "target"), errors);
    }
    let out = get_string(table, "build", "out", false, errors).unwrap_or_else(|| "dist".to_owned());

    let mode =
        get_string(table, "build", "mode", false, errors).unwrap_or_else(|| "plain".to_owned());
    if !mode.is_empty() && !ALLOWED_BUNDLE_MODES.contains(&mode.as_str()) {
        errors.push(ManifestError::new(
            format!(
                "invalid build.mode `{mode}` (valid: {})",
                ALLOWED_BUNDLE_MODES.join(", ")
            ),
            item_span(table, "mode"),
        ));
    }

    // `entry` defaults to the conventional single entry point when absent;
    // an explicit empty list stays empty (a library with nothing to bundle).
    let entry = if table.get("entry").is_some() {
        get_string_array(table, "build", "entry", errors)
    } else {
        vec![DEFAULT_ENTRY.to_owned()]
    };
    let outfile = get_string(table, "build", "outfile", false, errors);
    let bundle = get_bool(table, "build", "bundle", false, errors);
    let sourcemap = get_bool(table, "build", "sourcemap", false, errors);
    let minify = get_bool(table, "build", "minify", false, errors);

    Build {
        target,
        out,
        mode,
        entry,
        outfile,
        bundle,
        sourcemap,
        minify,
    }
}

fn parse_types(root: &Table, errors: &mut Vec<ManifestError>) -> Types {
    let Some(table) = get_table(root, "types", errors) else {
        return Types::default();
    };
    check_unknown_keys(table, "[types] key", TYPES_KEYS, errors);

    Types {
        strict: get_bool(table, "types", "strict", false, errors),
        defs: get_string_array(table, "types", "defs", errors),
    }
}

/// Parse `[lint]` (SPEC.md §9). `globals` is a string array; every other key
/// is a level entry (`allow`/`warn`/`deny`) targeting either a tier name
/// ([`LINT_TIERS`]) or a rule id. Rule ids are open (they live in
/// `luabox-lint`), so unknown keys are not rejected here — only the *level
/// value* is validated, with a cargo-style did-you-mean nudge.
fn parse_lint(root: &Table, errors: &mut Vec<ManifestError>) -> Lint {
    let Some(table) = get_table(root, "lint", errors) else {
        return Lint::default();
    };
    let mut lint = Lint::default();
    for (key, item) in table.iter() {
        if key == "globals" {
            lint.globals = get_string_array(table, "lint", "globals", errors);
            continue;
        }
        let Some(raw) = item.as_str() else {
            errors.push(ManifestError::new(
                format!("`lint.{key}` must be a level string (allow, warn, deny)"),
                item.span(),
            ));
            continue;
        };
        let Some(level) = parse_lint_level(raw) else {
            errors.push(ManifestError::unknown_key(
                &format!("lint level for `{key}`"),
                raw,
                LINT_LEVELS,
                item_span(table, key),
            ));
            continue;
        };
        if LINT_TIERS.contains(&key) {
            lint.tiers.insert(key.to_owned(), level);
        } else {
            lint.rules.insert(key.to_owned(), level);
        }
    }
    lint
}

fn parse_lint_level(raw: &str) -> Option<LintLevel> {
    match raw {
        "allow" => Some(LintLevel::Allow),
        "warn" => Some(LintLevel::Warn),
        "deny" => Some(LintLevel::Deny),
        _ => None,
    }
}

fn parse_dependencies(
    root: &Table,
    section: &str,
    errors: &mut Vec<ManifestError>,
) -> BTreeMap<String, Dependency> {
    let Some(table) = get_table(root, section, errors) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (name, item) in table.iter() {
        if let Some(dependency) = parse_dependency(section, name, item, errors) {
            out.insert(name.to_owned(), dependency);
        }
    }
    out
}

fn parse_dependency(
    section: &str,
    name: &str,
    item: &Item,
    errors: &mut Vec<ManifestError>,
) -> Option<Dependency> {
    if let Some(s) = item.as_str() {
        return Some(Dependency::Version(s.to_owned()));
    }

    let Some(table) = item.as_table_like() else {
        errors.push(ManifestError::new(
            format!("`{section}.{name}` must be a version-requirement string or an inline table"),
            item.span(),
        ));
        return None;
    };

    let ctx = format!("{section}.{name}");
    check_unknown_keys(table, &format!("`{ctx}` key"), DEPENDENCY_KEYS, errors);

    let git = get_string(table, &ctx, "git", false, errors);
    let path = get_string(table, &ctx, "path", false, errors);
    let url = get_string(table, &ctx, "url", false, errors);
    let sha256 = get_string(table, &ctx, "sha256", false, errors);
    let version = get_string(table, &ctx, "version", false, errors);
    let rev = get_string(table, &ctx, "rev", false, errors);
    let tag = get_string(table, &ctx, "tag", false, errors);
    let branch = get_string(table, &ctx, "branch", false, errors);

    let kinds_present =
        usize::from(git.is_some()) + usize::from(path.is_some()) + usize::from(url.is_some());
    if kinds_present == 0 {
        // No source key. An orphan source *modifier* names the source it is
        // missing — uniformly, whether or not `version` is also present —
        // before the version-only form is considered.
        if let Some(span) = table.get("sha256").and_then(Item::span) {
            errors.push(ManifestError::new(
                format!("`{ctx}.sha256` is only valid alongside a `url` source"),
                Some(span),
            ));
            return None;
        }
        if let Some(span) = ["rev", "tag", "branch"]
            .iter()
            .find_map(|key| table.get(key).and_then(Item::span))
        {
            errors.push(ManifestError::new(
                format!("`{ctx}` has a git reference key but no `git` source"),
                Some(span),
            ));
            return None;
        }
        // A `version`-only table is the bare-string form spelled longhand
        // (cargo semantics) — `pkg = { version = "1.0" }` ≡ `pkg = "1.0"`.
        if let Some(version) = version {
            return Some(Dependency::Version(version));
        }
        errors.push(ManifestError::new(
            format!("`{ctx}` must specify one of `git`, `path`, `url`, or `version`"),
            item.span(),
        ));
        return None;
    }
    if kinds_present > 1 {
        errors.push(ManifestError::new(
            format!("`{ctx}` must specify only one of `git`, `path`, or `url`"),
            item.span(),
        ));
        return None;
    }

    if let Some(url) = url {
        // Integrity is non-negotiable: an http(s) tarball source must pin its
        // SHA-256, so a `url` with no `sha256` is a hard error, never a silent
        // unverified fetch (SPEC.md §6).
        let Some(sha256) = sha256 else {
            errors.push(ManifestError::new(
                format!(
                    "`{ctx}` has a `url` source but no `sha256` — an http(s) tarball dependency \
                     must pin its SHA-256 digest for integrity"
                ),
                item.span(),
            ));
            return None;
        };
        return Some(Dependency::Url(UrlDependency {
            url,
            sha256,
            version,
        }));
    }
    if let Some(span) = table.get("sha256").and_then(Item::span) {
        // A `sha256` with no `url` is meaningless — flag it rather than
        // silently ignore it.
        errors.push(ManifestError::new(
            format!("`{ctx}.sha256` is only valid alongside a `url` source"),
            Some(span),
        ));
    }

    if let Some(git) = git {
        let reference_count = [rev.is_some(), tag.is_some(), branch.is_some()]
            .into_iter()
            .filter(|b| *b)
            .count();
        if reference_count > 1 {
            errors.push(ManifestError::new(
                format!("`{ctx}` must specify at most one of `rev`, `tag`, `branch`"),
                item.span(),
            ));
        }
        return Some(Dependency::Git(GitDependency {
            git,
            rev,
            tag,
            branch,
            version,
        }));
    }

    // kinds_present == 1 and it wasn't url or git, so it must be path.
    path.map(|path| Dependency::Path(PathDependency { path, version }))
}
