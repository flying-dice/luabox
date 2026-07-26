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

use luabox_bundle::{
    BundleError, BundleMap, BundleRequest, DynamicRequireSite, bundle, resolve_candidates,
    unmap_traceback,
};
use luabox_lower::{LowerDiagnostic, Severity};
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

// === degenerate graphs ===================================================

#[test]
fn a_lone_entry_bundles_without_the_module_map_or_shim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(\"solo\")\n");

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 0);
    // No requires means no module map, no `__luabox_require` shim: the
    // bundle is the banner plus the entry chunk verbatim.
    assert!(!out.text.contains("__luabox_modules"), "{}", out.text);
    assert!(!out.text.contains("__luabox_require"), "{}", out.text);
    assert_eq!(
        out.text,
        "-- bundled by luabox (5.1 -> 5.1)\nprint(\"solo\")\n"
    );
}

#[test]
fn an_empty_module_still_registers_and_maps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"blank\"))\n");
    write(root, "src/blank.lua", "");

    let entry = root.join("src/main.lua");
    let mut req = request(root, &entry, Dialect::Lua51, Dialect::Lua51);
    req.sourcemap = true;
    let out = bundle(&req).expect("bundle");
    assert_eq!(out.modules, 1);
    // The empty body contributes no bundle lines at all — registration and
    // its `end` sit back to back.
    assert!(
        out.text
            .contains("__luabox_modules[\"blank\"] = function(...)\nend\n"),
        "{}",
        out.text
    );
    let map = BundleMap::from_json(out.map.as_deref().expect("map")).expect("parse map");
    assert_eq!(map.lines.len(), out.text.lines().count());
    // The empty module is still a known file, it simply claims no lines.
    assert_eq!(
        map.files,
        vec!["src/blank.lua".to_owned(), "src/main.lua".to_owned()]
    );
    assert!(
        (1..=u32::try_from(map.lines.len()).expect("fits"))
            .filter_map(|l| map.lookup(l))
            .all(|(f, _)| f == "src/main.lua"),
        "an empty module contributes no mapped lines"
    );

    if let Some(runtime) = lua() {
        let script = write_bundle(root, &out.text);
        assert_eq!(run_lua(runtime, &script), "true\n");
    }
}

#[test]
fn one_file_reached_by_two_names_is_bundled_once_under_the_first() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // `pkg` and `pkg.init` are two require spellings of the same file.
    write(
        root,
        "src/main.lua",
        "local a = require(\"shared\")\nlocal b = require(\"shared\")\nprint(a, b)\n",
    );
    write(root, "src/shared.lua", "return \"once\"\n");

    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 1, "module identity is the file, not the edge");
    assert_eq!(
        out.text
            .matches("__luabox_modules[\"shared\"] = function(...)")
            .count(),
        1,
        "{}",
        out.text
    );
    assert_eq!(out.text.matches("__luabox_require(\"shared\")").count(), 2);
}

// === failure paths =======================================================

#[test]
fn a_missing_entry_file_is_an_io_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let entry = root.join("src/nope.lua");
    let err =
        bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect_err("missing entry");
    let BundleError::Io { path, .. } = &err else {
        panic!("expected Io, got {err}");
    };
    assert!(path.ends_with("src/nope.lua"), "{}", path.display());
    assert!(err.to_string().starts_with("cannot read `"), "{err}");
}

#[test]
fn a_missing_dependency_file_never_reaches_io() {
    // An unresolvable require is *external*, not an error — the graph walk
    // simply leaves the call site alone.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"ghost\"))\n");
    let entry = root.join("src/main.lua");
    let out = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51)).expect("bundle");
    assert_eq!(out.modules, 0);
    assert!(out.text.contains("require(\"ghost\")"), "{}", out.text);
}

