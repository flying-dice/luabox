//! LuaRocks bridge integration tests (SPEC.md §6, ticket #19).
//!
//! Hermetic scenarios use a local mirror directory (`with_mirror`) so no
//! network is touched: the mirror holds `<rock>-<version>.rockspec` files and
//! pre-extracted `<rock>-<version>/` source trees, exactly the shape
//! `LUABOX_LUAROCKS_MIRROR` expects. One live end-to-end test resolves and
//! fetches a real pure-Lua rock from luarocks.org, skipping gracefully when
//! the network is unavailable.

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::fs;
use std::path::Path;

use luabox_resolve::provider::{PackageId, PackageProvider};
use luabox_resolve::{LuaRocksProvider, Manifest, resolve};
use semver::Version;

/// Writes a file under `dir`, creating parent directories.
fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(&path, contents).expect("write fixture file");
}

/// A two-rock hermetic mirror: `a-1.0-1` (depends on `b >= 2.0`) and
/// `b-2.1-1`, each a pure-Lua builtin rock with one module.
fn build_mirror(mirror: &Path) {
    write(
        mirror,
        "a-1.0-1.rockspec",
        r#"
package = "a"
version = "1.0-1"
source = { url = "https://example.invalid/a-1.0.tar.gz" }
dependencies = {
  "lua >= 5.1",
  "b >= 2.0",
}
build = {
  type = "builtin",
  modules = { a = "a.lua" },
}
"#,
    );
    write(mirror, "a-1.0-1/a.lua", "return 'a'\n");

    write(
        mirror,
        "b-2.1-1.rockspec",
        r#"
package = "b"
version = "2.1-1"
source = { url = "https://example.invalid/b-2.1.tar.gz" }
dependencies = { "lua >= 5.1" }
build = {
  type = "builtin",
  modules = { ["b.core"] = "src/b/core.lua" },
}
"#,
    );
    write(mirror, "b-2.1-1/src/b/core.lua", "return 'b.core'\n");
}

fn provider(cache: &Path, mirror: &Path) -> LuaRocksProvider {
    LuaRocksProvider::new(cache).with_mirror(Some(mirror.to_path_buf()))
}

#[test]
fn lists_translated_versions_from_a_mirror() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);

    // Registry packages are keyed by bare rock name (no prefix).
    let versions = provider.list_versions(&PackageId::registry("a")).unwrap();
    assert_eq!(versions, vec![Version::new(1, 0, 0)]);

    // A path/git source is not this provider's job (falls through).
    assert!(
        provider
            .list_versions(&PackageId::path("a", "/tmp/a"))
            .is_err_and(|e| matches!(e, luabox_resolve::ProviderError::UnsupportedSource { .. }))
    );
}

#[test]
fn dependencies_are_bare_named_and_lua_is_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);
    let id = PackageId::registry("a");
    let v = Version::new(1, 0, 0);

    let deps = provider.dependencies(&id, &v).unwrap();
    // `b` is bridged by its bare name; `lua` is NOT a dependency.
    assert!(deps.contains_key("b"), "deps: {deps:?}");
    assert!(!deps.keys().any(|k| k.contains("lua ") || k == "lua"));
    let luabox_resolve::manifest::Dependency::Version(req) = &deps["b"] else {
        panic!("expected a version requirement");
    };
    assert_eq!(req, ">=2.0");

    // The `lua >= 5.1` constraint became lua-versions metadata.
    let meta = provider.metadata(&id, &v).unwrap();
    assert_eq!(meta.lua_versions, ["5.1", "5.2", "5.3", "5.4", "luajit"]);
}

#[test]
fn fetch_lays_out_the_module_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);

    // Single top-level module `a`.
    let tree = provider
        .fetch(&PackageId::registry("a"), &Version::new(1, 0, 0))
        .unwrap();
    assert!(tree.join("a.lua").is_file(), "a.lua missing in {tree:?}");

    // Dotted module `b.core` → nested path `b/core.lua`.
    let tree = provider
        .fetch(&PackageId::registry("b"), &Version::new(2, 1, 0))
        .unwrap();
    assert!(
        tree.join("b").join("core.lua").is_file(),
        "b/core.lua missing in {tree:?}"
    );
}

#[test]
fn c_rock_is_rejected_with_a_naming_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    write(
        &mirror,
        "luasocket-3.0-1.rockspec",
        r#"
package = "luasocket"
version = "3.0-1"
source = { url = "git+https://example.invalid/luasocket.git" }
build = {
  type = "make",
  modules = { ["socket.core"] = "src/luasocket.c" },
}
"#,
    );
    let provider = provider(&tmp.path().join("cache"), &mirror);
    let err = provider
        .dependencies(&PackageId::registry("luasocket"), &Version::new(3, 0, 0))
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("luasocket"), "{text}");
    assert!(text.contains("C/native module"), "{text}");
}

