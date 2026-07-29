//! The published JSON Schema for `luabox.toml` (draft 2020-12).
//!
//! The manifest parser is hand-rolled ([`crate::model::Manifest::parse`]) so
//! it can collect *every* error with a span and a did-you-mean nudge — things
//! a schema validator cannot give. That leaves the contract itself written
//! twice: once as Rust, once as the schema editors, validators and LLM coding
//! assistants read (`luabox schema`).
//!
//! Two copies of a contract drift. The test suite at the bottom of this file
//! is what makes that impossible: it asserts *structural parity* between the
//! schema document and the parser's own key allow-lists, closed vocabularies
//! and required keys, and then validates a corpus of real manifests — every
//! `examples/*/luabox.toml` in the repo plus curated valid/invalid fixtures —
//! through **both**, requiring the same verdict from each.

/// The complete JSON Schema (draft 2020-12) describing `luabox.toml`.
///
/// Embedded verbatim from `schema/luabox.schema.json` so the file that ships
/// in the repository — and the one the `$id` URL serves — is byte-for-byte
/// what the binary prints.
const SCHEMA: &str = include_str!("../schema/luabox.schema.json");

/// The complete JSON Schema (draft 2020-12) for `luabox.toml`, as JSON text.
///
/// This is what `luabox schema` writes to stdout: point an editor, a
/// validator, or an LLM coding assistant at it and it has the whole manifest
/// contract — every table, key, default, enum and mutually-exclusive
/// dependency form — with a prose description on each.
///
/// The schema describes the manifest's *data model*. Manifests are written in
/// TOML; tooling maps TOML to JSON with the standard mapping and validates
/// that.
///
/// # Examples
///
/// ```
/// let schema = luabox_manifest::schema::json_schema();
/// assert!(schema.contains("\"$schema\""));
/// ```
#[must_use]
pub fn json_schema() -> &'static str {
    SCHEMA
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use super::json_schema;
    use crate::model::{BundleMode, DialectId, LintLevel, LintTier, Manifest};
    use crate::parse::{BUILD_KEYS, DEPENDENCY_KEYS, PACKAGE_KEYS, TOP_LEVEL_KEYS, TYPES_KEYS};

    // -----------------------------------------------------------------
    // helpers
    // -----------------------------------------------------------------

    fn schema() -> Value {
        serde_json::from_str(json_schema()).expect("the embedded schema is valid JSON")
    }

    /// The schema at `pointer`, or a panic naming the pointer that is missing
    /// — a parity failure must say *where*, not just that something differs.
    fn at<'a>(root: &'a Value, pointer: &str) -> &'a Value {
        root.pointer(pointer)
            .unwrap_or_else(|| panic!("schema has no `{pointer}`"))
    }

    /// The property names declared at `pointer` (which must be a `properties`
    /// object), as a set.
    fn property_names(root: &Value, pointer: &str) -> BTreeSet<String> {
        at(root, pointer)
            .as_object()
            .unwrap_or_else(|| panic!("`{pointer}` is not an object"))
            .keys()
            .cloned()
            .collect()
    }

    /// The `enum` spellings declared at `pointer`, in schema order.
    fn enum_values(root: &Value, pointer: &str) -> Vec<String> {
        at(root, pointer)
            .as_array()
            .unwrap_or_else(|| panic!("`{pointer}` is not an array"))
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("`{pointer}` entries must be strings"))
                    .to_owned()
            })
            .collect()
    }

    fn set_of(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// A compiled draft 2020-12 validator over the embedded schema.
    fn validator() -> jsonschema::Validator {
        jsonschema::draft202012::new(&schema()).expect("the schema compiles as draft 2020-12")
    }

    /// The standard TOML→JSON projection a validator would apply before
    /// checking a manifest against the schema.
    fn toml_to_json(text: &str) -> Value {
        toml_edit::de::from_str(text).expect("corpus manifests are well-formed TOML")
    }

    /// Every error the schema reports for `text`, as strings.
    fn schema_errors(validator: &jsonschema::Validator, text: &str) -> Vec<String> {
        let instance = toml_to_json(text);
        validator
            .iter_errors(&instance)
            .map(|error| error.to_string())
            .collect()
    }

    // -----------------------------------------------------------------
    // the document itself
    // -----------------------------------------------------------------

    #[test]
    fn the_schema_is_a_valid_draft_2020_12_document() {
        let schema = schema();
        assert_eq!(
            schema["$schema"], "https://json-schema.org/draft/2020-12/schema",
            "the schema must declare draft 2020-12"
        );
        assert!(
            jsonschema::meta::is_valid(&schema),
            "the schema does not satisfy the draft 2020-12 meta-schema: {:?}",
            jsonschema::meta::validate(&schema)
                .err()
                .map(|e| e.to_string())
        );
        // Compiling it is the stronger claim: every `$ref` resolves.
        let _validator = validator();
    }

    #[test]
    fn the_schema_identifies_itself_by_its_canonical_url() {
        let schema = schema();
        assert_eq!(
            schema["$id"],
            "https://raw.githubusercontent.com/flying-dice/luabox/main/crates/luabox-manifest/schema/luabox.schema.json",
            "the `$id` must be the canonical raw URL of this file on `main`"
        );
        assert!(schema["title"].as_str().is_some_and(|t| !t.is_empty()));
        // The TOML→JSON mapping caveat is part of the contract: a reader who
        // only ever sees the schema has to be told the file is TOML.
        let description = schema["description"].as_str().unwrap_or_default();
        assert!(description.contains("TOML"), "{description}");
    }

    /// Every entry of every `properties` object, and every `$defs` entry,
    /// carries a non-empty `description` — the schema has to be readable on
    /// its own, because that is the whole point of publishing it.
    #[test]
    fn every_property_and_definition_is_described() {
        fn walk(node: &Value, path: &str, missing: &mut Vec<String>) {
            let Some(object) = node.as_object() else {
                if let Some(array) = node.as_array() {
                    for (index, item) in array.iter().enumerate() {
                        walk(item, &format!("{path}/{index}"), missing);
                    }
                }
                return;
            };
            for (key, value) in object {
                let child = format!("{path}/{key}");
                if (key == "properties" || key == "$defs")
                    && let Some(entries) = value.as_object()
                {
                    for (name, entry) in entries {
                        let described = entry
                            .get("description")
                            .and_then(Value::as_str)
                            .is_some_and(|d| !d.trim().is_empty());
                        if !described {
                            missing.push(format!("{child}/{name}"));
                        }
                    }
                }
                walk(value, &child, missing);
            }
        }

        let mut missing = Vec::new();
        walk(&schema(), "", &mut missing);
        assert!(
            missing.is_empty(),
            "these schema members have no description: {missing:#?}"
        );
    }

    // -----------------------------------------------------------------
    // structural parity with the parser
    // -----------------------------------------------------------------

    #[test]
    fn the_top_level_tables_are_exactly_the_ones_the_parser_accepts() {
        assert_eq!(
            property_names(&schema(), "/properties"),
            set_of(TOP_LEVEL_KEYS)
        );
    }

    #[test]
    fn the_package_build_and_types_keys_are_exactly_the_ones_the_parser_accepts() {
        let schema = schema();
        assert_eq!(
            property_names(&schema, "/$defs/package/properties"),
            set_of(PACKAGE_KEYS),
            "[package]"
        );
        assert_eq!(
            property_names(&schema, "/$defs/build/properties"),
            set_of(BUILD_KEYS),
            "[build]"
        );
        assert_eq!(
            property_names(&schema, "/$defs/types/properties"),
            set_of(TYPES_KEYS),
            "[types]"
        );
    }

    #[test]
    fn the_dependency_table_keys_are_exactly_the_ones_the_parser_accepts() {
        assert_eq!(
            property_names(&schema(), "/$defs/dependencyTable/properties"),
            set_of(DEPENDENCY_KEYS)
        );
    }

    /// Every table the parser closes with an unknown-key error is closed in
    /// the schema too — otherwise the schema would bless a manifest the
    /// toolchain rejects.
    #[test]
    fn every_closed_table_rejects_additional_properties() {
        let schema = schema();
        for pointer in [
            "/additionalProperties",
            "/$defs/package/additionalProperties",
            "/$defs/build/additionalProperties",
            "/$defs/types/additionalProperties",
            "/$defs/dependencyTable/additionalProperties",
        ] {
            assert_eq!(at(&schema, pointer), &Value::Bool(false), "{pointer}");
        }
    }

    /// Each closed vocabulary appears once in the schema, as a `$defs` enum,
    /// and lists exactly the Rust enum's variants in declaration order. The
    /// second half — that every field of that type `$ref`s the definition —
    /// is what stops a new variant being added in one place only.
    #[test]
    fn every_closed_vocabulary_matches_its_rust_enum() {
        let schema = schema();
        assert_eq!(
            enum_values(&schema, "/$defs/dialect/enum"),
            DialectId::NAMES,
            "DialectId"
        );
        assert_eq!(
            enum_values(&schema, "/$defs/bundleMode/enum"),
            BundleMode::NAMES,
            "BundleMode"
        );
        assert_eq!(
            enum_values(&schema, "/$defs/lintLevel/enum"),
            LintLevel::NAMES,
            "LintLevel"
        );

        for pointer in [
            "/$defs/package/properties/edition/$ref",
            "/$defs/package/properties/lua-versions/items/$ref",
            "/$defs/build/properties/target/$ref",
        ] {
            assert_eq!(at(&schema, pointer), "#/$defs/dialect", "{pointer}");
        }
        assert_eq!(
            at(&schema, "/$defs/build/properties/mode/$ref"),
            "#/$defs/bundleMode"
        );
        assert_eq!(
            at(&schema, "/$defs/lint/additionalProperties/$ref"),
            "#/$defs/lintLevel",
            "an open-ended `[lint]` rule key still takes a level, not any string"
        );
    }

    /// `[lint]` is the one half-open table: the tier names are closed and
    /// modelled as fixed properties, everything else is a rule id constrained
    /// by `additionalProperties`.
    #[test]
    fn the_lint_tier_properties_are_exactly_the_rust_tier_names() {
        let mut tiers = property_names(&schema(), "/$defs/lint/properties");
        assert!(
            tiers.remove("globals"),
            "`[lint] globals` is a string array, not a level — it must be a fixed property"
        );
        assert_eq!(tiers, set_of(LintTier::NAMES));
    }

    /// The schema's `required` arrays, and the parser's behaviour, agree on
    /// exactly which keys a manifest cannot omit.
    #[test]
    fn the_required_keys_are_exactly_the_ones_the_parser_demands() {
        let schema = schema();
        let required = |pointer: &str| -> BTreeSet<String> {
            at(&schema, pointer)
                .as_array()
                .unwrap_or_else(|| panic!("`{pointer}` is not an array"))
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(required("/required"), set_of(&["package"]));
        assert_eq!(required("/$defs/package/required"), set_of(&["edition"]));

        // ...and that is what the parser actually does. `[package] edition`
        // alone parses; drop either and it does not. `name`/`version` are
        // deliberately optional (the rockspec supplies them), which the
        // minimal manifest below proves by omitting them.
        Manifest::parse("[package]\nedition = \"5.4\"\n")
            .expect("the schema's required set must be sufficient");
        assert!(
            Manifest::parse("[build]\nout = \"dist\"\n").is_err(),
            "a manifest with no `[package]` must be rejected"
        );
        assert!(
            Manifest::parse("[package]\nname = \"ok\"\n").is_err(),
            "a `[package]` with no `edition` must be rejected"
        );
        // Every other top-level table is optional in both.
        for table in TOP_LEVEL_KEYS.iter().filter(|key| **key != "package") {
            assert!(
                !required("/required").contains(*table),
                "`{table}` is optional to the parser but required by the schema"
            );
        }
    }

    // -----------------------------------------------------------------
    // the corpus: parser and schema must return the same verdict
    // -----------------------------------------------------------------

    /// The repository root, from this crate's manifest directory.
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("the crate lives at <repo>/crates/<name>")
            .to_path_buf()
    }

    /// Every `luabox.toml` under `examples/`, at any depth — so adding an
    /// example project extends this corpus with no edit here.
    fn example_manifests() -> Vec<PathBuf> {
        fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.file_name().is_some_and(|name| name == "luabox.toml") {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(&repo_root().join("examples"), &mut found);
        found.sort();
        assert!(
            !found.is_empty(),
            "no example manifests found — the corpus walk is broken, not empty"
        );
        found
    }

    /// Manifests the parser accepts. Every accepted *shape* the parser has a
    /// test for is represented, so the schema cannot be stricter than the
    /// toolchain.
    const VALID: &[(&str, &str)] = &[
        ("minimal", "[package]\nedition = \"5.4\"\n"),
        (
            "every [package] key",
            "[package]\nname = \"@acme/my-lib2\"\nversion = \"1.2.3-beta.1+build.5\"\nedition = \"5.4\"\n\
             description = \"a thing\"\nlicense = \"MIT\"\nlua-versions = [\"5.1\", \"5.4\"]\n\
             min-luabox-version = \"0.3.0\"\n",
        ),
        (
            "empty name and version are treated as absent",
            "[package]\nname = \"\"\nversion = \"\"\nedition = \"5.1\"\n",
        ),
        (
            "every [build] key",
            "[package]\nedition = \"5.4\"\n\n[build]\ntarget = \"5.1\"\nout = \"build\"\nmode = \"plain\"\n\
             entry = [\"src/cli.lua\", \"src/worker.lua\"]\nbundle = true\nsourcemap = true\nminify = true\n",
        ),
        (
            "a single-entry bundle with an outfile",
            "[package]\nedition = \"5.4\"\n\n[build]\nbundle = true\noutfile = \"dist/app.lua\"\n",
        ),
        (
            "an explicitly empty entry list",
            "[package]\nedition = \"5.4\"\n\n[build]\nentry = []\n",
        ),
        (
            "every bundle mode",
            "[package]\nedition = \"luajit\"\n\n[build]\nmode = \"nvim-plugin\"\n",
        ),
        (
            "[types]",
            "[package]\nedition = \"5.4\"\n\n[types]\nstrict = true\ndefs = [\"love2d\"]\n",
        ),
        (
            "every dependency form",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\na = \"1.0\"\n\
             b = { version = \"1.0\" }\nc = { path = \"../c\" }\nd = { path = \"../d\", version = \"3.0\" }\n\
             e = { git = \"https://example/e\" }\nf = { git = \"https://example/f\", rev = \"9f2c1ab\" }\n\
             g = { git = \"https://example/g\", tag = \"v1\" }\n\
             h = { git = \"https://example/h\", branch = \"main\", version = \"2.0\" }\n\
             i = { url = \"https://example/i.tar.gz\", sha256 = \"abc123\" }\n\
             j = { url = \"https://example/j.tar.gz\", sha256 = \"abc123\", version = \"4.0\" }\n",
        ),
        (
            "dev-dependencies take the same forms",
            "[package]\nedition = \"5.4\"\n\n[dev-dependencies]\nbusted = \"2.0\"\n",
        ),
        (
            "[lint] globals, every tier and an open rule id",
            "[package]\nedition = \"5.4\"\n\n[lint]\nglobals = [\"vim\", \"love\"]\n\
             correctness = \"deny\"\nsuspicious = \"warn\"\nperf = \"warn\"\nstyle = \"allow\"\n\
             pedantic = \"warn\"\nunused-local = \"allow\"\nglobal-write = \"deny\"\n",
        ),
    ];

    /// Manifests the parser rejects for *structural* reasons — an unknown
    /// key, a wrong type, a value outside a closed vocabulary, a missing
    /// required key, or an illegal combination of dependency keys. Semantic
    /// checks only the parser can make (did-you-mean quality, spans, the
    /// package-name grammar's per-segment rules) are deliberately out of
    /// scope and are not listed here.
    const INVALID: &[(&str, &str)] = &[
        ("no [package] table", "[build]\nout = \"dist\"\n"),
        ("no package.edition", "[package]\nname = \"ok\"\n"),
        (
            "unknown top-level table",
            "[package]\nedition = \"5.4\"\n\n[depndencies]\nx = \"1\"\n",
        ),
        (
            "a top-level table 0.2.0 removed",
            "[package]\nedition = \"5.4\"\n\n[workspace]\nmembers = []\n",
        ),
        (
            "unknown [package] key",
            "[package]\nedition = \"5.4\"\ntypo-key = \"x\"\n",
        ),
        (
            "unknown [build] key",
            "[package]\nedition = \"5.4\"\n\n[build]\nentrypoint = \"src/main.lua\"\n",
        ),
        (
            "unknown [types] key",
            "[package]\nedition = \"5.4\"\n\n[types]\nstrictly = true\n",
        ),
        (
            "unknown dependency key",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", ref = \"a\" }\n",
        ),
        (
            "package.edition outside the dialect vocabulary",
            "[package]\nedition = \"5.9\"\n",
        ),
        (
            "build.target outside the dialect vocabulary",
            "[package]\nedition = \"5.4\"\n\n[build]\ntarget = \"6.0\"\n",
        ),
        (
            "build.mode outside the mode vocabulary",
            "[package]\nedition = \"5.4\"\n\n[build]\nmode = \"roblox\"\n",
        ),
        (
            "a lua-versions entry outside the dialect vocabulary",
            "[package]\nedition = \"5.4\"\nlua-versions = [\"5.4\", \"6.0\"]\n",
        ),
        (
            "a lint level outside the level vocabulary",
            "[package]\nedition = \"5.4\"\n\n[lint]\nunused-local = \"alow\"\n",
        ),
        (
            "a lint tier level outside the level vocabulary",
            "[package]\nedition = \"5.4\"\n\n[lint]\npedantic = \"loud\"\n",
        ),
        (
            "a non-string lint level",
            "[package]\nedition = \"5.4\"\n\n[lint]\nunused-local = 3\n",
        ),
        (
            "package.name is not a string",
            "[package]\nname = 1\nedition = \"5.4\"\n",
        ),
        (
            "package.version is not semver-shaped",
            "[package]\nversion = \"1.0\"\nedition = \"5.4\"\n",
        ),
        (
            "package.min-luabox-version is not semver-shaped",
            "[package]\nedition = \"5.4\"\nmin-luabox-version = \"latest\"\n",
        ),
        (
            "build.bundle is not a boolean",
            "[package]\nedition = \"5.4\"\n\n[build]\nbundle = \"yes\"\n",
        ),
        (
            "build.out is not a string",
            "[package]\nedition = \"5.4\"\n\n[build]\nout = 5\n",
        ),
        (
            "types.defs is a bare string",
            "[package]\nedition = \"5.4\"\n\n[types]\ndefs = \"lib.d.lua\"\n",
        ),
        (
            "a types.defs entry is not a string",
            "[package]\nedition = \"5.4\"\n\n[types]\ndefs = [\"a.d.lua\", 3]\n",
        ),
        (
            "package.lua-versions is a bare string",
            "[package]\nedition = \"5.4\"\nlua-versions = \"5.1\"\n",
        ),
        (
            "a [lint] globals entry is not a string",
            "[package]\nedition = \"5.4\"\n\n[lint]\nglobals = [\"vim\", 3]\n",
        ),
        (
            "a section that is not a table",
            "build = 5\n\n[package]\nedition = \"5.4\"\n",
        ),
        (
            "a dependency that is neither string nor table",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\na = 3\n",
        ),
        (
            "a dependency table naming no source and no version",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = {}\n",
        ),
        (
            "a dependency table naming two sources",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", path = \"../y\" }\n",
        ),
        (
            "a git dependency pinned two ways",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { git = \"https://x\", rev = \"a\", tag = \"b\" }\n",
        ),
        (
            "a url dependency with no sha256",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { url = \"https://x/u.tar.gz\" }\n",
        ),
        (
            "a sha256 with no url source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { path = \"../d\", sha256 = \"abc\" }\n",
        ),
        (
            "a git reference key with no git source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { version = \"1.0\", tag = \"v1\" }\n",
        ),
        (
            "a rev with a path source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { path = \"../d\", rev = \"9f2c1ab\" }\n",
        ),
        (
            "a branch with a path source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { path = \"../d\", branch = \"main\" }\n",
        ),
        (
            "a tag with a url source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { url = \"https://x/u.tar.gz\", sha256 = \"abc\", tag = \"v1\" }\n",
        ),
        (
            "a rev with a url source",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\nbad = { url = \"https://x/u.tar.gz\", sha256 = \"abc\", rev = \"9f2c1ab\" }\n",
        ),
        (
            "the removed `workspace = true` dependency form",
            "[package]\nedition = \"5.4\"\n\n[dependencies]\ne = { workspace = true }\n",
        ),
    ];

    #[test]
    fn every_example_manifest_parses_and_validates() {
        let validator = validator();
        for path in example_manifests() {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
            Manifest::parse(&text)
                .unwrap_or_else(|errors| panic!("{} does not parse: {errors:?}", path.display()));
            let errors = schema_errors(&validator, &text);
            assert!(
                errors.is_empty(),
                "{} parses but fails the schema: {errors:#?}",
                path.display()
            );
        }
    }

    #[test]
    fn every_valid_fixture_parses_and_validates() {
        let validator = validator();
        for (label, text) in VALID {
            Manifest::parse(text)
                .unwrap_or_else(|errors| panic!("{label}: does not parse: {errors:?}"));
            let errors = schema_errors(&validator, text);
            assert!(
                errors.is_empty(),
                "{label}: parses but fails the schema: {errors:#?}"
            );
        }
    }

    #[test]
    fn every_invalid_fixture_is_rejected_by_both() {
        let validator = validator();
        for (label, text) in INVALID {
            assert!(
                Manifest::parse(text).is_err(),
                "{label}: the parser accepts a fixture the corpus calls invalid"
            );
            assert!(
                !schema_errors(&validator, text).is_empty(),
                "{label}: the parser rejects it but the schema accepts it"
            );
        }
    }

    /// The corpus is only proof if the fixtures are distinct — a copy/paste
    /// slip that duplicates one entry would quietly shrink the evidence.
    #[test]
    fn the_corpus_fixtures_are_distinct() {
        for corpus in [VALID, INVALID] {
            let labels: BTreeSet<&str> = corpus.iter().map(|(label, _)| *label).collect();
            assert_eq!(labels.len(), corpus.len(), "duplicate fixture label");
            let bodies: BTreeSet<&str> = corpus.iter().map(|(_, body)| *body).collect();
            assert_eq!(bodies.len(), corpus.len(), "duplicate fixture body");
        }
    }
}
