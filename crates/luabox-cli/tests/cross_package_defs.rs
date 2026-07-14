//! Integration tests for cross-package LuaCATS type sharing (#108) — the
//! luals `workspace.library` model over the real `luabox` binary.
//!
//! A dependency package's own `[types] defs` files join the *consumer's*
//! ambient scope automatically: its `---@class` declarations become
//! referenceable and checkable across the package boundary, and its
//! def-declared global APIs get param/return checking, exactly as stdlib/love2d
//! defs do. Cucumber's one-shot `I run` step always runs from the scenario
//! root, but these scenarios need the *consumer* to be a subdirectory with the
//! *dependency* as a sibling (a real path dependency, resolved outside the
//! consumer's own file walk), so they drive the binary directly with an
//! explicit `current_dir`.
//!
//! Boundary (stated for the reader, not tested here): this shares *ambient
//! declarations* — it does NOT type `local geo = require("geometry")` module
//! returns, which is cross-file `require` resolution (#85), out of scope.

// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// Write a file under `root`, creating parent directories.
fn write(root: &Path, rel: &str, content: &str) {
    let full = root.join(rel);
    fs::create_dir_all(full.parent().expect("has parent")).expect("create dirs");
    fs::write(&full, content).unwrap_or_else(|e| panic!("write {rel}: {e}"));
}

/// Run a `luabox` subcommand from `dir`, returning the completed output.
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(args)
        .current_dir(dir)
        .env("LUABOX_STORE", dir.join(".luabox-store"))
        .env_remove("LUABOX_REGISTRY")
        .output()
        .expect("spawn luabox")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The `geometry` dependency: a package whose own `[types] defs` publishes
/// 2D-geometry classes and a def-declared global constructor API. Written as a
/// sibling `dep/` under `temp`, with package name `geometry` so the consumer's
/// `geometry = { path = "../dep" }` mounts it.
fn write_geometry_dep(temp: &Path) {
    write(
        temp,
        "dep/luabox.toml",
        "[package]\nname = \"geometry\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\ndefs = [\"geometry\"]\n",
    );
    write(
        temp,
        "dep/defs/geometry.d.lua",
        "---@meta\n\
         \n---@class geometry.Point\n---@field x number\n---@field y number\n\
         \n---@class geometry.Shape\n---@field area fun(self): number\n\
         \n---@class geometry.Drawable : geometry.Shape\n---@field draw fun(self): string\n\
         \n---@class geometrylib\ngeometry = {}\n\
         \n---@param x number\n---@param y number\n---@return geometry.Point\n\
         function geometry.point(x, y) end\n",
    );
}

/// A consumer manifest at `app/luabox.toml` with a path dependency on the
/// sibling `geometry` package and the given extra `[types] defs` names.
fn write_consumer_manifest(temp: &Path, extra_defs: &[&str]) {
    let defs = if extra_defs.is_empty() {
        String::new()
    } else {
        let list = extra_defs
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("defs = [{list}]\n")
    };
    write(
        temp,
        "app/luabox.toml",
        &format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
             [types]\nstrict = true\n{defs}\n\
             [dependencies]\ngeometry = {{ path = \"../dep\" }}\n"
        ),
    );
}

// (a) a dependency-declared class is referenceable at an annotation position,
//     and a value that violates it is caught with the right type.
#[test]
fn dependency_class_is_referenceable_and_checked() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &[]);
    write(
        temp,
        "app/src/main.lua",
        "---@param p geometry.Point\nlocal function use(p) end\n\
         use({ x = 1, y = 2 })\nuse({ x = 1, y = \"no\" })\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    // The good call is clean; the bad one is a field type mismatch — proof the
    // dep class resolved (no LB0305) and its field types are enforced.
    assert!(so.contains("LB0300"), "expected LB0300; stdout:\n{so}");
    assert!(
        !so.contains("LB0305"),
        "geometry.Point must resolve; stdout:\n{so}"
    );
}

