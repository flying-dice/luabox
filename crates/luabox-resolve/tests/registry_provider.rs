//! Registry provider integration tests (SPEC.md §6, ticket #20): the
//! sparse-index [`RegistryProvider`] behind the resolver, over a hermetic
//! on-disk fixture registry.

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::fs;
use std::path::Path;

use luabox_resolve::registry::index_rel_path;
use luabox_resolve::{
    IndexDep, IndexEntry, LockedPackage, LockedSource, Lockfile, Manifest, PackageId,
    PackageProvider as _, ProviderError, Registry, RegistryError, RegistryProvider, ResolveError,
    resolve,
};
use semver::Version;

/// Write index lines for `name` directly into a fixture registry rooted at
/// `root`, at the crates.io-style prefix path.
fn write_index(root: &Path, name: &str, entries: &[IndexEntry]) {
    let path = root.join("index").join(index_rel_path(name));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = String::new();
    for entry in entries {
        text.push_str(&entry.to_json_line());
        text.push('\n');
    }
    fs::write(path, text).unwrap();
}

fn entry(name: &str, version: &str, deps: &[(&str, &str, bool)], yanked: bool) -> IndexEntry {
    IndexEntry {
        name: name.to_owned(),
        version: version.to_owned(),
        deps: deps
            .iter()
            .map(|(dep, req, dev)| IndexDep {
                name: (*dep).to_owned(),
                req: (*req).to_owned(),
                dev: *dev,
            })
            .collect(),
        lua_versions: vec![],
        checksum: format!("sha256:{}", "ab".repeat(32)),
        yanked,
    }
}

fn manifest(deps: &str) -> Manifest {
    let text = format!(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n[dependencies]\n{deps}"
    );
    Manifest::parse(&text).expect("fixture manifest parses")
}

fn provider_over(dir: &Path) -> RegistryProvider {
    RegistryProvider::new(Registry::open(&dir.to_string_lossy()).unwrap())
}

#[test]
fn lists_versions_and_skips_yanked() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[
            entry("penlight", "1.13.0", &[], false),
            entry("penlight", "1.14.0", &[], true),
        ],
    );
    let provider = provider_over(tmp.path());
    let versions = provider
        .list_versions(&PackageId::registry("penlight"))
        .unwrap();
    assert_eq!(versions, vec![Version::parse("1.13.0").unwrap()]);
}

#[test]
fn locked_yanked_version_stays_restorable() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[
            entry("penlight", "1.13.0", &[], false),
            entry("penlight", "1.14.0", &[], true),
        ],
    );
    // A lockfile that pins the (now yanked) 1.14.0.
    let lock = Lockfile::new(vec![LockedPackage {
        name: "penlight".to_owned(),
        version: Version::parse("1.14.0").unwrap(),
        source: Some(LockedSource::Registry),
        checksum: None,
        dependencies: vec![],
    }]);
    let provider = provider_over(tmp.path()).with_locked(&lock);
    let versions = provider
        .list_versions(&PackageId::registry("penlight"))
        .unwrap();
    assert!(
        versions.contains(&Version::parse("1.14.0").unwrap()),
        "a lockfile-pinned yanked version must stay resolvable, got {versions:?}"
    );

    // End to end: the resolve keeps the yanked pin (crates.io yank rule).
    let resolution = resolve(
        &manifest("penlight = \"^1\"\n"),
        tmp.path(),
        &provider,
        Some(&lock),
    )
    .expect("locked resolve succeeds");
    let locked = resolution.lockfile.get("penlight").unwrap();
    assert_eq!(locked.version, Version::parse("1.14.0").unwrap());
}

#[test]
fn new_resolutions_avoid_yanked_versions() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[
            entry("penlight", "1.13.0", &[], false),
            entry("penlight", "1.14.0", &[], true),
        ],
    );
    let provider = provider_over(tmp.path());
    let resolution = resolve(
        &manifest("penlight = \"^1\"\n"),
        tmp.path(),
        &provider,
        None,
    )
    .expect("resolve succeeds");
    let locked = resolution.lockfile.get("penlight").unwrap();
    assert_eq!(
        locked.version,
        Version::parse("1.13.0").unwrap(),
        "a fresh resolve must skip the yanked 1.14.0"
    );
}

#[test]
fn resolves_transitively_and_locks_registry_checksums() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[entry("penlight", "1.14.0", &[("base", "^2", false)], false)],
    );
    write_index(tmp.path(), "base", &[entry("base", "2.3.0", &[], false)]);

    let provider = provider_over(tmp.path());
    let resolution = resolve(
        &manifest("penlight = \"^1\"\n"),
        tmp.path(),
        &provider,
        None,
    )
    .expect("resolve succeeds");
    assert_eq!(resolution.packages.len(), 2);

    let text = resolution.lockfile.to_toml_string();
    assert!(text.contains("source = \"registry\""), "lockfile:\n{text}");
    assert!(
        text.contains(&format!("checksum = \"sha256:{}\"", "ab".repeat(32))),
        "registry checksums must land in the lockfile:\n{text}"
    );
}

