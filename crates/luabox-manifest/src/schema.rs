//! The published JSON Schema for `luabox.toml` (draft 2020-12).
//!
//! The manifest parser is hand-rolled ([`crate::model::Manifest::parse`]) so
//! it can collect *every* error with a span and a did-you-mean nudge — things
//! a schema validator cannot give. That used to leave the contract written
//! twice: once as Rust, once as the schema editors, validators and LLM coding
//! assistants read (`luabox schema`), with a parity suite standing between
//! them.
//!
//! Both are now rendered from one declarative table, [`crate::contract`]: the
//! parser builds its key allow-lists from it, and [`render`] below builds the
//! schema document from it plus a handful of hand-written fragments for the
//! shapes a key table cannot express. Most of the old parity suite is gone,
//! because what it asserted is now unrepresentable rather than merely untrue.
//!
//! # What still guards this file, and why
//!
//! * `schema_file_is_current` — the checked-in `schema/luabox.schema.json` is
//!   a *generated* artifact, kept in the tree because its `$id` is a raw
//!   GitHub URL an editor can point at, and because `luabox schema` prints it
//!   with `include_str!` rather than rendering at runtime. This test is what
//!   makes "generated" true; it rewrites the file under `LUABOX_BLESS=1`.
//! * `the_schema_is_a_valid_draft_2020_12_document` — the renderer emits a
//!   document, not necessarily a *schema*. Compiling it proves every `$ref`
//!   resolves and the meta-schema is satisfied.
//! * `the_schema_identifies_itself_by_its_canonical_url` and
//!   `every_property_and_definition_is_described` — the envelope and the
//!   hand-written `$defs`, which no table produces. The `$id` is the URL
//!   editors point at, and a `$defs` member with no prose defeats the point
//!   of publishing the document at all.
//! * `the_required_keys_are_exactly_the_ones_the_parser_demands` and
//!   `every_declared_value_type_is_the_one_the_parser_enforces` — the two
//!   columns of the table the parser reads by hand rather than by loop.
//!   Behavioural: they run `Manifest::parse` and check what it actually does.
//! * The corpus — every `examples/*/luabox.toml` in the repository plus a
//!   curated valid/invalid fixture set, run through **both** the schema
//!   validator and the parser, requiring the same verdict. This is what
//!   guards the hand-written fragments, above all the four-branch dependency
//!   `oneOf`, which no table drives and which an adversarial subset sweep
//!   showed has real teeth.
//!
//! What is *gone*: "the schema's properties equal the parser's key lists",
//! "the schema's enums equal the Rust vocabularies", "the schema's `[lint]`
//! tier properties equal `LintTier::NAMES`", "every closed table forbids
//! additional properties". One table now produces both sides of each of
//! those, so there is nothing left to compare.

/// The complete JSON Schema (draft 2020-12) describing `luabox.toml`.
///
/// Embedded verbatim from `schema/luabox.schema.json` so the file that ships
/// in the repository — and the one the `$id` URL serves — is byte-for-byte
/// what the binary prints. That file is generated from [`crate::contract`];
/// `schema_file_is_current` is what keeps it so.
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

/// Renders the schema document from [`crate::contract`].
///
/// Test-only on purpose: the binary prints the checked-in file with
/// `include_str!`, so nothing at runtime needs a JSON writer, and
/// `luabox-manifest` keeps `serde_json` as a dev-dependency. The generated
/// file is the artifact; this module is how it is produced.
#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test-support code — a malformed fragment is a bug, not an input"
)]
mod render {
    use serde::ser::{SerializeMap, SerializeSeq, Serializer};
    use serde_json::Value;

    use crate::contract::{self, Json, KeySpec, TableSpec, ValueTy, VocabularySpec};

    // -----------------------------------------------------------------
    // The envelope and the fragments no key table can express
    // -----------------------------------------------------------------

