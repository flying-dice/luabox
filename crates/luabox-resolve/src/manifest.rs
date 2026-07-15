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
//! the parsed [`toml_edit::DocumentMut`] alongside the typed view so later
//! edits (`luabox add`/`luabox remove`) preserve comments and formatting —
//! see [`Manifest::set_dependency`] for the pattern this generalizes to.
//! [`Manifest::workspace_members`] expands `[workspace] members` globs
//! (`packages/*`) into concrete member directories, cargo-style.

mod edit;
mod error;
mod model;
mod parse;
mod workspace;

pub use error::ManifestError;
pub use model::{
    ALLOWED_BUNDLE_MODES, ALLOWED_DIALECTS, Build, Dependency, GitDependency, LINT_TIERS, Lint,
    LintLevel, Manifest, Package, PathDependency, TaskValue, Types, Workspace, WorkspaceDependency,
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

        assert_eq!(
            manifest.dependencies.get("penlight"),
            Some(&Dependency::Version("1.14".to_owned()))
        );
        match manifest.dependencies.get("promise") {
            Some(Dependency::Git(git)) => {
                assert_eq!(git.rev.as_deref(), Some("abc123"));
                assert_eq!(git.tag, None);
                assert_eq!(git.branch, None);
            }
            other => panic!("expected a git dependency, got {other:?}"),
        }

        assert_eq!(
            manifest.dev_dependencies.get("busted-compat"),
            Some(&Dependency::Version("1.0".to_owned()))
        );

        assert_eq!(
            manifest.tasks.get("start"),
            Some(&TaskValue::Single("luabox run src/main.lua".to_owned()))
        );
        assert_eq!(
            manifest.tasks.get("ci"),
            Some(&TaskValue::Multiple(vec![
                "luabox check".to_owned(),
                "luabox lint".to_owned(),
                "luabox fmt --check".to_owned(),
            ]))
        );

        assert_eq!(
            manifest.workspace.as_ref().map(|w| w.members.clone()),
            Some(vec!["packages/*".to_owned()])
        );

        // Round-trip: an untouched document renders back byte-identical.
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
        assert!(!manifest.types.strict);
        assert!(manifest.types.defs.is_empty());
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.dev_dependencies.is_empty());
        assert!(manifest.tasks.is_empty());
        assert!(manifest.workspace.is_none());
    }

    #[test]
    fn missing_package_table_is_an_error() {
        let errors = Manifest::parse("[build]\ntarget = \"5.1\"\n").unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("[package]")));
    }

    #[test]
    fn missing_edition_is_reported_but_name_version_are_optional() {
        // `edition` is a tool concern and stays required; `name`/`version`
        // are optional in `luabox.toml` because the rockspec supplies them
        // (SPEC.md §6).
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
        // The slimmed `luabox.toml` scaffold: edition only, rockspec owns the
        // rest.
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
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\na = \"1.0\"\nb = { git = \"https://example/b\", tag = \"v1\" }\nc = { git = \"https://example/c\", branch = \"main\" }\nd = { path = \"../d\" }\ne = { workspace = true }\n";
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
        assert!(matches!(
            manifest.dependencies.get("e"),
            Some(Dependency::Workspace(_))
        ));
    }

    #[test]
    fn dependency_table_needs_exactly_one_kind() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbad = { version = \"1.0\" }\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("must specify one of"))
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
    fn task_value_string_and_array_forms() {
        let manifest = Manifest::parse(
            "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[tasks]\nsolo = \"echo hi\"\nmulti = [\"echo a\", \"echo b\"]\n",
        )
        .expect("valid manifest");
        assert_eq!(
            manifest.tasks.get("solo"),
            Some(&TaskValue::Single("echo hi".to_owned()))
        );
        assert_eq!(
            manifest.tasks.get("multi"),
            Some(&TaskValue::Multiple(vec![
                "echo a".to_owned(),
                "echo b".to_owned()
            ]))
        );
    }

    #[test]
    fn task_value_rejects_non_string_array_entries() {
        let src = "[package]\nname = \"ok\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[tasks]\nbad = [1, 2]\n";
        let errors = Manifest::parse(src).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("tasks.bad")));
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
}