#[test]
fn resolve_bridges_a_rock_and_its_transitive_dependency() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);

    // A rockspec declares the registry dependency; the rockspec is the
    // package manifest, merged onto an edition-only luabox.toml.
    let manifest = Manifest::parse("[package]\nedition = \"5.4\"\n").expect("manifest parses");
    let spec = luabox_resolve::luarocks::rockspec::read(
        "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = { \"a >= 1.0\" }\n",
    );
    let effective =
        luabox_resolve::effective_manifest(&manifest, Some(&spec)).expect("merge succeeds");

    let resolution = resolve(&effective, tmp.path(), &provider, None).expect("resolve succeeds");
    let names: Vec<&str> = resolution
        .packages
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"a"), "{names:?}");
    assert!(names.contains(&"b"), "transitive rock missing: {names:?}");
}

/// A one-rock hermetic mirror: `oldrock-1.0-1`, a pure-Lua builtin whose
/// rockspec constrains `lua >= 5.1, < 5.4` — i.e. it supports the
/// `{5.1, 5.2, 5.3}` family (plus luajit, 5.1-family) but **not** 5.4.
fn build_boundary_mirror(mirror: &Path) {
    write(
        mirror,
        "oldrock-1.0-1.rockspec",
        r#"package = "oldrock"
version = "1.0-1"
source = { url = "git+https://example.com/oldrock.git" }
dependencies = {
  "lua >= 5.1, < 5.4",
}
build = {
  type = "builtin",
  modules = { oldrock = "oldrock.lua" },
}
"#,
    );
    write(mirror, "oldrock-1.0-1/oldrock.lua", "return 'oldrock'\n");
}

/// The ecosystem boundary (#5): a rockspec's `lua` range becomes a family set
/// at the LuaRocks bridge, and that set gates the project's build target. A
/// rock supporting `{5.1, 5.2, 5.3}` is rejected for a 5.4-target project with
/// the `LB1003` diagnostic, and accepted for a 5.1-target project.
#[test]
fn rockspec_lua_range_becomes_a_family_set_and_gates_the_target() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_boundary_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);

    // The project's rockspec declares the registry dependency (the rockspec is
    // the package manifest); `luabox.toml` carries only the edition/target.
    let spec = luabox_resolve::luarocks::rockspec::read(
        "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = { \"oldrock >= 1.0\" }\n",
    );

    // Ships 5.4: 5.4 is not in {5.1, 5.2, 5.3, luajit}, and a rock declares no
    // lowerable edition, so this is a hard resolution failure (LB1003).
    let target_54 = Manifest::parse("[package]\nedition = \"5.4\"\n").expect("manifest parses");
    let effective_54 =
        luabox_resolve::effective_manifest(&target_54, Some(&spec)).expect("merge succeeds");
    let err = resolve(&effective_54, tmp.path(), &provider, None).unwrap_err();
    let report = err.to_string();
    assert!(report.contains("LB1003"), "expected LB1003, got: {report}");
    assert!(
        report.contains("supports Lua 5.1, 5.2, 5.3, luajit"),
        "expected the translated family set, got: {report}"
    );
    assert!(
        report.contains("the build target is Lua 5.4"),
        "expected the target named, got: {report}"
    );

    // Ships 5.1: 5.1 is in the set, so the rock resolves cleanly.
    let target_51 = Manifest::parse("[package]\nedition = \"5.1\"\n").expect("manifest parses");
    let effective_51 =
        luabox_resolve::effective_manifest(&target_51, Some(&spec)).expect("merge succeeds");
    let resolution = resolve(&effective_51, tmp.path(), &provider, None).expect("resolves for 5.1");
    assert!(
        resolution.packages.iter().any(|p| p.name == "oldrock"),
        "oldrock should resolve for a 5.1 target"
    );
}

// --- one live end-to-end test (skips gracefully offline) -----------------

/// Whether luarocks.org is reachable right now.
fn network_available() -> bool {
    std::process::Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "15",
            "-o",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
            "https://luarocks.org/manifest.json",
        ])
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn live_resolve_and_fetch_a_real_pure_lua_rock() {
    if !network_available() {
        eprintln!("skipping: luarocks.org is unreachable");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // No mirror: hit the real rocks server, caching under `cache`.
    let provider = LuaRocksProvider::new(tmp.path().join("cache"));
    let id = PackageId::registry("inspect");

    let versions = provider.list_versions(&id).expect("list inspect versions");
    assert!(
        versions.contains(&Version::new(3, 1, 3)),
        "expected inspect 3.1.3 among {versions:?}"
    );

    // `inspect` is a single-file pure-Lua rock; fetch and confirm the module.
    let tree = provider
        .fetch(&id, &Version::new(3, 1, 3))
        .expect("fetch inspect source");
    assert!(
        tree.join("inspect.lua").is_file(),
        "inspect.lua missing in fetched tree {tree:?}"
    );

    // Metadata reflects `lua >= 5.1`.
    let meta = provider.metadata(&id, &Version::new(3, 1, 3)).unwrap();
    assert!(meta.lua_versions.contains(&"5.4".to_owned()));
}