#[test]
fn a_module_that_does_not_parse_is_a_parse_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"broken\"))\n");
    write(root, "src/broken.lua", "local = = =\n");

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua54, Dialect::Lua51))
        .expect_err("broken module must fail");
    // A syntax error is caught while *lowering* the module, so it surfaces
    // as LB0001 through the Lower variant rather than the reparse check.
    let BundleError::Lower { file, diagnostics } = &err else {
        panic!("expected Lower, got {err}");
    };
    assert_eq!(file, "src/broken.lua");
    assert!(diagnostics.iter().any(|d| d.code == "LB0001"), "{err}");
    assert!(
        err.to_string().contains("cannot lower `src/broken.lua`"),
        "{err}"
    );
}

#[test]
fn a_same_dialect_bundle_still_rejects_a_module_that_does_not_parse() {
    // `edition == target` makes lowering the identity — no parse happens
    // there — so the bundler's own parse of each module is what catches it.
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"broken\"))\n");
    write(root, "src/broken.lua", "local = = =\n");

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51))
        .expect_err("broken module must fail");
    let BundleError::Parse { file, message } = &err else {
        panic!("expected Parse, got {err}");
    };
    assert_eq!(file, "src/broken.lua");
    assert!(!message.is_empty(), "the parser's own message is carried");
    assert_eq!(err.to_string(), format!("`src/broken.lua`: {message}"));
}

#[test]
fn a_module_with_an_irreducible_goto_is_a_lower_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(root, "src/main.lua", "print(require(\"jumpy\"))\n");
    write(
        root,
        "src/jumpy.lua",
        "while true do\n  goto out\nend\n::out::\nreturn 1\n",
    );

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua54, Dialect::Lua51))
        .expect_err("irreducible goto must fail the bundle");
    let BundleError::Lower { file, diagnostics } = &err else {
        panic!("expected Lower, got {err}");
    };
    assert_eq!(file, "src/jumpy.lua");
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        vec!["LB0601"]
    );
    let message = err.to_string();
    assert!(
        message.starts_with("cannot lower `src/jumpy.lua` for bundling:"),
        "{message}"
    );
    assert!(
        message.contains("\n  LB0601: irreducible `goto`"),
        "{message}"
    );
}

#[test]
fn a_construct_with_no_lowering_rule_fails_residual_validation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Hex float literals are 5.2+; nothing lowers them for a 5.1 target, so
    // they must not ship (same residual check `luabox build` runs).
    write(root, "src/main.lua", "local x = 0x1p4\nprint(x)\n");

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua54, Dialect::Lua51))
        .expect_err("hex float cannot target 5.1");
    let BundleError::Parse { file, message } = &err else {
        panic!("expected Parse, got {err}");
    };
    assert_eq!(file, "src/main.lua");
    assert!(message.contains("not legal under target 5.1"), "{message}");
    assert!(message.contains("no lowering rule"), "{message}");
    assert_eq!(err.to_string(), format!("`src/main.lua`: {message}"));
}

#[test]
fn dynamic_requires_are_reported_across_modules_in_file_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "src/main.lua",
        "local dep = require(\"dep\")\nlocal m = require(dep.name)\nprint(m)\n",
    );
    write(
        root,
        "src/dep.lua",
        "local M = {}\nM.name = \"x\"\nlocal other = require(M.name)\nreturn M\n",
    );

    let entry = root.join("src/main.lua");
    let err = bundle(&request(root, &entry, Dialect::Lua51, Dialect::Lua51))
        .expect_err("dynamic requires must fail");
    let BundleError::DynamicRequires(sites) = &err else {
        panic!("expected DynamicRequires, got {err}");
    };
    // Sorted by (file, line), so `dep` precedes `main` regardless of the
    // order the BFS happened to visit them in.
    assert_eq!(
        sites,
        &[
            DynamicRequireSite {
                file: "src/dep.lua".to_owned(),
                line: 3,
            },
            DynamicRequireSite {
                file: "src/main.lua".to_owned(),
                line: 2,
            },
        ]
    );
}

// === error rendering =====================================================

