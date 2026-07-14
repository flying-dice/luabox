//! Integration tests for the bundler (SPEC.md §7): resolution, cycle
//! semantics, tree-shaking, dynamic-require diagnostics, lowering
//! integration, minify, and sourcemap round-trips — with real-runtime
//! verification against `lua` when it is on `PATH` (skipped gracefully
//! otherwise; CI provides it via the toolchain work, ticket #23).

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use luabox_bundle::{BundleError, BundleMap, BundleRequest, bundle, unmap_traceback};
use luabox_syntax::Dialect;

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, content).expect("write fixture");
}

fn request<'a>(root: &'a Path, entry: &'a Path, from: Dialect, to: Dialect) -> BundleRequest<'a> {
    BundleRequest {
        root,
        entry,
        edition: from,
        target: to,
        name: "app.lua",
        minify: false,
        sourcemap: false,
    }
}

/// `lua` from `PATH`, when present (Lua 5.1 in CI/dev per ticket #23).
fn lua() -> Option<&'static str> {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let ok = *AVAILABLE.get_or_init(|| {
        Command::new("lua")
            .arg("-v")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if ok {
        Some("lua")
    } else {
        eprintln!("skipping real-runtime assertion: no `lua` on PATH");
        None
    }
}

/// Run a bundle under the real runtime and return its stdout.
fn run_lua(runtime: &str, script: &Path) -> String {
    let output = Command::new(runtime)
        .arg(script)
        .output()
        .expect("spawn lua");
    assert!(
        output.status.success(),
        "lua failed on {}:\nstdout: {}\nstderr: {}",
        script.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
}

fn write_bundle(dir: &Path, text: &str) -> PathBuf {
    let path = dir.join("app.lua");
    std::fs::write(&path, text).expect("write bundle");
    path
}

// === resolution ==========================================================

#[test]
fn resolution_covers_dotted_init_and_lua_modules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        r#"local ab = require("a.b")
local c = require("c")
local pkg = require("pkg")
local extra = require("pkg.extra")
print(ab, c, pkg, extra)
"#,
    );
    write(root, "src/a/b.lua", "return \"dotted\"\n");
    write(root, "src/c/init.lua", "return \"init\"\n");
    write(
        root,
        "lua_modules/pkg/src/init.lua",
        "return \"pkg-init\"\n",
    );
    write(
        root,
        "lua_modules/pkg/src/extra.lua",
        "return \"pkg-extra\"\n",
    );

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 4);
    for key in ["\"a.b\"", "\"c\"", "\"pkg\"", "\"pkg.extra\""] {
        assert!(
            out.text
                .contains(&format!("__luabox_modules[{key}] = function(...)")),
            "missing registration for {key}:\n{}",
            out.text
        );
        assert!(
            out.text.contains(&format!("__luabox_require({key})")),
            "missing rewritten require for {key}:\n{}",
            out.text
        );
    }
    // The rewrite replaced every static require in the reachable graph
    // (a leading space would mean a bare `require(` call survived).
    assert!(!out.text.contains(" require(\"a.b\")"), "{}", out.text);

    if let Some(runtime) = lua() {
        let script = write_bundle(root, &out.text);
        assert_eq!(
            run_lua(runtime, &script),
            "dotted\tinit\tpkg-init\tpkg-extra\n"
        );
    }
}

#[test]
fn unresolved_requires_are_left_for_the_runtime() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local util = require(\"util\")\nlocal ok = pcall(require, \"socket.core\")\n\
         local direct = require(\"socket\")\nprint(util, ok, direct)\n",
    );
    write(root, "src/util.lua", "return 1\n");

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 1);
    assert!(
        out.text.contains("__luabox_require(\"util\")"),
        "{}",
        out.text
    );
    // External module: original call site untouched, no bundle entry.
    assert!(out.text.contains("require(\"socket\")"), "{}", out.text);
    assert!(
        !out.text.contains("__luabox_modules[\"socket\"]"),
        "{}",
        out.text
    );
}

#[test]
fn requiring_the_entry_module_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "require(\"x\")\n");
    write(root, "src/x.lua", "require(\"main\")\nreturn true\n");

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51))
        .expect_err("entry cycle must fail");
    let BundleError::EntryRequired { file, module } = &err else {
        panic!("expected EntryRequired, got {err}");
    };
    assert_eq!(file, "src/x.lua");
    assert_eq!(module, "main");
}

// === cycles ==============================================================

