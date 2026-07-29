//! `luabox.toml` parsing and validation (SPEC.md §5).
//!
//! All errors are collected — a single [`Manifest::parse`] call reports every
//! problem in the file, not just the first (cargo/rustc-style batch
//! diagnostics, SPEC.md §14).
//!
//! Validation stays syntax-free: `edition` and `build.target` are parsed into
//! the local [`DialectId`] vocabulary, never by parsing Lua — Distribution
//! "never parses syntax" (SPEC.md §16), so this crate has no dependency on
//! `luabox-syntax` and never will.

use std::collections::BTreeMap;
use std::ops::Range;

use toml_edit::{ImDocument, Item, Table, TableLike};

use crate::error::ManifestError;
use crate::model::{
    Build, BundleMode, DEFAULT_ENTRY, DEFAULT_OUT, Dependency, DialectId, GitDependency, Lint,
    LintLevel, LintTier, Manifest, Package, PathDependency, Types, UrlDependency,
};

// The key allow-lists below are `pub(crate)` for one reason: they are the
// anchors the JSON Schema parity suite ([`crate::schema`]) compares the
// published schema's `properties` objects against, so the two descriptions of
// the manifest contract cannot drift apart.
pub(crate) const TOP_LEVEL_KEYS: &[&str] = &[
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
pub(crate) const PACKAGE_KEYS: &[&str] = &[
    "name",
    "version",
    "edition",
    "description",
    "license",
    "lua-versions",
    "min-luabox-version",
];
pub(crate) const BUILD_KEYS: &[&str] = &[
    "target",
    "out",
    "mode",
    "entry",
    "outfile",
    "bundle",
    "sourcemap",
    "minify",
];
pub(crate) const TYPES_KEYS: &[&str] = &["strict", "defs"];
/// The [`DialectId`] a `Package` carries while its `edition` is missing or
/// invalid. Never observable: `Manifest::parse` has pushed an error by the
/// time it is used, so it returns `Err` and the `Package` is dropped.
const EDITION_PLACEHOLDER: DialectId = DialectId::Lua54;
pub(crate) const DEPENDENCY_KEYS: &[&str] = &[
    "git", "rev", "tag", "branch", "path", "url", "sha256", "version",
];

impl Manifest {
    /// Parse and validate a `luabox.toml` document.
    ///
    /// Collects *every* validation error rather than stopping at the first;
    /// on success, the returned [`Manifest`] retains `text` verbatim, so it
    /// renders back byte-identically (`Display`), comments and all.
    pub fn parse(text: &str) -> Result<Manifest, Vec<ManifestError>> {
        // Parsed as an `ImDocument` (not `toml_edit::DocumentMut`):
        // `DocumentMut::from_str` despans on construction, so item/key spans
        // — needed for the span-rich errors below — would already be gone by
        // the time validation runs.
        let im_document = match ImDocument::<String>::parse(text.to_owned()) {
            Ok(document) => document,
            Err(error) => return Err(vec![ManifestError::new(error.to_string(), error.span())]),
        };

        let mut errors = Vec::new();
        let root = im_document.as_table();
        check_top_level_keys(root, &mut errors);

        let package = parse_package(root, &mut errors);
        let build = parse_build(root, package.edition, &mut errors);
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
                source: text.to_owned(),
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

/// Like [`get_string_array`] but each entry is parsed as a [`DialectId`]
/// (SPEC.md §2, §6); an entry that is not a known dialect is reported and
/// dropped.
fn get_dialect_array(
    table: &dyn TableLike,
    ctx: &str,
    key: &str,
    errors: &mut Vec<ManifestError>,
) -> Vec<DialectId> {
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
            Some(s) => out.extend(parse_closed::<DialectId>(
                &format!("{ctx}.{key} entry"),
                s,
                value.span(),
                errors,
            )),
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

/// Parse one closed-vocabulary value ([`DialectId`], [`BundleMode`], …),
/// recording an `invalid <what> …` error naming the accepted set when the
/// spelling is not one the vocabulary accepts.
///
/// This is the single place a `[package]`/`[build]` enum field is validated:
/// the typed model can therefore only ever carry accepted values, which is
/// what lets frontends map the field inward with an exhaustive match instead
/// of re-checking a string that already passed here.
fn parse_closed<T>(
    what: &str,
    value: &str,
    span: Option<Range<usize>>,
    errors: &mut Vec<ManifestError>,
) -> Option<T>
where
    T: std::str::FromStr<Err = crate::model::UnknownValue>,
{
    match value.parse::<T>() {
        Ok(parsed) => Some(parsed),
        Err(unknown) => {
            errors.push(ManifestError::new(
                format!("invalid {what} {unknown}"),
                span,
            ));
            None
        }
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
            edition: EDITION_PLACEHOLDER,
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

    // `edition` is required, so the placeholder below is only ever the value
    // of a `Package` that is discarded: `Manifest::parse` returns `Err` with
    // the missing/invalid-key error whenever this branch is taken.
    let edition = get_string(table, "package", "edition", true, errors)
        .and_then(|raw| {
            parse_closed::<DialectId>("package.edition", &raw, item_span(table, "edition"), errors)
        })
        .unwrap_or(EDITION_PLACEHOLDER);

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

fn parse_build(
    root: &Table,
    edition_fallback: DialectId,
    errors: &mut Vec<ManifestError>,
) -> Build {
    let Some(table) = get_table(root, "build", errors) else {
        return Build::defaults(edition_fallback);
    };
    check_unknown_keys(table, "[build] key", BUILD_KEYS, errors);

    let target = get_string(table, "build", "target", false, errors)
        .map_or(Some(edition_fallback), |raw| {
            parse_closed::<DialectId>("build.target", &raw, item_span(table, "target"), errors)
        })
        .unwrap_or(edition_fallback);
    let out =
        get_string(table, "build", "out", false, errors).unwrap_or_else(|| DEFAULT_OUT.to_owned());

    let mode = get_string(table, "build", "mode", false, errors)
        .map_or(Some(BundleMode::default()), |raw| {
            parse_closed::<BundleMode>("build.mode", &raw, item_span(table, "mode"), errors)
        })
        .unwrap_or_default();

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
/// ([`LintTier`]) or a rule id. Rule ids are open (they live in
/// `luabox-lint`), so unknown keys are not rejected here — only the *level
/// value* is validated, with a cargo-style did-you-mean nudge. Tier names are
/// closed, so [`Lint::tiers`] is keyed by the typed [`LintTier`] and can never
/// hand a consumer a tier it does not know.
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
        let Ok(level) = raw.parse::<LintLevel>() else {
            errors.push(ManifestError::unknown_key(
                &format!("lint level for `{key}`"),
                raw,
                LintLevel::NAMES,
                item_span(table, key),
            ));
            continue;
        };
        match key.parse::<LintTier>() {
            Ok(tier) => {
                lint.tiers.insert(tier, level);
            }
            Err(_) => {
                lint.rules.insert(key.to_owned(), level);
            }
        }
    }
    lint
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
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;

    /// Extracts the fenced `toml` code block under "## 5. Project manifest"
    /// from the repo's own SPEC.md, so the happy-path test always exercises
    /// the spec's actual example — no hand-transcription to drift out of
    /// sync.
    fn spec_manifest_example() -> String {
        // Normalize CRLF first: a Windows checkout with core.autocrlf=true
        // (GitHub's windows runners) hands include_str! CRLF text, and the
        // exact "```toml\n" fence match would miss the trailing \r.
        let spec = include_str!("../../../SPEC.md").replace("\r\n", "\n");
        let heading = spec
            .find("## 5. Project manifest")
            .expect("SPEC.md §5 heading present");
        let after_heading = &spec[heading..];
        let fence = "```toml\n";
        let body_start = after_heading
            .find(fence)
            .expect("SPEC.md §5 has a fenced toml block")
            + fence.len();
        let body = &after_heading[body_start..];
        let body_end = body.find("```").expect("SPEC.md §5 fenced block is closed");
        body[..body_end].to_owned()
    }

    #[test]
    fn parses_the_spec_example_verbatim() {
        let source = spec_manifest_example();
        let manifest = Manifest::parse(&source).expect("SPEC.md §5 example must parse");

        assert_eq!(manifest.package.name, "my-lib");
        assert_eq!(manifest.package.version, "1.2.0");
        assert_eq!(manifest.package.edition, DialectId::Lua54);
        assert_eq!(manifest.package.license.as_deref(), Some("MIT"));
        assert_eq!(manifest.package.description, None);

        assert_eq!(manifest.build.target, DialectId::Lua51);
        assert_eq!(manifest.build.out, "dist");

        assert!(manifest.types.strict);
        assert_eq!(manifest.types.defs, vec!["love2d".to_owned()]);

        // The example's optional tables (e.g. `[dependencies]`) are asserted
        // by the focused tests below against fixtures this file owns;
        // asserting their *contents* here would pin the test to SPEC.md's
        // illustrative values. What this test owns is that the spec's own
        // example parses at all, and round-trips.
        assert_eq!(manifest.to_string(), source);
    }

    #[test]
    fn minimal_manifest_applies_defaults() {
        let manifest =
            Manifest::parse("[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"5.1\"\n")
                .expect("minimal manifest is valid");

        assert_eq!(
            manifest.build.target,
            DialectId::Lua51,
            "target defaults to edition"
        );
        assert_eq!(manifest.build.out, "dist");
        assert_eq!(
            manifest.build.mode,
            BundleMode::Plain,
            "mode defaults to plain"
        );
        assert_eq!(
            manifest.build.entry,
            vec!["src/main.lua".to_owned()],
            "entry defaults to the conventional single entry point"
        );
        assert_eq!(manifest.build.outfile, None);
        assert!(!manifest.build.bundle, "bundle defaults to false");
        assert!(!manifest.build.sourcemap, "sourcemap defaults to false");
        assert!(!manifest.build.minify, "minify defaults to false");
        assert!(!manifest.types.strict);
        assert!(manifest.types.defs.is_empty());
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.dev_dependencies.is_empty());
    }

    #[test]
    fn missing_package_table_is_an_error() {
        let errors = Manifest::parse("[build]\ntarget = \"5.1\"\n").unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("[package]")));
    }

    #[test]
    fn missing_edition_is_reported_but_name_version_are_optional() {
        // `edition` is a tool concern and stays required; `name`/`version`
        // are optional in `luabox.toml` because the rockspec luarocks reads
        // supplies them (SPEC.md §6).
        let errors = Manifest::parse("[package]\nlicense = \"MIT\"\n").unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("package.edition")));
        assert!(
            !errors.iter().any(|e| e.message.contains("package.name")),
            "name is optional now"
        );
        assert!(
            !errors.iter().any(|e| e.message.contains("package.version")),
            "version is optional now"
        );
    }

    #[test]
    fn package_without_name_or_version_parses() {
        // The slimmed `luabox.toml` scaffold: edition only; the rockspec owns
        // the rest.
        let manifest =
            Manifest::parse("[package]\nedition = \"5.4\"\n").expect("edition-only manifest valid");
        assert!(manifest.package.name.is_empty());
        assert!(manifest.package.version.is_empty());
        assert_eq!(manifest.package.edition, DialectId::Lua54);
    }

    #[test]
    fn collects_multiple_unrelated_errors_in_one_pass() {
        let src = "[package]\nname = \"Bad Name!\"\nversion = \"not-a-version\"\nedition = \"5.9\"\n\n[unknown-table]\nx = 1\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.len() >= 4, "expected >=4 errors, got {errors:#?}");
        assert!(errors.iter().any(|e| e.message.contains("package.name")));
        assert!(errors.iter().any(|e| e.message.contains("semver")));
        assert!(errors.iter().any(|e| e.message.contains("edition")));
        assert!(errors.iter().any(|e| e.message.contains("unknown-table")));
    }

    #[test]
    fn unknown_top_level_key_suggests_fix() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[depndencies]\nx = \"1\"\n",
        )
        .unwrap_err();
        let msg = errors
            .iter()
            .find(|e| e.message.contains("depndencies"))
            .expect("unknown key error present")
            .message
            .clone();
        assert!(msg.contains("did you mean `dependencies`"), "{msg}");
    }

    #[test]
    fn unknown_package_key_is_an_error() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\ntypo-key = \"x\"\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("typo-key")));
    }

    #[test]
    fn invalid_edition_lists_allowed_set() {
        let errors =
            Manifest::parse("[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.9\"\n")
                .unwrap_err();
        let msg = &errors[0].message;
        for dialect in DialectId::NAMES {
            assert!(msg.contains(dialect), "{msg} should mention {dialect}");
        }
    }

    #[test]
    fn invalid_build_target_is_reported() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[build]\ntarget = \"6.0\"\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("build.target")));
    }

    #[test]
    fn build_mode_accepts_love_and_nvim_plugin() {
        for mode in ["love", "nvim-plugin"] {
            let manifest = Manifest::parse(&format!(
                "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[build]\nmode = \"{mode}\"\n"
            ))
            .unwrap_or_else(|e| panic!("mode `{mode}` should be valid: {e:?}"));
            assert_eq!(manifest.build.mode.as_str(), mode);
        }
    }

    #[test]
    fn invalid_build_mode_lists_allowed_set() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[build]\nmode = \"roblox\"\n",
        )
        .unwrap_err();
        let msg = errors
            .iter()
            .find(|e| e.message.contains("build.mode"))
            .expect("build.mode error present");
        for mode in BundleMode::NAMES {
            assert!(
                msg.message.contains(mode),
                "{} should mention {mode}",
                msg.message
            );
        }
    }

    #[test]
    fn build_parses_the_unified_emit_fields() {
        let manifest = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n\
             [build]\ntarget = \"5.1\"\nentry = [\"src/a.lua\", \"src/b.lua\"]\n\
             bundle = true\nsourcemap = true\nminify = true\n",
        )
        .expect("valid [build]");
        assert_eq!(
            manifest.build.entry,
            vec!["src/a.lua".to_owned(), "src/b.lua".to_owned()]
        );
        assert!(manifest.build.bundle);
        assert!(manifest.build.sourcemap);
        assert!(manifest.build.minify);
        assert_eq!(manifest.build.outfile, None);
    }

    #[test]
    fn build_parses_outfile() {
        let manifest = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n\
             [build]\nbundle = true\noutfile = \"dist/app.lua\"\n",
        )
        .expect("valid [build]");
        assert_eq!(manifest.build.outfile.as_deref(), Some("dist/app.lua"));
    }

    #[test]
    fn build_rejects_non_bool_bundle() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n\
             [build]\nbundle = \"yes\"\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("build.bundle")));
    }

    #[test]
    fn build_rejects_unknown_key() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n\
             [build]\nentrypoint = \"src/main.lua\"\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("entrypoint")));
    }

    #[test]
    fn package_name_must_not_start_with_digit() {
        let errors =
            Manifest::parse("[package]\nname = \"1lib\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n")
                .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("start with a digit"))
        );
    }

    #[test]
    fn package_name_rejects_uppercase_and_underscore() {
        let errors = Manifest::parse(
            "[package]\nname = \"My_Lib\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("lowercase")));
    }

    #[test]
    fn package_name_allows_dash_and_digits_not_leading() {
        let manifest = Manifest::parse(
            "[package]\nname = \"my-lib2\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
        )
        .expect("valid name");
        assert_eq!(manifest.package.name, "my-lib2");
    }

    #[test]
    fn version_must_look_like_semver() {
        let errors =
            Manifest::parse("[package]\nname = \"ok\"\nversion = \"1.0\"\nedition = \"5.4\"\n")
                .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("semver")));
    }

    #[test]
    fn version_with_prerelease_and_build_metadata_is_accepted() {
        let manifest = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.2.3-beta.1+build.5\"\nedition = \"5.4\"\n",
        )
        .expect("pre-release/build semver accepted");
        assert_eq!(manifest.package.version, "1.2.3-beta.1+build.5");
    }

    #[test]
    fn lua_versions_and_min_luabox_version_parse() {
        let manifest = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\nlua-versions = [\"5.1\", \"5.4\"]\nmin-luabox-version = \"0.3.0\"\n",
        )
        .expect("valid manifest");
        assert_eq!(
            manifest.package.lua_versions,
            vec![DialectId::Lua51, DialectId::Lua54]
        );
        assert_eq!(
            manifest.package.min_luabox_version.as_deref(),
            Some("0.3.0")
        );
    }

    #[test]
    fn invalid_lua_versions_entry_is_reported() {
        let errors = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\nlua-versions = [\"5.4\", \"6.0\"]\n",
        )
        .unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("lua-versions")));
    }

    #[test]
    fn dependency_forms_all_parse() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\na = \"1.0\"\nb = { git = \"https://example/b\", tag = \"v1\" }\nc = { git = \"https://example/c\", branch = \"main\" }\nd = { path = \"../d\" }\n";
        let manifest = Manifest::parse(src).expect("valid manifest");

        assert_eq!(
            manifest.dependencies.get("a"),
            Some(&Dependency::Version("1.0".to_owned()))
        );
        match manifest.dependencies.get("b") {
            Some(Dependency::Git(git)) => assert_eq!(git.tag.as_deref(), Some("v1")),
            other => panic!("expected git dep with tag, got {other:?}"),
        }
        match manifest.dependencies.get("c") {
            Some(Dependency::Git(git)) => assert_eq!(git.branch.as_deref(), Some("main")),
            other => panic!("expected git dep with branch, got {other:?}"),
        }
        match manifest.dependencies.get("d") {
            Some(Dependency::Path(p)) => assert_eq!(p.path, "../d"),
            other => panic!("expected path dep, got {other:?}"),
        }
    }

    #[test]
    fn removed_workspace_dependency_form_is_an_error() {
        // `{ workspace = true }` deps died with `[workspace]` (#18).
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\ne = { workspace = true }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("`workspace`")));
    }

    #[test]
    fn removed_tasks_and_workspace_tables_are_unknown_table_errors() {
        // Dropped in 0.2.0 (#18): both only served removed subsystems, so
        // they get the standard unknown-table error, not parse-but-ignore —
        // with the removal named, since a 0.1.4 manifest carries them across
        // the upgrade and "unknown" alone reads as a typo.
        for table in ["tasks", "workspace"] {
            let src = format!("{PREAMBLE}\n[{table}]\nx = \"y\"\n");
            let errors = Manifest::parse(&src).unwrap_err();
            let error = errors
                .iter()
                .find(|e| e.message.contains(table))
                .unwrap_or_else(|| {
                    panic!("[{table}] should be an unknown-table error, got {errors:?}")
                });
            assert_eq!(
                error.message,
                format!(
                    "unknown top-level table `{table}` (valid: package, build, types, \
                     dependencies, dev-dependencies, lint) — removed in 0.2.0, see CHANGELOG.md"
                )
            );
        }
    }

    #[test]
    fn an_unknown_top_level_table_that_was_never_real_gets_no_removal_note() {
        // The nudge is for exactly the two tables 0.2.0 dropped; a plain typo
        // must not be told it used to work.
        let errors = Manifest::parse(&format!("{PREAMBLE}\n[typo]\nx = \"y\"\n")).unwrap_err();
        let error = errors
            .iter()
            .find(|e| e.message.contains("typo"))
            .unwrap_or_else(|| panic!("expected an unknown-table error, got {errors:?}"));
        assert!(!error.message.contains("removed in"), "{error:?}");
        assert!(error.message.contains("did you mean `types`?"), "{error:?}");
    }

    #[test]
    fn url_dependency_parses_with_its_digest() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nu = { url = \"https://example.com/u.tar.gz\", sha256 = \"abc123\" }\n";
        let manifest = Manifest::parse(src).expect("valid manifest");
        match manifest.dependencies.get("u") {
            Some(Dependency::Url(url)) => {
                assert_eq!(url.url, "https://example.com/u.tar.gz");
                assert_eq!(url.sha256, "abc123");
                assert_eq!(url.version, None);
            }
            other => panic!("expected url dep, got {other:?}"),
        }
    }

    #[test]
    fn url_dependency_without_sha256_is_rejected() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nu = { url = \"https://example.com/u.tar.gz\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("sha256")),
            "a url source with no sha256 must be a hard error: {errors:?}"
        );
    }

    #[test]
    fn sha256_without_a_url_source_is_rejected() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { path = \"../d\", sha256 = \"abc\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(
            errors.iter().any(|e| e.message.contains("sha256")),
            "a lone sha256 must be flagged: {errors:?}"
        );
    }

    #[test]
    fn dependency_table_needs_a_source_or_a_version() {
        // An empty inline table names nothing at all.
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = {}\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("must specify one of"))
        );
    }

    #[test]
    fn version_alongside_a_source_is_recorded_not_dropped() {
        // #29: the deliberate rule — `version` next to a source key is the
        // declared version expectation for that sourced package, kept on
        // the model (reserved for post-v1 resolution), exactly as spelled.
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\ng = { git = \"https://x\", version = \"2.0\" }\np = { path = \"../p\", version = \"3.0\" }\nu = { url = \"https://x/u.tar.gz\", sha256 = \"abc\", version = \"4.0\" }\n";
        let manifest = Manifest::parse(src).expect("valid manifest");
        match manifest.dependencies.get("g") {
            Some(Dependency::Git(d)) => assert_eq!(d.version.as_deref(), Some("2.0")),
            other => panic!("expected git dep, got {other:?}"),
        }
        match manifest.dependencies.get("p") {
            Some(Dependency::Path(d)) => assert_eq!(d.version.as_deref(), Some("3.0")),
            other => panic!("expected path dep, got {other:?}"),
        }
        match manifest.dependencies.get("u") {
            Some(Dependency::Url(d)) => assert_eq!(d.version.as_deref(), Some("4.0")),
            other => panic!("expected url dep, got {other:?}"),
        }
    }

    #[test]
    fn version_only_table_is_the_bare_string_form_spelled_longhand() {
        // #23: `version` is a valid dependency key, so a version-only table
        // must mean the same thing as `pkg = "1.0"` — not an error that
        // contradicts the valid-key list.
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\npkg = { version = \"1.0\" }\n";
        let manifest = Manifest::parse(src).expect("version-only table is valid");
        assert_eq!(
            manifest.dependencies.get("pkg"),
            Some(&Dependency::Version("1.0".to_owned()))
        );
    }

    #[test]
    fn orphan_source_modifiers_name_the_missing_source() {
        // A git reference or digest without its source names the source it
        // is missing — uniformly, with or without `version` present.
        for extra in ["", "version = \"1.0\", "] {
            let src = format!(
                "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = {{ {extra}tag = \"v1\" }}\n"
            );
            let errors = Manifest::parse(&src).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("git reference key but no `git` source")),
                "extra={extra:?}: {errors:?}"
            );

            let src = format!(
                "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = {{ {extra}sha256 = \"abc\" }}\n"
            );
            let errors = Manifest::parse(&src).unwrap_err();
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("only valid alongside a `url` source")),
                "extra={extra:?}: {errors:?}"
            );
        }
    }

    #[test]
    fn dependency_table_rejects_multiple_kinds() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", path = \"../y\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("only one of")));
    }

    #[test]
    fn dependency_table_rejects_multiple_ref_kinds() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", rev = \"a\", tag = \"b\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("at most one of")));
    }

    #[test]
    fn unknown_dependency_table_key_is_reported() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", ref = \"a\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("`ref`")));
    }

    #[test]
    fn parse_errors_carry_spans_where_available() {
        let errors =
            Manifest::parse("[package]\nname = \"1bad\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n")
                .unwrap_err();
        let name_error = errors
            .iter()
            .find(|e| e.message.contains("start with a digit"))
            .expect("error present");
        assert!(name_error.span.is_some());
    }

    #[test]
    fn malformed_toml_reports_a_parse_error() {
        let errors = Manifest::parse("[package\nname = \"ok\"\n").unwrap_err();
        assert!(!errors.is_empty());
    }

    const PREAMBLE: &str = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n";

    #[test]
    fn lint_defaults_to_empty() {
        let manifest = Manifest::parse(PREAMBLE).expect("valid manifest");
        assert!(manifest.lint.globals.is_empty());
        assert!(manifest.lint.tiers.is_empty());
        assert!(manifest.lint.rules.is_empty());
    }

    #[test]
    fn lint_parses_globals_tiers_and_rules() {
        let src = format!(
            "{PREAMBLE}\n[lint]\nglobals = [\"vim\", \"love\"]\npedantic = \"warn\"\nunused-local = \"allow\"\nglobal-write = \"deny\"\n"
        );
        let manifest = Manifest::parse(&src).expect("valid [lint]");
        assert_eq!(manifest.lint.globals, vec!["vim", "love"]);
        assert_eq!(
            manifest.lint.tiers.get(&LintTier::Pedantic),
            Some(&LintLevel::Warn)
        );
        assert_eq!(
            manifest.lint.rules.get("unused-local"),
            Some(&LintLevel::Allow)
        );
        assert_eq!(
            manifest.lint.rules.get("global-write"),
            Some(&LintLevel::Deny)
        );
        // A tier name is classified as a tier, not a rule.
        assert!(!manifest.lint.rules.contains_key("pedantic"));
    }

    #[test]
    fn lint_bad_level_suggests_valid_levels() {
        let src = format!("{PREAMBLE}\n[lint]\nunused-local = \"alow\"\n");
        let errors = Manifest::parse(&src).unwrap_err();
        let msg = &errors[0].message;
        assert!(msg.contains("did you mean `allow`"), "{msg}");
    }

    #[test]
    fn lint_non_string_level_is_reported() {
        let src = format!("{PREAMBLE}\n[lint]\nunused-local = 3\n");
        let errors = Manifest::parse(&src).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("must be a level string"))
        );
    }

    // --- shape errors ------------------------------------------------------

    /// Every error message produced by parsing `src`, for substring assertions.
    fn errors_for(src: &str) -> Vec<String> {
        let errors = Manifest::parse(src).unwrap_err();
        errors.iter().map(|e| e.message.clone()).collect()
    }

    fn assert_reports(src: &str, needle: &str) {
        let messages = errors_for(src);
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected an error containing {needle:?}, got {messages:?}"
        );
    }

    #[test]
    fn a_section_that_is_not_a_table_is_reported_per_section() {
        for section in ["build", "types", "lint", "dependencies"] {
            // The scalar must precede `[package]` to stay a top-level key.
            let src = format!("{section} = 5\n{PREAMBLE}");
            assert_reports(&src, &format!("`[{section}]` must be a table"));
        }
    }

    #[test]
    fn a_non_string_scalar_field_names_the_field_it_belongs_to() {
        assert_reports(
            &format!("{PREAMBLE}\n[build]\nout = 5\n"),
            "`build.out` must be a string",
        );
        assert_reports(
            "[package]\nname = 1\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
            "`package.name` must be a string",
        );
    }

    #[test]
    fn a_string_array_field_rejects_a_bare_string_and_non_string_entries() {
        assert_reports(
            &format!("{PREAMBLE}\n[types]\ndefs = \"lib.d.lua\"\n"),
            "`types.defs` must be an array of strings",
        );
        assert_reports(
            &format!("{PREAMBLE}\n[types]\ndefs = [\"a.d.lua\", 3]\n"),
            "`types.defs` entries must be strings",
        );
    }

    #[test]
    fn lua_versions_rejects_a_bare_string_and_non_string_entries() {
        assert_reports(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\nlua-versions = \"5.1\"\n",
            "`package.lua-versions` must be an array of strings",
        );
        assert_reports(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\nlua-versions = [\"5.1\", 51]\n",
            "`package.lua-versions` entries must be strings",
        );
    }

    // --- package names -----------------------------------------------------

    #[test]
    fn an_empty_package_name_is_treated_as_absent_not_invalid() {
        // `name` is optional (the rockspec supplies it), and `parse_package`
        // guards the name rules with `!name.is_empty()` — so an explicit empty
        // string parses and simply carries no name.
        let manifest =
            Manifest::parse("[package]\nname = \"\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n")
                .expect("an empty name is not a validation failure");
        assert_eq!(manifest.package.name, "");
    }

    #[test]
    fn scoped_package_names_parse_and_validate_both_segments() {
        let ok = Manifest::parse(
            "[package]\nname = \"@acme/widgets\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
        )
        .expect("a well-formed scoped name is valid");
        assert_eq!(ok.package.name, "@acme/widgets");

        // `@` with no `/` is not a scoped name.
        assert_reports(
            "[package]\nname = \"@acme\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
            "is scoped but not of the form `@scope/name`",
        );
        // Either segment may be empty, and either is caught.
        assert_reports(
            "[package]\nname = \"@/widgets\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
            "must not have an empty scope or name segment",
        );
        assert_reports(
            "[package]\nname = \"@acme/\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
            "must not have an empty scope or name segment",
        );
        // The per-segment rules still apply inside a scope.
        assert_reports(
            "[package]\nname = \"@1acme/widgets\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
            "must not start with a digit",
        );
    }

    #[test]
    fn min_luabox_version_must_look_like_semver() {
        assert_reports(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\nmin-luabox-version = \"latest\"\n",
            "`package.min-luabox-version` \"latest\" doesn't look like semver",
        );
    }

    // --- dependencies ------------------------------------------------------

    #[test]
    fn a_dependency_that_is_neither_string_nor_table_is_reported() {
        assert_reports(
            &format!("{PREAMBLE}\n[dependencies]\na = 3\n"),
            "`dependencies.a` must be a version-requirement string or an inline table",
        );
        assert_reports(
            &format!("{PREAMBLE}\n[dev-dependencies]\nb = true\n"),
            "`dev-dependencies.b` must be a version-requirement string or an inline table",
        );
    }

    // --- lossless round-trip -----------------------------------------------

    #[test]
    fn a_parsed_manifest_renders_back_byte_identically() {
        let src = format!(
            "# leading comment\n{PREAMBLE}\n# deps!\n[dependencies]\ngreet = {{ path = \"../greet\" }}\n"
        );
        let manifest = Manifest::parse(&src).expect("valid manifest");

        // `Display` renders the source text the manifest was parsed from —
        // comments, key order and formatting intact.
        assert_eq!(manifest.to_string(), src);
    }
}