    const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";
    /// The canonical raw URL this file is served from on `main` — the address
    /// an editor's `$schema` line points at, so it is part of the contract.
    const ID: &str = "https://raw.githubusercontent.com/flying-dice/luabox/main/crates/luabox-manifest/schema/luabox.schema.json";
    const TITLE: &str = "luabox.toml";
    const ROOT_DESCRIPTION: &str = "The complete configuration contract for a luabox project manifest (`luabox.toml`, SPEC.md §5). luabox is a purely static Lua toolchain: it typechecks, lints, formats, lowers and bundles Lua, consumes a `lua_modules/` rock tree materialised by luarocks, and never fetches packages or runs Lua.\n\nTOML→JSON mapping caveat: this schema describes the manifest's *data model*, not its file syntax. You write `luabox.toml` in TOML; editors, validators and other tooling map that TOML to JSON using the standard mapping (tables → objects, arrays → arrays, strings/integers/booleans → their JSON counterparts) and validate the result against this document. Every `[table]` heading below is therefore an object property, and every `key = value` a property of that object.\n\nThis document is emitted verbatim by `luabox schema`, and is generated from the same declarative key table the hand-written parser (`luabox-manifest`) reads: the accepted key sets, the closed value vocabularies, and the required keys are a single source, so they cannot drift apart from what the toolchain actually accepts.";

    /// `[package] version` and `[package] min-luabox-version` share this
    /// shape check. The parser's `looks_like_semver` is the authority; this
    /// is its regex transcription, and the corpus is what holds the two
    /// together.
    const SEMVER_PATTERN: &str = r"^[0-9]+\.[0-9]+\.[0-9]+([-+][\s\S]*)?$";

    /// One legal shape of a dependency inline table.
    struct DependencyForm {
        title: &'static str,
        description: &'static str,
        /// The keys this form requires.
        needs: &'static [&'static str],
        /// Key sets this form forbids. Each entry is a *combination*: the
        /// form is rejected when every key in it is present, which is how
        /// "at most one of `rev`, `tag`, `branch`" is said in JSON Schema.
        forbids: &'static [&'static [&'static str]],
    }

    /// The four legal shapes of a dependency inline table.
    ///
    /// This is the one piece of the contract the key table cannot hold: it is
    /// about *combinations* of keys, which is exactly what
    /// `crate::parse::parse_dependency` hand-codes. The two are held together
    /// by the corpus, not by construction — every fixture below runs through
    /// both the parser and this schema and the verdicts must agree.
    const DEPENDENCY_FORMS: &[DependencyForm] = &[
        DependencyForm {
            title: "path source",
            description: "A sibling package read in place from the filesystem.",
            needs: &["path"],
            forbids: &[
                &["git"],
                &["url"],
                &["sha256"],
                &["rev"],
                &["tag"],
                &["branch"],
            ],
        },
        DependencyForm {
            title: "git source",
            description: "A git repository, optionally pinned by exactly one of `rev`, `tag` or `branch`.",
            needs: &["git"],
            forbids: &[
                &["path"],
                &["url"],
                &["sha256"],
                &["rev", "tag"],
                &["rev", "branch"],
                &["tag", "branch"],
            ],
        },
        DependencyForm {
            title: "url source",
            description: "An http(s) tarball, pinned by its SHA-256 digest.",
            needs: &["url", "sha256"],
            forbids: &[&["git"], &["path"], &["rev"], &["tag"], &["branch"]],
        },
        DependencyForm {
            title: "version only",
            description: "No source at all — the bare version-requirement string spelled longhand.",
            needs: &["version"],
            forbids: &[
                &["git"],
                &["path"],
                &["url"],
                &["sha256"],
                &["rev"],
                &["tag"],
                &["branch"],
            ],
        },
    ];

    // -----------------------------------------------------------------
    // An ordered JSON tree
    // -----------------------------------------------------------------

    /// A JSON value whose object keys keep the order they were written in.
    ///
    /// `serde_json::Value` sorts them, which would scramble a document meant
    /// to be read by humans (`type`, `title`, `description`, then the
    /// constraints). Emission order is the only reason this type exists.
    /// One ordered object member: its key, and its value.
    type Entry = (String, J);

    enum J {
        Leaf(Value),
        Arr(Vec<J>),
        Obj(Vec<Entry>),
    }