#[test]
fn cycles_get_lua_faithful_partial_table_semantics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // `a` publishes its partial table through `package.loaded` before
    // requiring back into the cycle — the pattern real Lua modules use.
    // The shim caches through the real `package.loaded`, so `b`'s
    // re-entrant require of "a" must observe the partial table.
    write(
        root,
        "src/a.lua",
        "local M = {}\nM.tag = \"a\"\npackage.loaded[\"a\"] = M\n\
         local b = require(\"b\")\nM.partner = b.tag\nreturn M\n",
    );
    write(
        root,
        "src/b.lua",
        "local M = {}\nM.tag = \"b\"\nlocal a = require(\"a\")\nM.seen = a.tag\nreturn M\n",
    );
    write(
        root,
        "src/main.lua",
        "local a = require(\"a\")\nlocal b = require(\"b\")\n\
         print(a.tag, b.tag, a.partner, b.seen)\n",
    );

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");

    // The emitted shim implements the Lua 5.x loader protocol: cache is
    // `package.loaded` itself, written after the chunk runs, truthy hits
    // short-circuit, `true` stored for value-less modules.
    for line in [
        "local __luabox_loaded = type(package) == \"table\" and type(package.loaded) == \"table\" and package.loaded or {}",
        "local hit = __luabox_loaded[name]",
        "local ret = chunk(name)",
        "if ret ~= nil then",
        "__luabox_loaded[name] = ret",
        "elseif __luabox_loaded[name] == nil then",
        "__luabox_loaded[name] = true",
    ] {
        assert!(
            out.text.contains(line),
            "shim is missing `{line}`:\n{}",
            out.text
        );
    }

    if let Some(runtime) = lua() {
        let script = write_bundle(root, &out.text);
        // b saw a's *partial* table (tag set, partner not yet); a then
        // completed against b's finished table.
        assert_eq!(run_lua(runtime, &script), "a\tb\tb\ta\n");
    }
}

// === tree-shaking ========================================================

#[test]
fn unreachable_modules_are_tree_shaken() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"used\"))\n");
    write(root, "src/used.lua", "return \"used-module-body\"\n");
    write(root, "src/unused.lua", "UNREACHABLE_MARKER()\nreturn 0\n");

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 1);
    assert!(out.text.contains("used-module-body"));
    assert!(
        !out.text.contains("UNREACHABLE_MARKER"),
        "unreachable module must be shaken:\n{}",
        out.text
    );
}

// === dynamic requires ====================================================

#[test]
fn dynamic_requires_are_diagnosed_with_their_sites() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local which = \"a\"\nlocal m = require(which)\nprint(m)\n",
    );

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51))
        .expect_err("dynamic require must fail");
    let BundleError::DynamicRequires(sites) = &err else {
        panic!("expected DynamicRequires, got {err}");
    };
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].file, "src/main.lua");
    assert_eq!(sites[0].line, 2);
    let message = err.to_string();
    assert!(message.contains("src/main.lua:2"), "{message}");
    assert!(message.contains("string literal"), "{message}");
    assert!(message.contains("allow-dynamic"), "{message}");
}

// === lowering integration ================================================

#[test]
fn lowering_hoists_one_rt_prelude_and_strips_goto() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local util = require(\"util\")\nlocal mask = 5 & 3\nlocal i = 0\n::top::\n\
         i = i + 1\nif i < 2 then goto top end\nprint(i, mask, util.flags(6))\n",
    );
    write(
        root,
        "src/util.lua",
        "local M = {}\nfunction M.flags(x)\n  return x & 4\nend\nreturn M\n",
    );

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua54, Dialect::Lua51)).expect("bundle");

    // Both modules used `&`, yet exactly one hoisted prelude is emitted.
    assert_eq!(
        out.text.matches("local __luabox_rt = (function()").count(),
        1,
        "exactly one rt prelude:\n{}",
        out.text
    );
    assert!(out.text.contains("__luabox_rt.band"), "{}", out.text);
    assert!(
        !out.text.contains('&'),
        "5.3 operators must be lowered:\n{}",
        out.text
    );
    assert!(
        !out.text.contains("goto"),
        "goto must be lowered for 5.1:\n{}",
        out.text
    );

    if let Some(runtime) = lua() {
        let script = write_bundle(root, &out.text);
        assert_eq!(run_lua(runtime, &script), "2\t1\t4\n");
    }
}

// === minify ==============================================================

