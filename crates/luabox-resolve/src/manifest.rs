//! `luabox.toml` — project manifest (SPEC.md §5, §6, §15).
//!
//! Owned end-to-end by this crate as part of the Distribution bounded
//! context (SPEC.md §16): Distribution "never parses syntax", so `edition`
//! and `build.target` are validated as plain strings against a local
//! allow-list ([`ALLOWED_DIALECTS`]) — this module has no dependency on
//! `luabox-syntax` and never will.
//!
//! [`Manifest::parse`] collects *every* validation error in one pass
//! (SPEC.md §14: batch diagnostics, not fail-fast) and, on success, keeps
//! the parsed [`toml_edit::DocumentMut`] alongside the typed view so the
//! manifest round-trips byte-identically, comments and formatting intact.

mod error;
mod model;
mod parse;

pub use error::ManifestError;
pub use model::{
    ALLOWED_BUNDLE_MODES, ALLOWED_DIALECTS, Build, DEFAULT_ENTRY, Dependency, GitDependency,
    LINT_TIERS, Lint, LintLevel, Manifest, Package, PathDependency, Types, UrlDependency,
};

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
        assert_eq!(manifest.package.edition, "5.4");
        assert_eq!(manifest.package.license.as_deref(), Some("MIT"));
        assert_eq!(manifest.package.description, None);

        assert_eq!(manifest.build.target, "5.1");
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

        assert_eq!(manifest.build.target, "5.1", "target defaults to edition");
        assert_eq!(manifest.build.out, "dist");
        assert_eq!(manifest.build.mode, "plain", "mode defaults to plain");
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
        assert_eq!(manifest.package.edition, "5.4");
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
        for dialect in ALLOWED_DIALECTS {
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
            assert_eq!(manifest.build.mode, mode);
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
        for mode in ALLOWED_BUNDLE_MODES {
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
            vec!["5.1".to_owned(), "5.4".to_owned()]
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
        // they get the standard unknown-table error, not parse-but-ignore.
        for table in ["tasks", "workspace"] {
            let src = format!("{PREAMBLE}\n[{table}]\nx = \"y\"\n");
            let errors = Manifest::parse(&src).unwrap_err();
            assert!(
                errors.iter().any(|e| e.message.contains(table)),
                "[{table}] should be an unknown-table error, got {errors:?}"
            );
        }
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
    fn version_only_table_still_rejects_orphan_source_modifiers_legacy() {
        // Kept from the first cut of #23: the with-version shape, asserted
        // directly.
        let git_ref = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { version = \"1.0\", tag = \"v1\" }\n";
        let errors = Manifest::parse(git_ref).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("git reference key but no `git` source")),
            "{errors:?}"
        );

        let digest = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { version = \"1.0\", sha256 = \"abc\" }\n";
        let errors = Manifest::parse(digest).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("only valid alongside a `url` source")),
            "{errors:?}"
        );
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
        assert_eq!(manifest.lint.tiers.get("pedantic"), Some(&LintLevel::Warn));
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
    fn the_backing_document_is_exposed_and_round_trips_byte_identically() {
        let src = format!(
            "# leading comment\n{PREAMBLE}\n# deps!\n[dependencies]\ngreet = {{ path = \"../greet\" }}\n"
        );
        let manifest = Manifest::parse(&src).expect("valid manifest");

        // `document()` hands back the lossless parse, and `Display` renders it
        // — comments and formatting intact.
        assert_eq!(manifest.document().to_string(), src);
        assert_eq!(manifest.to_string(), src);
        assert!(manifest.document().get("dependencies").is_some());
    }
}
