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

    let versions = provider
        .list_versions(&PackageId::registry("luarocks/a"))
        .unwrap();
    assert_eq!(versions, vec![Version::new(1, 0, 0)]);

    // A non-luarocks package is not this provider's job (falls through).
    assert!(
        provider
            .list_versions(&PackageId::registry("a"))
            .is_err_and(|e| matches!(e, luabox_resolve::ProviderError::UnsupportedSource { .. }))
    );
}

#[test]
fn dependencies_are_prefixed_and_lua_is_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let mirror = tmp.path().join("mirror");
    build_mirror(&mirror);
    let provider = provider(&tmp.path().join("cache"), &mirror);
    let id = PackageId::registry("luarocks/a");
    let v = Version::new(1, 0, 0);

    let deps = provider.dependencies(&id, &v).unwrap();
    // `b` is bridged (prefixed); `lua` is NOT a dependency.
    assert!(deps.contains_key("luarocks/b"), "deps: {deps:?}");
    assert!(!deps.keys().any(|k| k.contains("lua ") || k == "lua"));
    let luabox_resolve::manifest::Dependency::Version(req) = &deps["luarocks/b"] else {
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
        .fetch(&PackageId::registry("luarocks/a"), &Version::new(1, 0, 0))
        .unwrap();
    assert!(tree.join("a.lua").is_file(), "a.lua missing in {tree:?}");

    // Dotted module `b.core` → nested path `b/core.lua`.
    let tree = provider
        .fetch(&PackageId::registry("luarocks/b"), &Version::new(2, 1, 0))
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
        .dependencies(
            &PackageId::registry("luarocks/luasocket"),
            &Version::new(3, 0, 0),
        )
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

    let manifest = Manifest::parse(
        "[package]\n\
         name = \"app\"\n\
         version = \"0.1.0\"\n\
         edition = \"5.4\"\n\
         \n\
         [dependencies]\n\
         \"luarocks/a\" = \"1.0\"\n",
    )
    .expect("manifest parses");

    let resolution = resolve(&manifest, tmp.path(), &provider, None).expect("resolve succeeds");
    let names: Vec<&str> = resolution
        .packages
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(names.contains(&"luarocks/a"), "{names:?}");
    assert!(
        names.contains(&"luarocks/b"),
        "transitive rock missing: {names:?}"
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
    let id = PackageId::registry("luarocks/inspect");

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