#[test]
fn minify_mangles_locals_keeps_properties_and_preserves_behaviour() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local util = require(\"util\")\nlocal accumulator = 0\nfor index = 1, 4 do\n\
         \x20 accumulator = accumulator + util.add(index, index)\nend\n\
         print(accumulator, util.name)\n",
    );
    write(
        root,
        "src/util.lua",
        "local M = {}\nM.name = \"util\"\nlocal function calculate(left, right)\n\
         \x20 return left + right\nend\nfunction M.add(left, right)\n\
         \x20 return calculate(left, right)\nend\nreturn M\n",
    );

    let entry = root.join("src/main.lua");
    let plain = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("plain");
    let mut req = request(root, &entry, Dialect::Lua51, Dialect::Lua51);
    req.minify = true;
    // `bundle` reparses its own output; an Ok here already carries the
    // mechanical "minified bundle still parses" guarantee.
    let minified = bundle(&req).expect("minified");

    for local in ["accumulator", "calculate", "left", "right", "index"] {
        assert!(
            !minified.text.contains(local),
            "local `{local}` must be mangled:\n{}",
            minified.text
        );
    }
    // Property names and the module-map keys are never mangled.
    assert!(minified.text.contains(".add"), "{}", minified.text);
    assert!(minified.text.contains(".name"), "{}", minified.text);
    assert!(minified.text.contains("\"util\""), "{}", minified.text);
    assert!(
        minified.text.len() < plain.text.len(),
        "minified ({}) not smaller than plain ({})",
        minified.text.len(),
        plain.text.len()
    );

    if let Some(runtime) = lua() {
        let plain_script = write_bundle(root, &plain.text);
        let plain_out = run_lua(runtime, &plain_script);
        let min_script = root.join("app.min.lua");
        std::fs::write(&min_script, &minified.text).expect("write minified");
        let min_out = run_lua(runtime, &min_script);
        assert_eq!(plain_out, min_out, "minify must not change behaviour");
        assert_eq!(plain_out, "20\tutil\n");
    }
}

// === sourcemap ===========================================================

#[test]
fn sourcemap_round_trips_and_unmaps_a_traceback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local util = require(\"util\")\nutil.explode()\n",
    );
    write(
        root,
        "src/util.lua",
        "local M = {}\nfunction M.explode()\n  error(\"kaboom\")\nend\nreturn M\n",
    );

    let entry = root.join("src/main.lua");
    let mut req = request(root, &entry, Dialect::Lua51, Dialect::Lua51);
    req.sourcemap = true;
    let out = bundle(&req).expect("bundle");
    let map = BundleMap::from_json(out.map.as_deref().expect("map requested")).expect("map");
    assert_eq!(map.bundle, "app.lua");
    assert_eq!(map.lines.len(), out.text.lines().count());

    // The `error("kaboom")` call is line 3 of src/util.lua; find it in the
    // bundle and assert the map points straight back.
    let bundle_line = out
        .text
        .lines()
        .position(|l| l.contains("kaboom"))
        .expect("kaboom line in bundle")
        + 1;
    let line = u32::try_from(bundle_line).expect("line fits");
    assert_eq!(map.lookup(line), Some(("src/util.lua", 3)));

    // Wrapper lines are unmapped; module keys map to files exactly once each.
    assert_eq!(map.lookup(1), None, "banner line is bundler-generated");
    assert!(map.files.contains(&"src/util.lua".to_owned()));
    assert!(map.files.contains(&"src/main.lua".to_owned()));

    // Synthetic traceback round-trip — the `luabox unmap` engine.
    let traceback = format!(
        "lua: app.lua:{line}: kaboom\nstack traceback:\n\tapp.lua:{line}: in function 'explode'\n"
    );
    let names = vec!["app.lua".to_owned(), "dist/app.lua".to_owned()];
    let rewritten = unmap_traceback(&map, &names, &traceback);
    assert!(
        rewritten.contains("lua: src/util.lua:3: kaboom"),
        "{rewritten}"
    );
    assert!(
        rewritten.contains("\tsrc/util.lua:3: in function 'explode'"),
        "{rewritten}"
    );

    // A real traceback from the real runtime, unmapped end to end.
    if let Some(runtime) = lua() {
        let script = write_bundle(root, &out.text);
        let output = Command::new(runtime)
            .arg(&script)
            .output()
            .expect("spawn lua");
        assert!(!output.status.success(), "bundle is expected to raise");
        let stderr = String::from_utf8_lossy(&output.stderr).replace('\\', "/");
        let full = script.to_string_lossy().replace('\\', "/");
        let rewritten = unmap_traceback(&map, &[full], &stderr);
        assert!(
            rewritten.contains("src/util.lua:3"),
            "real traceback unmap failed:\nstderr: {stderr}\nrewritten: {rewritten}"
        );
    }
}

#[test]
fn sourcemap_under_minify_keeps_module_granularity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"util\"))\n");
    write(root, "src/util.lua", "local value = 41\nreturn value + 1\n");

    let entry = root.join("src/main.lua");
    let mut req = request(root, &entry, Dialect::Lua51, Dialect::Lua51);
    req.minify = true;
    req.sourcemap = true;
    let out = bundle(&req).expect("bundle");
    let map = BundleMap::from_json(out.map.as_deref().expect("map")).expect("parse map");
    // Every mapped line still identifies its module file.
    let mapped: Vec<_> = (1..=u32::try_from(map.lines.len()).expect("fits"))
        .filter_map(|l| map.lookup(l))
        .collect();
    assert!(
        mapped.iter().any(|(f, _)| *f == "src/util.lua"),
        "{mapped:?}"
    );
    assert!(
        mapped.iter().any(|(f, _)| *f == "src/main.lua"),
        "{mapped:?}"
    );
}
