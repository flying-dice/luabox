//! Black-box tests for the resolver library layer (SPEC.md §6):
//! dep kinds, version preference, lockfile behaviour, and cargo-style
//! conflict reports. CLI wiring (`luabox install/add`) is ticket #21.

use std::path::Path;

use luabox_resolve::{
    GitReference, LockedPackage, LockedSource, Lockfile, Manifest, PackageId, PackageMeta,
    PathProvider, ProviderError, Resolution, ResolveError, StackedProvider, StaticProvider,
    resolve, verify_resolution,
};
use semver::Version;

fn root_manifest(dependencies: &str) -> Manifest {
    root_manifest_with_edition("5.4", dependencies)
}

fn root_manifest_with_edition(edition: &str, dependencies: &str) -> Manifest {
    Manifest::parse(&format!(
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n\n[dependencies]\n{dependencies}"
    ))
    .expect("valid root manifest")
}

fn version_of(resolution: &Resolution, name: &str) -> Version {
    resolution
        .packages
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("package `{name}` in solution"))
        .version
        .clone()
}

fn v(s: &str) -> Version {
    Version::parse(s).expect("valid version")
}

#[test]
fn picks_highest_satisfying_version() {
    let mut provider = StaticProvider::new();
    provider.add("a", "1.0.0", &[]);
    provider.add("a", "1.2.0", &[]);
    provider.add("a", "2.0.0", &[]);
    let manifest = root_manifest("a = \"^1\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "a"), v("1.2.0"));
    verify_resolution(&manifest, Path::new("."), &provider, &resolution).expect("verifies");
}

#[test]
fn bare_requirement_is_caret() {
    let mut provider = StaticProvider::new();
    provider.add("a", "1.4.0", &[]);
    provider.add("a", "2.0.0", &[]);
    let manifest = root_manifest("a = \"1.2\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "a"), v("1.4.0"));
}

#[test]
fn semver_requirement_forms_resolve() {
    let mut provider = StaticProvider::new();
    for version in ["0.9.0", "1.0.0", "1.2.0", "1.2.5", "1.3.0", "2.0.0"] {
        provider.add("a", version, &[]);
    }
    for (req, expected) in [
        ("=1.2.0", "1.2.0"),
        (">=1.2.5", "2.0.0"),
        ("~1.2", "1.2.5"),
        ("<1.3.0", "1.2.5"),
        (">0.9, <1.3", "1.2.5"),
        ("1.*", "1.3.0"),
        ("*", "2.0.0"),
    ] {
        let manifest = root_manifest(&format!("a = \"{req}\"\n"));
        let resolution = resolve(&manifest, Path::new("."), &provider, None)
            .unwrap_or_else(|e| panic!("req `{req}` resolves: {e}"));
        assert_eq!(version_of(&resolution, "a"), v(expected), "req `{req}`");
    }
}

#[test]
fn prereleases_need_explicit_opt_in() {
    let mut provider = StaticProvider::new();
    provider.add("b", "1.0.0", &[]);
    provider.add("b", "1.1.0-alpha.1", &[]);
    let stable = resolve(
        &root_manifest("b = \"^1\"\n"),
        Path::new("."),
        &provider,
        None,
    )
    .expect("resolves");
    assert_eq!(version_of(&stable, "b"), v("1.0.0"));

    let pre = resolve(
        &root_manifest("b = \">=1.1.0-alpha\"\n"),
        Path::new("."),
        &provider,
        None,
    )
    .expect("resolves");
    assert_eq!(version_of(&pre, "b"), v("1.1.0-alpha.1"));
}

#[test]
fn diamond_dependencies_converge() {
    let mut provider = StaticProvider::new();
    provider.add("x", "1.0.0", &[("z", "^1.2")]);
    provider.add("y", "1.0.0", &[("z", "^1.4")]);
    for version in ["1.2.0", "1.4.0", "1.5.0", "2.0.0"] {
        provider.add("z", version, &[]);
    }
    let manifest = root_manifest("x = \"1\"\ny = \"1\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "z"), v("1.5.0"));
    assert_eq!(resolution.packages.len(), 3);
    verify_resolution(&manifest, Path::new("."), &provider, &resolution).expect("verifies");
}

#[test]
fn scoped_package_names_resolve() {
    let mut provider = StaticProvider::new();
    provider.add("@org/util", "1.1.0", &[]);
    provider.add("wrapper", "1.0.0", &[("@org/util", "^1")]);
    let manifest = root_manifest("wrapper = \"1\"\n\"@org/util\" = \"^1.0\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "@org/util"), v("1.1.0"));
    let text = resolution.lockfile.to_toml_string();
    assert!(text.contains("name = \"@org/util\""), "{text}");
}

#[test]
fn scoped_root_package_name_is_valid() {
    let manifest = Manifest::parse(
        "[package]\nname = \"@acme/tools\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n",
    )
    .expect("scoped package.name accepted (SPEC.md §19)");
    assert_eq!(manifest.package.name, "@acme/tools");
    assert!(
        Manifest::parse("[package]\nname = \"@acme\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n")
            .is_err(),
        "scope without name segment rejected"
    );
}

#[test]
fn compatible_lua_versions_pass() {
    let mut provider = StaticProvider::new();
    provider.add_with_lua("portable", "1.0.0", &[], &["5.1", "5.4"]);
    let manifest = root_manifest("portable = \"1\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "portable"), v("1.0.0"));
}

#[test]
fn dev_dependencies_of_root_participate() {
    let mut provider = StaticProvider::new();
    provider.add("helper", "1.0.0", &[]);
    let manifest = Manifest::parse(
        "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dev-dependencies]\nhelper = \"1\"\n",
    )
    .expect("valid manifest");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "helper"), v("1.0.0"));
}

#[test]
fn unknown_package_is_a_provider_error() {
    let provider = StaticProvider::new();
    let manifest = root_manifest("ghost = \"1\"\n");
    let err = resolve(&manifest, Path::new("."), &provider, None).unwrap_err();
    match &err {
        ResolveError::Provider(ProviderError::UnknownPackage { package }) => {
            assert_eq!(package, "ghost");
        }
        other => panic!("expected UnknownPackage, got {other:?}"),
    }
    assert!(err.to_string().contains("no package named `ghost`"));
}

#[test]
fn git_dependencies_resolve_and_lock() {
    let mut provider = StaticProvider::new();
    provider.add_package(
        PackageId::git(
            "promise",
            "https://example.com/promise.git",
            GitReference::Rev("abc123".to_owned()),
        ),
        v("1.0.0"),
        std::collections::BTreeMap::new(),
        PackageMeta::default(),
    );
    let manifest = root_manifest(
        "promise = { git = \"https://example.com/promise.git\", rev = \"abc123\" }\n",
    );
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    let lock_text = resolution.lockfile.to_toml_string();
    assert!(
        lock_text.contains("source = \"git+https://example.com/promise.git#abc123\""),
        "{lock_text}"
    );
}

// ---------------------------------------------------------------------------
// Path and workspace dependencies (PathProvider)
// ---------------------------------------------------------------------------

fn write_manifest(dir: &Path, contents: &str) {
    std::fs::create_dir_all(dir).expect("create dir");
    std::fs::write(dir.join("luabox.toml"), contents).expect("write manifest");
}

#[test]
fn path_dependencies_resolve_relative_to_the_dependent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let manifest_text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\nlocallib = { path = \"libs/locallib\" }\n";
    write_manifest(root, manifest_text);
    // locallib's own path dep is relative to *locallib's* directory.
    write_manifest(
        &root.join("libs/locallib"),
        "[package]\nname = \"locallib\"\nversion = \"0.2.0\"\nedition = \"5.4\"\n\n[dependencies]\nutil = { path = \"../util\" }\n\n[dev-dependencies]\nnot-resolved-dev-dep = \"9\"\n",
    );
    write_manifest(
        &root.join("libs/util"),
        "[package]\nname = \"util\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    );

    let manifest = Manifest::parse(manifest_text).expect("valid manifest");
    let provider = PathProvider::new();
    let resolution = resolve(&manifest, root, &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "locallib"), v("0.2.0"));
    assert_eq!(version_of(&resolution, "util"), v("0.1.0"));

    let lock_text = resolution.lockfile.to_toml_string();
    assert!(
        lock_text.contains("source = \"path+libs/locallib\""),
        "{lock_text}"
    );
    assert!(
        lock_text.contains("source = \"path+libs/util\""),
        "{lock_text}"
    );
    // Dev-dependencies of a *dependency* never participate.
    assert!(!lock_text.contains("not-resolved-dev-dep"), "{lock_text}");

    verify_resolution(&manifest, root, &provider, &resolution).expect("verifies");
}

#[test]
fn workspace_dependencies_resolve_against_members() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let manifest_text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\nalpha = { workspace = true }\n\n[workspace]\nmembers = [\"packages/*\"]\n";
    write_manifest(root, manifest_text);
    // alpha itself uses a workspace dep on beta: member → member.
    write_manifest(
        &root.join("packages/alpha"),
        "[package]\nname = \"alpha\"\nversion = \"1.0.0\"\nedition = \"5.4\"\n\n[dependencies]\nbeta = { workspace = true }\n",
    );
    write_manifest(
        &root.join("packages/beta"),
        "[package]\nname = \"beta\"\nversion = \"2.0.0\"\nedition = \"5.4\"\n",
    );

    let manifest = Manifest::parse(manifest_text).expect("valid manifest");
    let provider = PathProvider::new();
    let resolution = resolve(&manifest, root, &provider, None).expect("resolves");
    assert_eq!(version_of(&resolution, "alpha"), v("1.0.0"));
    assert_eq!(version_of(&resolution, "beta"), v("2.0.0"));
    let lock_text = resolution.lockfile.to_toml_string();
    assert!(
        lock_text.contains("source = \"path+packages/alpha\""),
        "{lock_text}"
    );
    assert!(
        lock_text.contains("source = \"path+packages/beta\""),
        "{lock_text}"
    );
}

#[test]
fn workspace_dependency_on_non_member_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let manifest_text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\nstranger = { workspace = true }\n\n[workspace]\nmembers = [\"packages/*\"]\n";
    write_manifest(root, manifest_text);

    let manifest = Manifest::parse(manifest_text).expect("valid manifest");
    let err = resolve(&manifest, root, &PathProvider::new(), None).unwrap_err();
    assert!(err.to_string().contains("not a workspace member"), "{err}");
}

#[test]
fn registry_and_path_sources_mix_via_stacked_provider() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let manifest_text = "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\npen = \"1\"\nlocallib = { path = \"locallib\" }\n";
    write_manifest(root, manifest_text);
    write_manifest(
        &root.join("locallib"),
        "[package]\nname = \"locallib\"\nversion = \"0.2.0\"\nedition = \"5.4\"\n\n[dependencies]\npen = \"^1.1\"\n",
    );

    let mut registry = StaticProvider::new();
    registry.add("pen", "1.0.0", &[]);
    registry.add("pen", "1.1.0", &[]);
    let paths = PathProvider::new();
    let provider = StackedProvider::new(vec![&paths, &registry]);

    let manifest = Manifest::parse(manifest_text).expect("valid manifest");
    let resolution = resolve(&manifest, root, &provider, None).expect("resolves");
    // Root wants ^1, locallib wants ^1.1 — intersection selects 1.1.0.
    assert_eq!(version_of(&resolution, "pen"), v("1.1.0"));
    assert_eq!(version_of(&resolution, "locallib"), v("0.2.0"));
    verify_resolution(&manifest, root, &provider, &resolution).expect("verifies");
}

// ---------------------------------------------------------------------------
// Lockfile behaviour
// ---------------------------------------------------------------------------

#[test]
fn lockfile_snapshot_is_deterministic_and_carries_checksums() {
    let mut provider = StaticProvider::new();
    provider.add_full("zeta", "2.1.0", &[], &[], Some("sha256:feed"));
    provider.add_full(
        "alpha",
        "1.0.0",
        &[("zeta", "^2")],
        &[],
        Some("sha256:beef"),
    );
    let manifest = root_manifest("alpha = \"1\"\n");
    let resolution = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    let expected = "\
# This file is @generated by luabox.
# It is not intended for manual editing.
version = 1

[[package]]
name = \"alpha\"
version = \"1.0.0\"
source = \"registry\"
checksum = \"sha256:beef\"
dependencies = [\"zeta\"]

[[package]]
name = \"myapp\"
version = \"0.1.0\"
dependencies = [\"alpha\"]

[[package]]
name = \"zeta\"
version = \"2.1.0\"
source = \"registry\"
checksum = \"sha256:feed\"
";
    assert_eq!(resolution.lockfile.to_toml_string(), expected);
    // Round trip: parse ∘ serialize is the identity on canonical text.
    let reparsed = Lockfile::parse(expected).expect("parses");
    assert_eq!(reparsed.to_toml_string(), expected);
}

#[test]
fn locked_version_wins_over_higher() {
    let mut provider = StaticProvider::new();
    provider.add("a", "1.0.0", &[]);
    provider.add("a", "1.2.0", &[]);
    let manifest = root_manifest("a = \"^1\"\n");
    let lock = Lockfile::new(vec![LockedPackage {
        name: "a".to_owned(),
        version: v("1.0.0"),
        source: Some(LockedSource::Registry),
        checksum: None,
        dependencies: vec![],
    }]);
    let resolution = resolve(&manifest, Path::new("."), &provider, Some(&lock)).expect("resolves");
    assert_eq!(
        version_of(&resolution, "a"),
        v("1.0.0"),
        "lock pin preferred"
    );

    // Without the lock the highest satisfying version wins.
    let fresh = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    assert_eq!(version_of(&fresh, "a"), v("1.2.0"));
}

#[test]
fn stale_pins_are_re_resolved_and_others_kept() {
    let mut provider = StaticProvider::new();
    provider.add("a", "1.0.0", &[]);
    provider.add("a", "2.0.0", &[]);
    provider.add("a", "2.1.0", &[]);
    provider.add("b", "1.0.0", &[]);
    provider.add("b", "1.5.0", &[]);
    // Requirement on `a` changed to ^2: the a-pin is stale; the b-pin must
    // survive (only the affected subgraph re-resolves).
    let manifest = root_manifest("a = \"^2\"\nb = \"^1\"\n");
    let lock = Lockfile::new(vec![
        LockedPackage {
            name: "a".to_owned(),
            version: v("1.0.0"),
            source: Some(LockedSource::Registry),
            checksum: None,
            dependencies: vec![],
        },
        LockedPackage {
            name: "b".to_owned(),
            version: v("1.0.0"),
            source: Some(LockedSource::Registry),
            checksum: None,
            dependencies: vec![],
        },
    ]);
    let resolution = resolve(&manifest, Path::new("."), &provider, Some(&lock)).expect("resolves");
    assert_eq!(
        version_of(&resolution, "a"),
        v("2.1.0"),
        "stale pin re-resolved"
    );
    assert_eq!(
        version_of(&resolution, "b"),
        v("1.0.0"),
        "unaffected pin kept"
    );
}

#[test]
fn re_resolving_with_own_lock_is_churn_free() {
    let mut provider = StaticProvider::new();
    provider.add("x", "1.0.0", &[("z", "^1.2")]);
    provider.add("y", "1.0.0", &[("z", "^1.4")]);
    for version in ["1.2.0", "1.4.0", "1.5.0", "2.0.0"] {
        provider.add("z", version, &[]);
    }
    let manifest = root_manifest("x = \"1\"\ny = \"1\"\n");
    let first = resolve(&manifest, Path::new("."), &provider, None).expect("resolves");
    let second =
        resolve(&manifest, Path::new("."), &provider, Some(&first.lockfile)).expect("resolves");
    assert_eq!(
        second.lockfile.to_toml_string(),
        first.lockfile.to_toml_string(),
        "lock-driven re-resolve must be byte-identical"
    );
}

// ---------------------------------------------------------------------------
// Conflict reports (snapshot-asserted human messages)
// ---------------------------------------------------------------------------

#[test]
fn direct_version_conflict_reads_like_cargo() {
    let mut provider = StaticProvider::new();
    provider.add("left", "1.0.0", &[("shared", "^1")]);
    provider.add("right", "1.0.0", &[("shared", "^2")]);
    provider.add("shared", "1.0.0", &[]);
    provider.add("shared", "2.0.0", &[]);
    let manifest = root_manifest("left = \"1\"\nright = \"1\"\n");
    let err = resolve(&manifest, Path::new("."), &provider, None).unwrap_err();
    assert!(matches!(err, ResolveError::NoSolution { .. }));
    assert_eq!(
        err.to_string(),
        "Because left >=1.0.0, <2.0.0 depends on shared >=1.0.0, <2.0.0 and right 1.0.0 depends on shared >=2.0.0, <3.0.0, left >=1.0.0, <2.0.0 and right 1.0.0 are incompatible.\n\
         And because myapp depends on left >=1.0.0, <2.0.0 and myapp depends on right >=1.0.0, <2.0.0, version solving failed."
    );
}

#[test]
fn transitive_conflict_reads_like_cargo() {
    let mut provider = StaticProvider::new();
    provider.add("a", "1.0.0", &[("b", "^1")]);
    provider.add("b", "1.0.0", &[("c", "^2")]);
    provider.add("c", "1.0.0", &[]);
    provider.add("c", "2.0.0", &[]);
    let manifest = root_manifest("a = \"1\"\nc = \"^1\"\n");
    let err = resolve(&manifest, Path::new("."), &provider, None).unwrap_err();
    assert!(matches!(err, ResolveError::NoSolution { .. }));
    assert_eq!(
        err.to_string(),
        "Because b >=1.0.0, <2.0.0 depends on c >=2.0.0, <3.0.0 and a 1.0.0 depends on b >=1.0.0, <2.0.0, a 1.0.0 depends on c >=2.0.0, <3.0.0.\n\
         And because myapp depends on c >=1.0.0, <2.0.0 and myapp depends on a >=1.0.0, <2.0.0, version solving failed."
    );
}

#[test]
fn lua_versions_conflict_reads_like_cargo() {
    let mut provider = StaticProvider::new();
    provider.add_with_lua("oldlib", "1.0.0", &[], &["5.1", "5.2"]);
    let manifest = root_manifest_with_edition("5.4", "oldlib = \"1\"\n");
    let err = resolve(&manifest, Path::new("."), &provider, None).unwrap_err();
    assert!(matches!(err, ResolveError::NoSolution { .. }));
    assert_eq!(
        err.to_string(),
        "Because no version of oldlib matches >1.0.0, <2.0.0 and oldlib 1.0.0 supports Lua 5.1, 5.2 but the project's edition is Lua 5.4, oldlib >=1.0.0, <2.0.0 cannot be used.\n\
         And because myapp depends on oldlib >=1.0.0, <2.0.0, version solving failed."
    );
}