// (b) THE flagship cross-package conformance test (#107 + #108): a consumer
//     carrier that claims a dependency interface but omits a member is flagged.
#[test]
fn cross_package_conformance_flags_missing_member() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &[]);
    write(
        temp,
        "app/src/square.lua",
        "---@class app.Square : geometry.Drawable\n---@field side integer\n\
         local Square = {}\nSquare.__index = Square\n\
         \nfunction Square:area()\n  return self.side\nend\n\
         \nreturn Square\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    // Provides `area` (from geometry.Shape) but not `draw` (geometry.Drawable).
    assert!(so.contains("LB0300"), "expected LB0300; stdout:\n{so}");
    assert!(
        so.contains("draw"),
        "should name the missing member `draw`; stdout:\n{so}"
    );
    assert!(
        so.contains("geometry.Drawable"),
        "should name the dep interface; stdout:\n{so}"
    );
}

// (c) a def-declared global API in the dependency is param-checked at a call
//     site in the consumer, the same way stdlib/love2d defs are.
#[test]
fn dependency_global_api_is_param_checked() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &[]);
    write(
        temp,
        "app/src/main.lua",
        "local ok = geometry.point(1, 2)\nlocal bad = geometry.point(\"no\", 2)\n\
         return { ok, bad }\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    assert!(so.contains("LB0300"), "expected LB0300; stdout:\n{so}");
}

// (d) two packages declaring the same class name → LB0307 warning naming both
//     files, with the deterministic winner (project-local defs win).
#[test]
fn duplicate_class_across_packages_warns_project_wins() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &["dup"]);
    // The consumer's own def redeclares geometry.Point with ONLY `x`. If the
    // project (winner) is used, a `{ x = 1 }` literal is complete; if the
    // dependency's two-field version won, `y` would be reported missing.
    write(
        temp,
        "app/defs/dup.d.lua",
        "---@meta\n\n---@class geometry.Point\n---@field x number\n",
    );
    write(
        temp,
        "app/src/main.lua",
        "---@type geometry.Point\nlocal p = { x = 1 }\nreturn p\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    // Warning only — a collision never fails the command.
    assert!(
        out.status.success(),
        "collision must not fail check; stdout:\n{so}\nstderr:\n{}",
        stderr(&out)
    );
    assert!(so.contains("LB0307"), "expected LB0307; stdout:\n{so}");
    // Both declaring files are named (winner in the note, loser at the span).
    assert!(
        so.contains("defs/dup.d.lua"),
        "should name the project def; stdout:\n{so}"
    );
    assert!(
        so.contains("geometry/defs/geometry.d.lua"),
        "should name the dependency def; stdout:\n{so}"
    );
    // Deterministic winner: project's single-field Point is the one in force,
    // so `{ x = 1 }` is complete — no missing-field diagnostic.
    assert!(
        !so.contains("LB0302"),
        "project decl must win (x-only); stdout:\n{so}"
    );
}

// (e) resolution is one level deep: a dependency-of-a-dependency's defs are NOT
//     visible to the top-level consumer.
#[test]
fn dep_of_dep_defs_are_not_visible() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    // dep2 (transitive) declares `deep.Thing`.
    write(
        temp,
        "dep2/luabox.toml",
        "[package]\nname = \"deep\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\ndefs = [\"deep\"]\n",
    );
    write(
        temp,
        "dep2/defs/deep.d.lua",
        "---@meta\n\n---@class deep.Thing\n---@field v number\n",
    );
    // dep (direct) declares `mid.Node` and itself depends on dep2.
    write(
        temp,
        "dep/luabox.toml",
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\ndefs = [\"mid\"]\n\n\
         [dependencies]\ndeep = { path = \"../dep2\" }\n",
    );
    write(
        temp,
        "dep/defs/mid.d.lua",
        "---@meta\n\n---@class mid.Node\n---@field n number\n",
    );
    write(
        temp,
        "app/luabox.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\n\n[dependencies]\nmid = { path = \"../dep\" }\n",
    );
    // Direct dep's class resolves; transitive dep's class does not (LB0305).
    write(
        temp,
        "app/src/main.lua",
        "---@type mid.Node\nlocal a = { n = 1 }\n\
         ---@type deep.Thing\nlocal b = { v = 2 }\n\
         return { a, b }\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    assert!(
        so.contains("LB0305"),
        "expected LB0305 for the transitive class; stdout:\n{so}"
    );
    assert!(
        so.contains("deep.Thing"),
        "LB0305 must name deep.Thing; stdout:\n{so}"
    );
    assert!(
        !so.contains("mid.Node"),
        "the direct dep's class must resolve; stdout:\n{so}"
    );
}