#[test]
fn dev_deps_of_published_packages_do_not_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[entry(
            "penlight",
            "1.14.0",
            &[("busted", "^2", true)],
            false,
        )],
    );
    let provider = provider_over(tmp.path());
    let deps = provider
        .dependencies(
            &PackageId::registry("penlight"),
            &Version::parse("1.14.0").unwrap(),
        )
        .unwrap();
    assert!(
        deps.is_empty(),
        "dev-deps never participate in a consumer's resolution: {deps:?}"
    );
}

#[test]
fn scoped_names_use_the_org_directory() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "@acme/util",
        &[entry("@acme/util", "0.2.0", &[], false)],
    );
    assert!(
        tmp.path()
            .join("index")
            .join("@acme")
            .join("util")
            .is_file(),
        "scoped index files live under their org directory"
    );

    let provider = provider_over(tmp.path());
    let resolution = resolve(
        &manifest("\"@acme/util\" = \"^0.2\"\n"),
        tmp.path(),
        &provider,
        None,
    )
    .expect("scoped resolve succeeds");
    assert_eq!(resolution.packages[0].name, "@acme/util");
}

#[test]
fn unknown_packages_and_versions_are_clear_errors() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[entry("penlight", "1.0.0", &[], false)],
    );
    let provider = provider_over(tmp.path());
    assert!(matches!(
        provider.list_versions(&PackageId::registry("nope")),
        Err(ProviderError::UnknownPackage { .. })
    ));
    assert!(matches!(
        provider.dependencies(
            &PackageId::registry("penlight"),
            &Version::parse("9.9.9").unwrap()
        ),
        Err(ProviderError::VersionNotFound { .. })
    ));
    // Non-registry sources fall through to other providers in the stack.
    assert!(matches!(
        provider.list_versions(&PackageId::path("penlight", "/x")),
        Err(ProviderError::UnsupportedSource { .. })
    ));
}

#[test]
fn resolution_failure_names_the_registry_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    write_index(
        tmp.path(),
        "penlight",
        &[entry("penlight", "1.0.0", &[], false)],
    );
    let provider = provider_over(tmp.path());
    let err = resolve(
        &manifest("penlight = \"^2\"\n"),
        tmp.path(),
        &provider,
        None,
    )
    .unwrap_err();
    assert!(matches!(err, ResolveError::NoSolution { .. }), "{err}");
}

#[test]
fn publish_appends_refuses_duplicates_and_yank_flips() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Registry::open(&tmp.path().to_string_lossy()).unwrap();
    assert!(registry.is_writable());

    // The registry copies whatever artifact file it is handed; content is
    // opaque at this layer (install verifies the tree hash, not publish).
    let artifact = tmp.path().join("pkg.tar");
    fs::write(&artifact, b"fixture-tar-bytes").unwrap();

    let first = entry("mathlib", "1.0.0", &[], false);
    registry.publish(&first, &artifact).unwrap();
    assert!(
        tmp.path()
            .join("artifacts")
            .join("mathlib")
            .join("1.0.0.tar")
            .is_file(),
        "publish must store the artifact tar"
    );

    // Same version again: refused — index lines are immutable.
    let err = registry.publish(&first, &artifact).unwrap_err();
    assert!(
        matches!(err, RegistryError::DuplicateVersion { .. }),
        "{err}"
    );

    // A second version appends; yank flips its flag in place.
    registry
        .publish(&entry("mathlib", "1.1.0", &[], false), &artifact)
        .unwrap();
    assert!(registry.set_yanked("mathlib", "1.1.0", true).unwrap());
    // Idempotent: already yanked reports "no change".
    assert!(!registry.set_yanked("mathlib", "1.1.0", true).unwrap());
    let entries = registry.load_entries("mathlib").unwrap().unwrap();
    assert_eq!(entries.len(), 2, "yank never deletes");
    assert!(!entries[0].yanked && entries[1].yanked);

    // Yanking a version that was never published is an error.
    assert!(matches!(
        registry.set_yanked("mathlib", "3.0.0", true),
        Err(RegistryError::VersionNotInIndex { .. })
    ));
}

#[test]
fn https_registries_are_read_only() {
    let registry = Registry::open("https://pkgs.example.com/registry").unwrap();
    assert!(!registry.is_writable());
    let artifact = std::env::temp_dir().join("never-read.tar");
    let err = registry
        .publish(&entry("mathlib", "1.0.0", &[], false), &artifact)
        .unwrap_err();
    assert!(matches!(err, RegistryError::ReadOnly { .. }), "{err}");
    assert!(matches!(
        registry.set_yanked("mathlib", "1.0.0", true),
        Err(RegistryError::ReadOnly { .. })
    ));
}