    impl serde::Serialize for J {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                J::Leaf(value) => value.serialize(serializer),
                J::Arr(items) => {
                    let mut seq = serializer.serialize_seq(Some(items.len()))?;
                    for item in items {
                        seq.serialize_element(item)?;
                    }
                    seq.end()
                }
                J::Obj(entries) => {
                    let mut map = serializer.serialize_map(Some(entries.len()))?;
                    for (key, value) in entries {
                        map.serialize_entry(key, value)?;
                    }
                    map.end()
                }
            }
        }
    }

    fn text(value: &str) -> J {
        J::Leaf(Value::String(value.to_owned()))
    }

    fn kv(key: &str, value: J) -> Entry {
        (key.to_owned(), value)
    }

    fn obj<const N: usize>(entries: [(&str, J); N]) -> J {
        J::Obj(entries.into_iter().map(|(k, v)| kv(k, v)).collect())
    }

    fn strings(values: &[&str]) -> J {
        J::Arr(values.iter().map(|value| text(value)).collect())
    }

    fn reference(def: &str) -> J {
        obj([("$ref", text(&format!("#/$defs/{def}")))])
    }

    fn literal(value: Json) -> J {
        match value {
            Json::Str(value) => text(value),
            Json::Bool(value) => J::Leaf(Value::Bool(value)),
            Json::Array(items) => J::Arr(items.iter().copied().map(literal).collect()),
        }
    }

    // -----------------------------------------------------------------
    // Rendering the table
    // -----------------------------------------------------------------

    /// The `type`/`$ref` head a shape contributes, and the constraints it
    /// contributes *after* `description` — the order this document reads in.
    fn shape(ty: &ValueTy) -> (Vec<Entry>, Vec<Entry>) {
        match *ty {
            ValueTy::Str { pattern } => (
                vec![kv("type", text("string"))],
                pattern
                    .map(|p| kv("pattern", text(p)))
                    .into_iter()
                    .collect(),
            ),
            ValueTy::Bool => (vec![kv("type", text("boolean"))], Vec::new()),
            ValueTy::ArrayOf {
                item,
                item_description,
            } => {
                let (mut entries, tail) = shape(item);
                entries.extend(tail);
                if let Some(description) = item_description {
                    entries.push(kv("description", text(description)));
                }
                (
                    vec![kv("type", text("array"))],
                    vec![kv("items", J::Obj(entries))],
                )
            }
            ValueTy::Ref(def) => (
                vec![kv("$ref", text(&format!("#/$defs/{def}")))],
                Vec::new(),
            ),
            // A fragment contributes its members where a constraint would go
            // — after `description`. `serde_json::Map` sorts them, which is
            // why a fragment is only ever the one small object the key table
            // cannot hold, never a shape whose reading order matters.
            ValueTy::Fragment(raw) => {
                let parsed: serde_json::Map<String, Value> =
                    serde_json::from_str(raw).expect("a `ValueTy::Fragment` is a JSON object");
                (
                    Vec::new(),
                    parsed.into_iter().map(|(k, v)| (k, J::Leaf(v))).collect(),
                )
            }
        }
    }

    fn property(key: &KeySpec) -> J {
        let (mut entries, tail) = shape(&key.ty);
        if let Some(title) = key.title {
            entries.push(kv("title", text(title)));
        }
        entries.push(kv("description", text(key.description)));
        entries.extend(tail);
        if let Some(default) = key.default {
            entries.push(kv("default", literal(default)));
        }
        if !key.examples.is_empty() {
            entries.push(kv(
                "examples",
                J::Arr(key.examples.iter().copied().map(literal).collect()),
            ));
        }
        J::Obj(entries)
    }

    fn properties(keys: &[KeySpec]) -> J {
        J::Obj(keys.iter().map(|key| kv(key.name, property(key))).collect())
    }

    /// The `required` array for `keys`, or `None` when nothing is required —
    /// an empty `required` is legal but noise.
    fn required(keys: &[KeySpec]) -> Option<J> {
        let names: Vec<&str> = keys
            .iter()
            .filter(|key| key.required)
            .map(|key| key.name)
            .collect();
        (!names.is_empty()).then(|| strings(&names))
    }

    /// One `$defs` table: its prose, what it says about keys it does not
    /// name, its keys, and any hand-written tail (the dependency `oneOf`).
    fn table(spec: &TableSpec, additional: J, tail: Vec<Entry>) -> J {
        let mut entries = vec![
            kv("type", text("object")),
            kv("title", text(spec.title)),
            kv("description", text(spec.description)),
            kv("additionalProperties", additional),
        ];
        entries.extend(required(spec.keys).map(|value| kv("required", value)));
        entries.push(kv("properties", properties(spec.keys)));
        entries.extend(tail);
        J::Obj(entries)
    }

    fn vocabulary(spec: &VocabularySpec) -> J {
        obj([
            ("type", text("string")),
            ("title", text(spec.title)),
            ("description", text(spec.description)),
            ("enum", strings(spec.names)),
        ])
    }

    fn closed() -> J {
        J::Leaf(Value::Bool(false))
    }

    // -----------------------------------------------------------------
    // The hand-written `$defs`
    // -----------------------------------------------------------------

    fn semver_def() -> J {
        obj([
            ("type", text("string")),
            ("title", text("Semantic version")),
            (
                "description",
                text(
                    "A semver-shaped version: `X.Y.Z` where each component is a run of ASCII digits, with an optional `-pre-release` and/or `+build` suffix. luabox shape-checks versions rather than ordering them (it resolves nothing), so anything after the first `-` or `+` is unconstrained.",
                ),
            ),
            ("pattern", text(SEMVER_PATTERN)),
            (
                "examples",
                strings(&["1.2.0", "0.1.0", "1.2.3-beta.1+build.5"]),
            ),
        ])
    }

    fn dependency_map_def() -> J {
        obj([
            ("type", text("object")),
            ("title", text("Dependency table")),
            (
                "description",
                text(
                    "A table of dependencies keyed by package name. Each value is either a bare version-requirement string or an inline table naming exactly one source.",
                ),
            ),
            ("additionalProperties", reference("dependency")),
        ])
    }

    fn dependency_def() -> J {
        obj([
            ("title", text("Dependency")),
            (
                "description",
                text(
                    "One `[dependencies]` / `[dev-dependencies]` entry (SPEC.md §6). Either a bare version-requirement string (`pkg = \"1.2.3\"`) or an inline table naming exactly one source — `path`, `git`, or `url` — or, equivalently to the bare string, only a `version`.",
                ),
            ),
            (
                "oneOf",
                J::Arr(vec![
                    obj([
                        ("type", text("string")),
                        ("title", text("Version requirement")),
                        (
                            "description",
                            text(
                                "A bare version requirement, e.g. `penlight = \"1.13\"`. Recorded verbatim; nothing resolves or compares it in v1.",
                            ),
                        ),
                        ("examples", strings(&["1.0", "1.2.3"])),
                    ]),
                    reference("dependencyTable"),
                ]),
            ),
        ])
    }

    /// The four-branch `oneOf` that makes the dependency source forms
    /// mutually exclusive, from [`DEPENDENCY_FORMS`].
    fn dependency_forms() -> J {
        J::Arr(
            DEPENDENCY_FORMS
                .iter()
                .map(|form| {
                    obj([
                        ("title", text(form.title)),
                        ("description", text(form.description)),
                        ("required", strings(form.needs)),
                        (
                            "not",
                            obj([(
                                "anyOf",
                                J::Arr(
                                    form.forbids
                                        .iter()
                                        .map(|keys| obj([("required", strings(keys))]))
                                        .collect(),
                                ),
                            )]),
                        ),
                    ])
                })
                .collect(),
        )
    }

    /// `[lint]`'s open half: any key that is not one the table names is a
    /// lint rule id, whose *value* is still closed to a level.
    fn lint_rule_levels() -> J {
        obj([
            ("$ref", text("#/$defs/lintLevel")),
            (
                "description",
                text(
                    "The level for one lint rule id, e.g. `unused-local = \"allow\"`. Ids live in `luabox lint`; run `luabox explain LB1004` for what happens to an id it does not know.",
                ),
            ),
        ])
    }

    // -----------------------------------------------------------------
    // The document
    // -----------------------------------------------------------------

    fn defs() -> J {
        let mut entries: Vec<Entry> = contract::VOCABULARIES
            .iter()
            .map(|spec| kv(spec.def, vocabulary(spec)))
            .collect();
        entries.push(kv("semver", semver_def()));
        for spec in [&contract::PACKAGE, &contract::BUILD, &contract::TYPES] {
            entries.push(kv(spec.def, table(spec, closed(), Vec::new())));
        }
        entries.push(kv("dependencyMap", dependency_map_def()));
        entries.push(kv("dependency", dependency_def()));
        entries.push(kv(
            contract::DEPENDENCY.def,
            table(
                &contract::DEPENDENCY,
                closed(),
                vec![kv("oneOf", dependency_forms())],
            ),
        ));
        entries.push(kv(
            contract::LINT.def,
            table(&contract::LINT, lint_rule_levels(), Vec::new()),
        ));
        J::Obj(entries)
    }

    fn document() -> J {
        let mut entries = vec![
            kv("$schema", text(DRAFT)),
            kv("$id", text(ID)),
            kv("title", text(TITLE)),
            kv("description", text(ROOT_DESCRIPTION)),
            kv("type", text("object")),
            kv("additionalProperties", closed()),
        ];
        entries.extend(required(contract::ROOT).map(|value| kv("required", value)));
        entries.push(kv("properties", properties(contract::ROOT)));
        entries.push(kv("$defs", defs()));
        J::Obj(entries)
    }

    /// The schema document as the JSON text `schema/luabox.schema.json` holds
    /// — pretty-printed, ordered, newline-terminated.
    pub(super) fn schema_json() -> String {
        let mut text = serde_json::to_string_pretty(&document())
            .expect("the rendered document serializes as JSON");
        text.push('\n');
        text
    }
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

    use super::{json_schema, render};
    use crate::contract::{self, KeySpec, ValueTy};
    use crate::model::Manifest;

    // -----------------------------------------------------------------
    // helpers
    // -----------------------------------------------------------------

    fn schema() -> Value {
        serde_json::from_str(json_schema()).expect("the embedded schema is valid JSON")
    }

    /// The schema at `pointer`, or a panic naming the pointer that is missing.
    fn at<'a>(root: &'a Value, pointer: &str) -> &'a Value {
        root.pointer(pointer)
            .unwrap_or_else(|| panic!("schema has no `{pointer}`"))
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

    /// Asserts `Manifest::parse` rejects `source` with a message containing
    /// `needle`.
    fn assert_reports(source: &str, needle: &str) {
        let errors = Manifest::parse(source)
            .err()
            .unwrap_or_else(|| panic!("expected {source:?} to be rejected"));
        assert!(
            errors.iter().any(|error| error.message.contains(needle)),
            "expected an error containing {needle:?} for {source:?}, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------
    // the checked-in file is what the contract renders
    // -----------------------------------------------------------------

    /// The path of the generated artifact, relative to nothing — the test
    /// rewrites this exact file when blessing.
    fn schema_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/luabox.schema.json")
    }

    /// `schema/luabox.schema.json` is generated from [`crate::contract`]. It
    /// stays checked in because its `$id` is a raw GitHub URL editors point
    /// at, and because `luabox schema` prints it with `include_str!` rather
    /// than rendering at runtime — so the file, not the renderer, is what
    /// ships. This test is the whole reason "generated" is a true statement.
    ///
    /// Regenerate with:
    /// `LUABOX_BLESS=1 cargo test -p luabox-manifest schema_file_is_current`
    #[test]
    fn schema_file_is_current() {
        let rendered = render::schema_json();
        if std::env::var_os("LUABOX_BLESS").is_some() {
            std::fs::write(schema_path(), &rendered)
                .unwrap_or_else(|error| panic!("rewriting {}: {error}", schema_path().display()));
            return;
        }
        let checked_in = json_schema();
        assert!(
            rendered == checked_in,
            "crates/luabox-manifest/schema/luabox.schema.json is out of date — it is generated \
             from `crate::contract`, and the contract has changed.\n{}\n\nRegenerate it with:\n    \
             LUABOX_BLESS=1 cargo test -p luabox-manifest schema_file_is_current",
            first_difference(checked_in, &rendered)
        );
    }

    /// The first line where two documents differ, as a short report — a
    /// 20 KiB `assert_eq!` diff tells a reader nothing.
    fn first_difference(checked_in: &str, rendered: &str) -> String {
        for (number, (old, new)) in checked_in.lines().zip(rendered.lines()).enumerate() {
            if old != new {
                return format!(
                    "first difference at line {}:\n  checked in: {old}\n  rendered:   {new}",
                    number + 1
                );
            }
        }
        format!(
            "the files agree for {} lines; the checked-in file has {} of them and the rendered \
             document has {}",
            checked_in.lines().count().min(rendered.lines().count()),
            checked_in.lines().count(),
            rendered.lines().count()
        )
    }

    // -----------------------------------------------------------------
    // the document is a schema, not merely a document
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

    /// The document's envelope — the one part of the schema no key table
    /// produces, and the part outside tooling depends on most: the `$id` is
    /// the URL an editor's `$schema` line points at, so changing it silently
    /// unpoints every editor already configured.
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

    /// Every `properties` entry and every `$defs` member carries a non-empty
    /// `description`.
    ///
    /// For a key the table declares this is true by construction —
    /// `KeySpec::description` is a `&str`, not an `Option`. What this walk
    /// still earns its keep on is the hand-written half: the `$defs` the
    /// renderer writes out longhand (`semver`, `dependencyMap`,
    /// `dependency`), their nested `oneOf` branches, and the `[lint]`
    /// rule-level mapping. The schema has to be readable on its own, because
    /// that is the whole point of publishing it.
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
    // the two columns the parser reads by hand
    // -----------------------------------------------------------------

    /// The schema's `required` arrays, and the parser's behaviour, agree on
    /// exactly which keys a manifest cannot omit.
    ///
    /// The `required` flag in [`crate::contract`] renders the schema side by
    /// construction; nothing renders the parser side, which asks for `edition`
    /// through a `required: true` argument written out at the call site. This
    /// is what holds those together.
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
        for key in contract::ROOT.iter().filter(|key| key.name != "package") {
            assert!(
                !required("/required").contains(key.name),
                "`{}` is optional to the parser but required by the schema",
                key.name
            );
        }
    }

    /// A key's declared [`ValueTy`] is the one `Manifest::parse` enforces.
    ///
    /// The renderer turns `ty` into the schema's `type`/`$ref`; the parser
    /// picks its reader (`get_string`, `get_bool`, `get_string_array`) by
    /// hand at each call site. Feeding every key a value of a shape no key
    /// accepts, and requiring the error the declared type predicts, is what
    /// stops `ty` from being decoration.
    ///
    /// `[lint]` is excluded: it is half-open, its level keys are read by a
    /// bespoke loop with its own message ("must be a level string"), and the
    /// invalid corpus covers that shape through both the parser and the
    /// schema.
    #[test]
    fn every_declared_value_type_is_the_one_the_parser_enforces() {
        /// A TOML value no manifest key accepts, and the phrasing a key of
        /// the given shape must answer it with.
        fn probe(ty: &ValueTy) -> (&'static str, &'static str) {
            match *ty {
                // Enum-valued and pattern-constrained keys are read as a
                // string first, so an integer is the universal wrong shape.
                ValueTy::Str { .. } | ValueTy::Ref(_) | ValueTy::Fragment(_) => {
                    ("1", "must be a string")
                }
                ValueTy::Bool => ("\"yes\"", "must be a boolean"),
                ValueTy::ArrayOf { .. } => ("\"one\"", "must be an array of strings"),
            }
        }

        for key in contract::PACKAGE.keys {
            let (bad, expected) = probe(&key.ty);
            // `edition` is required, so it supplies its own bad value; every
            // other key needs a valid one alongside.
            let source = if key.name == "edition" {
                format!("[package]\nedition = {bad}\n")
            } else {
                format!("[package]\nedition = \"5.4\"\n{} = {bad}\n", key.name)
            };
            assert_reports(&source, &format!("`package.{}` {expected}", key.name));
        }

        for spec in [&contract::BUILD, &contract::TYPES] {
            for key in spec.keys {
                let (bad, expected) = probe(&key.ty);
                let source = format!(
                    "[package]\nedition = \"5.4\"\n\n[{}]\n{} = {bad}\n",
                    spec.def, key.name
                );
                assert_reports(&source, &format!("`{}.{}` {expected}", spec.def, key.name));
            }
        }

        for key in contract::DEPENDENCY.keys {
            let (bad, expected) = probe(&key.ty);
            let source = format!(
                "[package]\nedition = \"5.4\"\n\n[dependencies]\nd = {{ {} = {bad} }}\n",
                key.name
            );
            assert_reports(
                &source,
                &format!("`dependencies.d.{}` {expected}", key.name),
            );
        }
    }

    /// Every top-level key names a *table*, and the parser says so when it is
    /// handed a scalar — the shape check the root's [`ValueTy::Ref`] entries
    /// stand for.
    #[test]
    fn every_top_level_key_must_be_a_table() {
        for key in contract::ROOT {
            // The scalar has to precede `[package]` to stay a top-level key;
            // `package` itself cannot be written twice.
            let source = if key.name == "package" {
                "package = 5\n".to_owned()
            } else {
                format!("{} = 5\n[package]\nedition = \"5.4\"\n", key.name)
            };
            assert_reports(&source, &format!("`[{}]` must be a table", key.name));
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

    /// Every key in the contract is spelled once per table. A duplicate would
    /// render a duplicate schema property (silently collapsing) and give the
    /// parser two allow-list entries for one key.
    #[test]
    fn no_table_names_a_key_twice() {
        let tables: [(&str, &[KeySpec]); 6] = [
            ("top level", contract::ROOT),
            ("[package]", contract::PACKAGE.keys),
            ("[build]", contract::BUILD.keys),
            ("[types]", contract::TYPES.keys),
            ("dependency table", contract::DEPENDENCY.keys),
            ("[lint]", contract::LINT.keys),
        ];
        for (what, keys) in tables {
            let names: BTreeSet<&str> = keys.iter().map(|key| key.name).collect();
            assert_eq!(names.len(), keys.len(), "{what} names a key twice");
        }
    }
}