#[test]
fn every_bundle_error_renders_its_own_shape() {
    let lower_diag = |code: &'static str, message: &str| LowerDiagnostic {
        code,
        severity: Severity::Error,
        message: message.to_owned(),
        range: rowan::TextRange::new(0.into(), 1.into()),
    };

    assert_eq!(
        BundleError::Io {
            path: PathBuf::from("src/gone.lua"),
            message: "No such file or directory (os error 2)".to_owned(),
        }
        .to_string(),
        "cannot read `src/gone.lua`: No such file or directory (os error 2)"
    );
    assert_eq!(
        BundleError::Parse {
            file: "src/a.lua".to_owned(),
            message: "unexpected token".to_owned(),
        }
        .to_string(),
        "`src/a.lua`: unexpected token"
    );
    assert_eq!(
        BundleError::Lower {
            file: "src/a.lua".to_owned(),
            diagnostics: vec![
                lower_diag("LB0601", "irreducible"),
                lower_diag("LB0604", "env")
            ],
        }
        .to_string(),
        "cannot lower `src/a.lua` for bundling:\n  LB0601: irreducible\n  LB0604: env"
    );
    assert_eq!(
        BundleError::EntryRequired {
            file: "src/x.lua".to_owned(),
            module: "main".to_owned(),
        }
        .to_string(),
        "`src/x.lua` requires \"main\", which is the entry module; bundling an entry that is \
         itself required is not supported yet"
    );
    assert_eq!(
        BundleError::SourceMap("expected value at line 1".to_owned()).to_string(),
        "invalid .lua.map: expected value at line 1"
    );
    assert_eq!(
        BundleError::SourceMapVersion(7).to_string(),
        "unsupported .lua.map version 7 (this luabox reads version 1)"
    );
    assert_eq!(
        BundleError::Internal("minify broke".to_owned()).to_string(),
        "internal bundler error: minify broke"
    );

    // A no-site dynamic-require error still renders the guidance body.
    let empty = BundleError::DynamicRequires(Vec::new()).to_string();
    assert!(empty.contains("must be a string literal"), "{empty}");
    assert!(empty.contains("allow-dynamic"), "{empty}");

    // The trait object is a real `std::error::Error`.
    let boxed: Box<dyn std::error::Error> = Box::new(BundleError::Internal("boom".to_owned()));
    assert_eq!(boxed.to_string(), "internal bundler error: boom");
}

#[test]
fn source_map_version_and_syntax_are_both_rejected() {
    let err = BundleMap::from_json("not json at all").expect_err("bad json");
    assert!(matches!(err, BundleError::SourceMap(_)), "{err}");
    let err = BundleMap::from_json(r#"{"version":2,"bundle":"x","files":[],"lines":[]}"#)
        .expect_err("bad version");
    assert!(matches!(err, BundleError::SourceMapVersion(2)), "{err}");
}

// === resolution candidates ===============================================

#[test]
fn illegal_module_names_have_no_candidates() {
    let root = Path::new("/project");
    for bad in ["", ".", "a.", ".a", "a..b", "..", "a...b"] {
        assert!(
            resolve_candidates(root, bad).is_empty(),
            "`{bad}` must not resolve anywhere"
        );
    }
    // A legal name still produces the SPEC §7 ordering.
    let ok = resolve_candidates(root, "a.b");
    assert_eq!(
        ok,
        vec![
            PathBuf::from("/project/a/b.lua"),
            PathBuf::from("/project/a/b/init.lua"),
            PathBuf::from("/project/src/a/b.lua"),
            PathBuf::from("/project/src/a/b/init.lua"),
            PathBuf::from("/project/lua_modules/a/src/b.lua"),
            PathBuf::from("/project/lua_modules/a/src/b/init.lua"),
            PathBuf::from("/project/lua_modules/a/b.lua"),
            PathBuf::from("/project/lua_modules/a/b/init.lua"),
        ]
    );
}