// (f) with no dependency at all, behaviour is unchanged: a clean consumer
//     checks clean, and an unknown cross-package name is still LB0305.
#[test]
fn no_dependency_is_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write(
        temp,
        "app/luabox.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\n",
    );
    write(
        temp,
        "app/src/main.lua",
        "---@param p geometry.Point\nlocal function use(p) end\nreturn use\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    assert!(
        so.contains("LB0305"),
        "unknown class stays LB0305; stdout:\n{so}"
    );
}

// The lint side (#103/#108): a dependency def's declared globals count as known
// globals, so `undefined-global` does not fire on them in the consumer.
#[test]
fn dependency_globals_are_known_to_lint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &[]);
    write(
        temp,
        "app/luabox.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\n\n\
         [lint]\nundefined-global = \"deny\"\n\n\
         [dependencies]\ngeometry = { path = \"../dep\" }\n",
    );
    // Reads the dependency-declared global `geometry`; a typo'd sibling is not
    // declared anywhere, so it must still be flagged.
    write(
        temp,
        "app/src/main.lua",
        "local a = geometry\nlocal b = gemoetry\nreturn { a, b }\n",
    );
    let out = run(&temp.join("app"), &["lint"]);
    let so = stdout(&out);
    // `geometry` is a known global (no finding for it); the typo `gemoetry` is.
    assert!(
        so.contains("LB0509"),
        "typo'd global should be flagged; stdout:\n{so}"
    );
    assert!(
        so.contains("gemoetry"),
        "the finding should name the typo; stdout:\n{so}"
    );
    assert!(
        !so.contains("undefined global `geometry`"),
        "the dependency global must be known (only the typo is flagged); stdout:\n{so}"
    );
}

// (h) reading a field a dependency class does not declare is `undefined-field`
//     (LB0306, #90) — the strictness rule fires identically for cross-package
//     classes resolved through the dependency's def package.
#[test]
fn reading_undeclared_field_on_dep_class_is_undefined_field() {
    let temp = tempfile::tempdir().expect("tempdir");
    let temp = temp.path();
    write_geometry_dep(temp);
    write_consumer_manifest(temp, &[]);
    // Construct through the dep's declared constructor so the value's type is
    // `geometry.Point` without any table-literal conformance noise.
    write(
        temp,
        "app/src/main.lua",
        "local p = geometry.point(1, 2)\n\
         local good = p.x\nlocal bad = p.nope\nreturn { good, bad }\n",
    );
    let out = run(&temp.join("app"), &["check"]);
    let so = stdout(&out);
    assert!(
        !out.status.success(),
        "expected failure; stderr:\n{}",
        stderr(&out)
    );
    assert!(so.contains("LB0306"), "expected LB0306; stdout:\n{so}");
    assert!(
        so.contains("nope"),
        "should name the missing field; stdout:\n{so}"
    );
    // The dep class must resolve — not an unknown-type-name error.
    assert!(
        !so.contains("LB0305"),
        "geometry.Point must resolve; stdout:\n{so}"
    );
}
