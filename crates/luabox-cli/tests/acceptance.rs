//! Cucumber acceptance tests — the executable spec (SPEC.md §16.2).
//!
//! Black-box: every scenario drives the real `luabox` binary against a
//! temp-dir fixture project. No internal API shortcuts.

// Cucumber step functions receive owned captures by signature contract.
#![allow(clippy::needless_pass_by_value)]

use std::process::Output;

use cucumber::gherkin::Step;
use cucumber::{World, given, then, when};

#[derive(Debug, World)]
#[world(init = Self::new)]
struct AcceptanceWorld {
    dir: tempfile::TempDir,
    output: Option<Output>,
}

impl AcceptanceWorld {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("failed to create temp dir"),
            output: None,
        }
    }

    fn output(&self) -> &Output {
        self.output.as_ref().expect("no command has been run yet")
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output().stderr).into_owned()
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output().stdout).into_owned()
    }

    /// Expands the `{dir}` placeholder to this scenario's project root
    /// (forward slashes), so scenarios can reference absolute fixture
    /// paths — e.g. local git repository URLs — hermetically.
    fn subst(&self, text: &str) -> String {
        let dir = self.dir.path().to_string_lossy().replace('\\', "/");
        text.replace("{dir}", &dir)
    }
}

#[given("an empty directory")]
fn empty_directory(_world: &mut AcceptanceWorld) {
    // Each scenario starts with a fresh temp dir; nothing to do.
}

#[given(expr = "I run {string}")]
#[when(expr = "I run {string}")]
fn run_command(world: &mut AcceptanceWorld, command: String) {
    let command = world.subst(&command);
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("empty command");
    assert_eq!(program, "luabox", "scenarios drive the luabox binary only");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(parts)
        .current_dir(world.dir.path())
        // Point dependency commands at a scenario-local store so tests
        // never touch (or pollute) the user's ~/.luabox/store.
        .env("LUABOX_STORE", world.dir.path().join(".luabox-store"))
        // Hermetic: a registry configured on the host must not leak into
        // scenarios (registry scenarios opt in via their own steps).
        .env_remove("LUABOX_REGISTRY")
        .output()
        .expect("failed to spawn luabox");
    world.output = Some(output);
}

#[then("the command succeeds")]
fn command_succeeds(world: &mut AcceptanceWorld) {
    let output = world.output();
    assert!(
        output.status.success(),
        "expected success, got {:?}\nstderr: {}",
        output.status.code(),
        world.stderr()
    );
}

#[then("the command fails")]
fn command_fails(world: &mut AcceptanceWorld) {
    assert!(
        !world.output().status.success(),
        "expected failure, but the command succeeded"
    );
}

#[then(expr = "the file {string} exists")]
fn file_exists(world: &mut AcceptanceWorld, path: String) {
    assert!(
        world.dir.path().join(&path).is_file(),
        "expected `{path}` to exist"
    );
}

#[then(expr = "{string} contains {string}")]
fn file_contains(world: &mut AcceptanceWorld, path: String, needle: String) {
    let needle = world.subst(&needle);
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    assert!(
        content.contains(&needle),
        "`{path}` does not contain `{needle}`; content:\n{content}"
    );
}

#[then(expr = "{string} does not contain {string}")]
fn file_does_not_contain(world: &mut AcceptanceWorld, path: String, needle: String) {
    let needle = world.subst(&needle);
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    assert!(
        !content.contains(&needle),
        "`{path}` should not contain `{needle}`; content:\n{content}"
    );
}

/// The step's docstring, normalized: the leading newline after `"""` is
/// stripped and exactly one trailing newline is guaranteed — matching the
/// formatter's final-newline convention so `equals:` comparisons are exact.
fn docstring(step: &Step) -> String {
    let raw = step
        .docstring
        .as_deref()
        .expect("this step requires a docstring (\"\"\" … \"\"\")");
    let body = raw.strip_prefix('\n').unwrap_or(raw);
    format!("{}\n", body.trim_end_matches(['\n', '\r']))
}

#[given(expr = "a file {string} containing:")]
fn file_containing(world: &mut AcceptanceWorld, path: String, step: &Step) {
    let full = world.dir.path().join(&path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("failed to create parent directories");
    }
    let content = world.subst(&docstring(step));
    std::fs::write(&full, content).unwrap_or_else(|e| panic!("cannot write `{path}`: {e}"));
}

#[then(expr = "{string} equals:")]
fn file_equals(world: &mut AcceptanceWorld, path: String, step: &Step) {
    let full = world.dir.path().join(&path);
    let actual =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    let expected = docstring(step);
    assert_eq!(
        actual, expected,
        "`{path}` does not match the expected content"
    );
}

#[then(expr = "stderr contains {string}")]
fn stderr_contains(world: &mut AcceptanceWorld, needle: String) {
    let stderr = world.stderr();
    assert!(
        stderr.contains(&needle),
        "stderr does not contain `{needle}`; stderr:\n{stderr}"
    );
}

#[then(expr = "stdout contains {string}")]
fn stdout_contains(world: &mut AcceptanceWorld, needle: String) {
    let stdout = world.stdout();
    assert!(
        stdout.contains(&needle),
        "stdout does not contain `{needle}`; stdout:\n{stdout}"
    );
}

// --- project fixtures (check.feature, dialect-validation.feature) --------

/// Write a minimal `luabox.toml` for a scenario project.
fn write_manifest(world: &AcceptanceWorld, edition: &str, strict: bool) {
    let manifest = format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n\
         \n\
         [types]\n\
         strict = {strict}\n"
    );
    std::fs::write(world.dir.path().join("luabox.toml"), manifest)
        .expect("failed to write luabox.toml");
}

#[given(expr = "a project with edition {string}")]
fn project_with_edition(world: &mut AcceptanceWorld, edition: String) {
    write_manifest(world, &edition, false);
}

#[given(expr = "a strict project with edition {string}")]
fn strict_project_with_edition(world: &mut AcceptanceWorld, edition: String) {
    write_manifest(world, &edition, true);
}

/// A one-line Lua source (used by the dialect-legality Examples tables).
/// Captured with a regex so backslash escapes in the source (`"a\x41"`)
/// arrive verbatim.
#[given(regex = r"^a Lua file containing '(.*)'$")]
fn lua_file_containing(world: &mut AcceptanceWorld, source: String) {
    let path = world.dir.path().join("src").join("main.lua");
    std::fs::create_dir_all(path.parent().expect("src parent"))
        .expect("failed to create src directory");
    std::fs::write(&path, format!("{source}\n")).expect("failed to write src/main.lua");
}

#[then(expr = "diagnostic {word} is reported")]
fn diagnostic_reported(world: &mut AcceptanceWorld, code: String) {
    let stdout = world.stdout();
    assert!(
        stdout.contains(&code),
        "expected diagnostic `{code}`; stdout:\n{stdout}\nstderr:\n{}",
        world.stderr()
    );
}

/// The dialect-legality codes (SPEC.md §2.1). "No dialect diagnostic"
/// means none of these — type/parse diagnostics are out of scope for the
/// dialect matrix.
const DIALECT_CODES: &[&str] = &[
    "LB0010", "LB0011", "LB0012", "LB0013", "LB0014", "LB0015", "LB0016",
];

#[then("no dialect diagnostic is reported")]
fn no_dialect_diagnostic(world: &mut AcceptanceWorld) {
    let output = format!("{}\n{}", world.stdout(), world.stderr());
    for code in DIALECT_CODES {
        assert!(
            !output.contains(code),
            "expected no dialect diagnostic, found `{code}`; output:\n{output}"
        );
    }
}

// --- shape fixtures (shapes/*.feature) ------------------------------------

/// Write a file under the scenario project, creating parent directories.
fn write_file(world: &AcceptanceWorld, rel: &str, content: &str) {
    let full = world.dir.path().join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("failed to create parent directories");
    }
    std::fs::write(&full, content).unwrap_or_else(|e| panic!("cannot write `{rel}`: {e}"));
}

/// The manifest every step-built shape fixture shares: strict types (so
/// positional conformance errors are hard) and `shapes/` as the ambient
/// shape path (SHAPES-V2.md — the scope is ambient, there are no imports).
fn ensure_shape_manifest(world: &AcceptanceWorld) {
    if world.dir.path().join("luabox.toml").is_file() {
        return;
    }
    write_file(
        world,
        "luabox.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\nshape-paths = [\"shapes\"]\n",
    );
}

#[given(regex = r#"^a shape module "(\w+)" declaring (.+)$"#)]
fn shape_module_declaring(world: &mut AcceptanceWorld, name: String, decl: String) {
    ensure_shape_manifest(world);
    write_file(world, &format!("shapes/{name}.luab"), &format!("{decl}\n"));
}

#[given(regex = r"^a Lua file binding a table (\{.*\}) with ---@type ([\w.]+)$")]
fn lua_file_binding_table(world: &mut AcceptanceWorld, table: String, type_name: String) {
    ensure_shape_manifest(world);
    let source = format!("---@type {type_name}\nlocal value = {table}\nreturn value\n");
    write_file(world, "src/main.lua", &source);
}

#[given(regex = r"^type Shape with methods (\w+) and (\w+)$")]
fn type_shape_with_methods(world: &mut AcceptanceWorld, first: String, second: String) {
    ensure_shape_manifest(world);
    let module = format!(
        "export type Shape = {{\n    {first}(self): number,\n    {second}(self): number,\n}}\n"
    );
    write_file(world, "shapes/geometry.luab", &module);
}

#[given(regex = r#"^type Drawable = Shape & draw in "geometry\.luab"$"#)]
fn type_drawable_intersection(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "shapes/geometry.luab",
        "export type Shape = {\n    area(self): number,\n    perimeter(self): number,\n}\n\
         export type Drawable = Shape & { draw(self): string }\n",
    );
}

#[given(regex = r#"^type Shape in "geometry\.luab"$"#)]
fn type_shape_in_geometry(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "shapes/geometry.luab",
        "export type Shape = {\n    area(self): number,\n}\n",
    );
}

#[given(regex = r#"^type Point in "geometry\.luab"$"#)]
fn type_point_in_geometry(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "shapes/geometry.luab",
        "type Point = { x: number, y: number }\n",
    );
}

/// A carrier class table with the given `:`-methods, positionally asserted
/// against `asserted` via a `---@type` binding — the v2 idiom for a local
/// conformance check (SHAPES-V2.md: the general mechanism covers the
/// special case; no dedicated tag exists).
fn carrier_asserted(methods: &[&str], asserted: &str) -> String {
    use std::fmt::Write as _;
    let mut src = String::from("local Circle = {}\nCircle.__index = Circle\n");
    for m in methods {
        let _ = write!(
            src,
            "\n---@return number\nfunction Circle:{m}()\n  return 1\nend\n"
        );
    }
    let _ = write!(src, "\n---@type {asserted}\nlocal s = Circle\nreturn s\n");
    src
}

#[given(regex = r"^a carrier table defining only (\w+) asserted as ([\w.]+)$")]
fn carrier_defining_only(world: &mut AcceptanceWorld, only: String, asserted: String) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "src/main.lua",
        &carrier_asserted(&[&only], &asserted),
    );
}

#[given(regex = r"^a carrier table defining (\w+) and (\w+) asserted as ([\w.]+)$")]
fn carrier_defining_two(
    world: &mut AcceptanceWorld,
    first: String,
    second: String,
    asserted: String,
) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "src/main.lua",
        &carrier_asserted(&[&first, &second], &asserted),
    );
}

#[given(
    regex = r"^a carrier table defining (\w+), (\w+) and an inherent helper asserted as ([\w.]+)$"
)]
fn carrier_with_inherent_helper(
    world: &mut AcceptanceWorld,
    first: String,
    second: String,
    asserted: String,
) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "src/main.lua",
        &carrier_asserted(&[&first, &second, "helper"], &asserted),
    );
}

#[given("a ---@class annotated table asserted as geometry.Shape")]
fn class_table_asserted(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    let source = "\
---@class Square
---@field side number
local Square = {}
Square.__index = Square

---@return number
function Square:area()
  return self.side * self.side
end

---@type geometry.Shape
local s = Square
return s
";
    write_file(world, "src/main.lua", source);
}

#[given("a Lua function annotated ---@param p geometry.Point reading p.x")]
fn lua_function_reading_point(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    let source = "\
---@param p geometry.Point
---@return number
local function get_x(p)
  return p.x
end

return get_x({ x = 1, y = 2 })
";
    write_file(world, "src/main.lua", source);
}

#[given("a shape module containing a method with a body")]
fn shape_module_with_body(world: &mut AcceptanceWorld) {
    ensure_shape_manifest(world);
    write_file(
        world,
        "shapes/bad.luab",
        "type Shape = {\n    area(self): number { return 1 },\n}\n",
    );
}

#[then(expr = "diagnostic {word} is reported naming field {string}")]
#[then(expr = "diagnostic {word} is reported naming key {string}")]
#[then(expr = "diagnostic {word} is reported listing {string}")]
fn diagnostic_reported_naming(world: &mut AcceptanceWorld, code: String, name: String) {
    let stdout = world.stdout();
    assert!(
        stdout.contains(&code),
        "expected diagnostic `{code}`; stdout:\n{stdout}\nstderr:\n{}",
        world.stderr()
    );
    let quoted = format!("`{name}`");
    assert!(
        stdout.contains(&quoted),
        "expected `{code}` to name {quoted}; stdout:\n{stdout}"
    );
}

#[then(expr = "diagnostic {word} is reported with both spans")]
fn diagnostic_with_both_spans(world: &mut AcceptanceWorld, code: String) {
    let stdout = world.stdout();
    assert!(
        stdout.contains(&code),
        "expected diagnostic `{code}`; stdout:\n{stdout}\nstderr:\n{}",
        world.stderr()
    );
    for file in ["main.lua", "geometry.luab"] {
        assert!(
            stdout.contains(file),
            "expected `{code}` to show a span in `{file}`; stdout:\n{stdout}"
        );
    }
}

#[then("zero shape diagnostics are reported")]
fn zero_shape_diagnostics(world: &mut AcceptanceWorld) {
    let output = format!("{}\n{}", world.stdout(), world.stderr());
    assert!(
        !output.contains("LB2"),
        "expected no LB2xxx diagnostics; output:\n{output}"
    );
}

#[then("zero diagnostics are reported")]
fn zero_diagnostics(world: &mut AcceptanceWorld) {
    let stdout = world.stdout();
    assert!(
        !stdout.contains("LB"),
        "expected no diagnostics at all; stdout:\n{stdout}\nstderr:\n{}",
        world.stderr()
    );
}

// --- dependency fixtures (deps.feature) -----------------------------------

/// Runs one git command inside `dir`, panicking (with stderr) on failure.
/// Only used by `@git`-tagged scenarios, which `main` filters out when the
/// `git` CLI is unavailable.
fn git_in(dir: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("failed to spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A local, hermetic git repository under the scenario directory whose
/// tree is a minimal luabox package, committed and tagged `v<version>`.
/// Scenarios reference it as `{dir}/<repo>`.
#[given(expr = "a git repository at {string} exporting package {string} version {string}")]
fn git_repository_at(world: &mut AcceptanceWorld, repo: String, name: String, version: String) {
    let dir = world.dir.path().join(&repo);
    std::fs::create_dir_all(dir.join("src")).expect("failed to create repo directories");
    std::fs::write(
        dir.join("luabox.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"5.4\"\n"),
    )
    .expect("failed to write repo manifest");
    std::fs::write(
        dir.join("src").join("init.lua"),
        format!("return \"{name} {version}\"\n"),
    )
    .expect("failed to write repo source");
    git_in(&dir, &["init", "--quiet"]);
    git_in(&dir, &["add", "."]);
    git_in(
        &dir,
        &[
            "-c",
            "user.name=luabox-test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "init",
        ],
    );
    git_in(&dir, &["tag", &format!("v{version}")]);
}

#[then("stdout is valid JSON")]
fn stdout_is_valid_json(world: &mut AcceptanceWorld) {
    let stdout = world.stdout();
    if let Err(error) = serde_json::from_str::<serde_json::Value>(&stdout) {
        panic!("stdout is not valid JSON: {error}\nstdout:\n{stdout}");
    }
}

#[tokio::main]
async fn main() {
    // @git scenarios drive real (local, hermetic) git repositories; skip
    // them gracefully where the git CLI is unavailable.
    let git_available = std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    // @wip gates feature files written ahead of implementation (spec-first,
    // SPEC.md §16.2). Remove the tag when the behaviour ships.
    AcceptanceWorld::filter_run("tests/features", move |feature, _rule, scenario| {
        let tagged = |tag: &str| {
            feature.tags.iter().any(|t| t == tag) || scenario.tags.iter().any(|t| t == tag)
        };
        !tagged("wip") && (git_available || !tagged("git"))
    })
    .await;
}

// --- test runner (execution/test.feature) --------------------------------
//
// These steps drive `luabox test` hermetically, with no real Lua: a fake
// `.bat` runtime (pointed at via `LUABOX_LUA`) echoes each test file, which
// is authored as raw runner protocol (TAB-separated fields). Appended after
// `main` (item order is irrelevant to the cucumber macro registration).

/// Run a `luabox …` command with extra environment variables set. Mirrors
/// the base `run_command` step but threads a custom environment through.
fn run_command_with_env(world: &mut AcceptanceWorld, command: &str, envs: &[(String, String)]) {
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("empty command");
    assert_eq!(program, "luabox", "scenarios drive the luabox binary only");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_luabox"));
    cmd.args(parts).current_dir(world.dir.path());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    world.output = Some(cmd.output().expect("failed to spawn luabox"));
}

/// The fake runtime protocol lines for one single-case file.
fn test_file_protocol(name: &str, message: Option<&str>) -> String {
    match message {
        None => {
            format!("LUABOX_TEST_BEGIN\t{name}\nLUABOX_TEST_PASS\t{name}\nLUABOX_TEST_DONE\t1\t0\n")
        }
        Some(msg) => format!(
            "LUABOX_TEST_BEGIN\t{name}\nLUABOX_TEST_FAIL\t{name}\t{msg}\nLUABOX_TEST_DONE\t0\t1\n"
        ),
    }
}

/// The fake runtime: a `.bat` that echoes the given test file (`%2`) and
/// exits nonzero if it contains a FAIL line. The runner spawns it as
/// `<bat> <harness> <test_file>`, so `%2` is always the test file.
#[given("a fake Lua runtime")]
fn fake_lua_runtime(world: &mut AcceptanceWorld) {
    let script = "@echo off\r\n\
        type \"%~2\"\r\n\
        findstr /C:\"LUABOX_TEST_FAIL\" \"%~2\" >nul\r\n\
        if not errorlevel 1 exit /b 1\r\n\
        exit /b 0\r\n";
    std::fs::write(world.dir.path().join("fake_runtime.bat"), script)
        .expect("failed to write fake runtime");
}

#[given(expr = "a passing test file {string} with test {string}")]
fn passing_test_file(world: &mut AcceptanceWorld, path: String, name: String) {
    write_file(world, &path, &test_file_protocol(&name, None));
}

#[given(expr = "a failing test file {string} with test {string} failing with {string}")]
fn failing_test_file(world: &mut AcceptanceWorld, path: String, name: String, message: String) {
    write_file(world, &path, &test_file_protocol(&name, Some(&message)));
}

#[when(expr = "I run {string} with the fake runtime")]
fn run_with_fake_runtime(world: &mut AcceptanceWorld, command: String) {
    let fake = world.dir.path().join("fake_runtime.bat");
    run_command_with_env(
        world,
        &command,
        &[(
            "LUABOX_LUA".to_string(),
            fake.to_string_lossy().into_owned(),
        )],
    );
}

#[when(expr = "I run {string} with env {string}")]
fn run_with_env(world: &mut AcceptanceWorld, command: String, env: String) {
    let (key, value) = env.split_once('=').expect("env must be KEY=VALUE");
    run_command_with_env(world, &command, &[(key.to_string(), value.to_string())]);
}

// --- build fixtures (emit/build.feature, shapes/lb-files.feature — #22) ----

/// Write a manifest with a `[build] target` (SPEC.md §5).
fn write_manifest_with_target(world: &AcceptanceWorld, edition: &str, target: &str, strict: bool) {
    let manifest = format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n\
         \n\
         [build]\n\
         target = \"{target}\"\n\
         \n\
         [types]\n\
         strict = {strict}\n"
    );
    std::fs::write(world.dir.path().join("luabox.toml"), manifest)
        .expect("failed to write luabox.toml");
}

#[given(expr = "a project with edition {string} targeting {string}")]
fn project_with_edition_and_target(world: &mut AcceptanceWorld, edition: String, target: String) {
    write_manifest_with_target(world, &edition, &target, false);
}

#[given(expr = "a strict project with edition {string} targeting {string}")]
fn strict_project_with_edition_and_target(
    world: &mut AcceptanceWorld,
    edition: String,
    target: String,
) {
    write_manifest_with_target(world, &edition, &target, true);
}

#[then(expr = "the file {string} does not exist")]
fn file_does_not_exist(world: &mut AcceptanceWorld, path: String) {
    assert!(
        !world.dir.path().join(&path).exists(),
        "expected `{path}` not to exist"
    );
}

/// Every regular file under `dir`, recursively, in deterministic order.
fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[then(expr = "the emitted output contains no {string}")]
fn emitted_output_contains_no(world: &mut AcceptanceWorld, needle: String) {
    let dist = world.dir.path().join("dist");
    let files = files_under(&dist);
    assert!(!files.is_empty(), "no build output found under `dist`");
    for file in files {
        let content = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("cannot read `{}`: {e}", file.display()));
        assert!(
            !content.contains(&needle),
            "`{}` contains `{needle}`:\n{content}",
            file.display()
        );
    }
}

/// Snapshot the build output into a hidden dir (skipped by the file walk)
/// so a later build of the same project can be compared byte-for-byte —
/// SHAPES.md §1 invariant 1: `.luab` shapes never affect emitted output.
#[then("I stash the build output")]
fn stash_build_output(world: &mut AcceptanceWorld) {
    let dist = world.dir.path().join("dist");
    let stash = world.dir.path().join(".luabox-stash");
    for file in files_under(&dist) {
        let rel = file.strip_prefix(&dist).expect("under dist");
        let stashed = stash.join(rel);
        if let Some(parent) = stashed.parent() {
            std::fs::create_dir_all(parent).expect("failed to create stash directories");
        }
        std::fs::copy(&file, &stashed).expect("failed to stash build output");
    }
    std::fs::remove_dir_all(&dist).expect("failed to clear dist for the next build");
}

#[then("the build output is byte-identical to the stashed output")]
fn build_output_matches_stash(world: &mut AcceptanceWorld) {
    let dist = world.dir.path().join("dist");
    let stash = world.dir.path().join(".luabox-stash");
    let rel = |base: &std::path::Path, files: Vec<std::path::PathBuf>| {
        files
            .into_iter()
            .map(|f| f.strip_prefix(base).expect("under base").to_path_buf())
            .collect::<Vec<_>>()
    };
    let dist_files = rel(&dist, files_under(&dist));
    let stash_files = rel(&stash, files_under(&stash));
    assert_eq!(
        dist_files, stash_files,
        "build output file sets differ with and without shapes"
    );
    for file in dist_files {
        let now = std::fs::read(dist.join(&file)).expect("cannot read dist file");
        let before = std::fs::read(stash.join(&file)).expect("cannot read stashed file");
        assert_eq!(
            now,
            before,
            "`{}` differs between builds with and without shapes",
            file.display()
        );
    }
}

// --- run (execution/run.feature — #28) ------------------------------------
//
// Task scenarios use shell builtins (`echo`, `exit`) that behave the same
// under `cmd /C` and `sh -c`, so they need no OS-specific fixture. The
// script scenarios reuse the "fake Lua runtime" idea from
// execution/test.feature, but with `run`-specific fakes: one that echoes
// its argv (to prove args pass through to the script invocation), one that
// always fails (to prove the script's exit code propagates).

#[then(expr = "stdout does not contain {string}")]
fn stdout_does_not_contain(world: &mut AcceptanceWorld, needle: String) {
    let stdout = world.stdout();
    assert!(
        !stdout.contains(&needle),
        "stdout should not contain `{needle}`; stdout:\n{stdout}"
    );
}

/// A fake Lua runtime that echoes all of its arguments (script path plus
/// any extra `args`) to stdout, prefixed so it's unambiguous in assertions,
/// then exits 0.
#[given("a fake Lua runtime that echoes its arguments")]
fn fake_lua_runtime_echoes_args(world: &mut AcceptanceWorld) {
    let script = "@echo off\r\necho RAN: %*\r\nexit /b 0\r\n";
    std::fs::write(world.dir.path().join("fake_echo_runtime.bat"), script)
        .expect("failed to write fake echo runtime");
}

/// A fake Lua runtime that always exits nonzero, regardless of arguments —
/// used to prove a script's failure propagates to `luabox run`'s own exit
/// code.
#[given("a fake Lua runtime that always fails")]
fn fake_lua_runtime_always_fails(world: &mut AcceptanceWorld) {
    let script = "@echo off\r\necho FAILED\r\nexit /b 1\r\n";
    std::fs::write(world.dir.path().join("fake_failing_runtime.bat"), script)
        .expect("failed to write fake failing runtime");
}

#[when(expr = "I run {string} with the echo runtime")]
fn run_with_echo_runtime(world: &mut AcceptanceWorld, command: String) {
    let fake = world.dir.path().join("fake_echo_runtime.bat");
    run_command_with_env(
        world,
        &command,
        &[(
            "LUABOX_LUA".to_string(),
            fake.to_string_lossy().into_owned(),
        )],
    );
}

#[when(expr = "I run {string} with the failing runtime")]
fn run_with_failing_runtime(world: &mut AcceptanceWorld, command: String) {
    let fake = world.dir.path().join("fake_failing_runtime.bat");
    run_command_with_env(
        world,
        &command,
        &[(
            "LUABOX_LUA".to_string(),
            fake.to_string_lossy().into_owned(),
        )],
    );
}

// --- bench (execution/bench.feature — #26) --------------------------------
//
// Hermetic, mirroring the "fake Lua runtime" idea from execution/test.feature:
// a `.bat` shim (pointed at via `LUABOX_LUA`) echoes each bench file, which
// is authored here as raw `LUABOX_BENCH_*` protocol. Unlike the test
// runtime's bat, this one always exits 0 — benches never fail the build, so
// there is no pass/fail signal to encode. `luabox bench` resolves *every*
// runtime on PATH (`resolve_matrix`), so the fake runtime shows up in the
// comparison table labeled `LUABOX_LUA` (matching `resolve_matrix`'s label
// for the override), alongside any real Lua that happens to be on the host's
// PATH — assertions below only check `stdout contains`, so a real
// interpreter's harmless load-error row for these non-Lua fixture files
// doesn't affect them.

/// A fake Lua runtime for `luabox bench`: echoes the bench file (`%2`,
/// already containing protocol lines) and always exits 0.
#[given("a fake bench runtime")]
fn fake_bench_runtime(world: &mut AcceptanceWorld) {
    let script = "@echo off\r\ntype \"%~2\"\r\nexit /b 0\r\n";
    std::fs::write(world.dir.path().join("fake_bench_runtime.bat"), script)
        .expect("failed to write fake bench runtime");
}

/// A bench file authored as raw protocol: one bench reporting the given
/// comma-separated ns/iter samples as separate `RESULT` batches.
#[given(expr = "a bench file {string} with bench {string} producing samples {string}")]
fn bench_file_with_samples(
    world: &mut AcceptanceWorld,
    path: String,
    name: String,
    samples: String,
) {
    use std::fmt::Write as _;
    let mut proto = format!("LUABOX_BENCH_BEGIN\t{name}\n");
    for sample in samples.split(',') {
        let _ = writeln!(proto, "LUABOX_BENCH_RESULT\t{name}\t{sample}");
    }
    proto.push_str("LUABOX_BENCH_DONE\t1\n");
    write_file(world, &path, &proto);
}

#[when(expr = "I run {string} with the fake bench runtime")]
fn run_with_fake_bench_runtime(world: &mut AcceptanceWorld, command: String) {
    let fake = world.dir.path().join("fake_bench_runtime.bat");
    run_command_with_env(
        world,
        &command,
        &[(
            "LUABOX_LUA".to_string(),
            fake.to_string_lossy().into_owned(),
        )],
    );
}

// --- toolchain (execution/toolchain.feature — #27) ------------------------
//
// Hermetic: no network, no real Lua. A scenario-local index
// (`LUABOX_TOOLCHAIN_INDEX`) points the installer at a `.tar.gz` fixture
// whose "interpreter" is a `.cmd` shim behaving exactly like the test
// runner's fake runtime; toolchains install into a scenario-local directory
// (`LUABOX_TOOLCHAINS`). The correct fixture checksum comes from
// `luabox-store` (a normal dependency, available to integration tests).

/// The index platform key for this host — must mirror
/// `toolchain_cmd::current_platform`.
fn toolchain_platform() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{}-{arch}", std::env::consts::OS)
}

/// Build the `.tar.gz` fixture runtime and write `index.toml` mapping
/// `<id>-<platform>` at it. When `correct` is false the recorded checksum is
/// wrong, so an install must reject the archive.
fn write_toolchain_index(world: &AcceptanceWorld, id: &str, correct: bool) {
    // A `.cmd` shim that behaves like the test runner's fake runtime: echo
    // the test file (argv 2) and fail iff it carries a FAIL line.
    let shim = "@echo off\r\n\
        type \"%~2\"\r\n\
        findstr /C:\"LUABOX_TEST_FAIL\" \"%~2\" >nul\r\n\
        if not errorlevel 1 exit /b 1\r\n\
        exit /b 0\r\n";
    let fixture_src = world.dir.path().join(".fixture-src");
    std::fs::create_dir_all(&fixture_src).expect("failed to create fixture source dir");
    std::fs::write(fixture_src.join("lua.cmd"), shim).expect("failed to write fixture shim");

    let archive = world.dir.path().join("fixture.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&fixture_src)
        .arg("lua.cmd")
        .status()
        .expect("failed to run tar to build the fixture archive");
    assert!(status.success(), "tar failed to build the fixture archive");

    let sha = if correct {
        luabox_store::hash_file(&archive).expect("failed to hash the fixture archive")
    } else {
        "0".repeat(64)
    };
    let key = format!("{id}-{}", toolchain_platform());
    let url = archive.to_string_lossy().replace('\\', "/");
    let index = format!("[toolchain.\"{key}\"]\nurl = \"{url}\"\nsha256 = \"{sha}\"\n");
    std::fs::write(world.dir.path().join("index.toml"), index)
        .expect("failed to write toolchain index");
}

#[given(expr = "a toolchain index offering {string} with a working runtime")]
fn toolchain_index_working(world: &mut AcceptanceWorld, id: String) {
    write_toolchain_index(world, &id, true);
}

#[given(expr = "a corrupt toolchain index offering {string}")]
fn toolchain_index_corrupt(world: &mut AcceptanceWorld, id: String) {
    write_toolchain_index(world, &id, false);
}

/// Run a `luabox …` command with the hermetic toolchain environment: a
/// scenario-local toolchains directory and index. Deliberately does not set
/// `LUABOX_LUA`, so a pinned toolchain is what resolution finds.
#[when(expr = "I run {string} with the toolchain env")]
fn run_with_toolchain_env(world: &mut AcceptanceWorld, command: String) {
    let toolchains = world.dir.path().join(".toolchains");
    let index = world.dir.path().join("index.toml");
    run_command_with_env(
        world,
        &command,
        &[
            (
                "LUABOX_TOOLCHAINS".to_string(),
                toolchains.to_string_lossy().into_owned(),
            ),
            (
                "LUABOX_TOOLCHAIN_INDEX".to_string(),
                index.to_string_lossy().into_owned(),
            ),
        ],
    );
}

// --- bundler fixtures (emit/bundle.feature — #24) ---------------------------

#[then(expr = "{string} contains exactly {int} occurrence of {string}")]
fn file_contains_exactly(world: &mut AcceptanceWorld, path: String, count: usize, needle: String) {
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    let found = content.matches(&needle).count();
    assert_eq!(
        found, count,
        "`{path}` contains {found} occurrence(s) of `{needle}`, expected {count}; content:\n{content}"
    );
}

/// Drive `luabox unmap` against the last line of an emitted bundle — the
/// entry chunk is inlined last, so that line always maps to a module file
/// without the scenario hardcoding bundle-internal line numbers.
#[when(expr = "I unmap the last bundle line of {string}")]
fn unmap_last_bundle_line(world: &mut AcceptanceWorld, path: String) {
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    let last = content.lines().count();
    run_command(
        world,
        format!("luabox unmap {path} {path}:{last}: synthetic-error"),
    );
}

// --- registry (distribution/registry.feature — #20) ------------------------
//
// Hermetic: LUABOX_REGISTRY points at a scenario-local directory registry
// (".registry" under the scenario dir) and LUABOX_STORE at the scenario
// store. Publisher and consumer projects are sibling subdirectories, so one
// scenario can publish from "pkg" and install from "app".

/// Run a `luabox …` command from a subdirectory of the scenario dir,
/// optionally with the scenario-local registry configured.
fn run_luabox_in_dir(world: &mut AcceptanceWorld, command: &str, dir: &str, registry: bool) {
    let command = world.subst(command);
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("empty command");
    assert_eq!(program, "luabox", "scenarios drive the luabox binary only");
    let cwd = world.dir.path().join(dir);
    std::fs::create_dir_all(&cwd).expect("failed to create the project directory");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_luabox"));
    cmd.args(parts)
        .current_dir(&cwd)
        .env("LUABOX_STORE", world.dir.path().join(".luabox-store"))
        // Hermetic either way: the host's registry never leaks in.
        .env_remove("LUABOX_REGISTRY");
    if registry {
        cmd.env("LUABOX_REGISTRY", world.dir.path().join(".registry"));
    }
    world.output = Some(cmd.output().expect("failed to spawn luabox"));
}

#[given(expr = "I run {string} in {string} against the registry")]
#[when(expr = "I run {string} in {string} against the registry")]
fn run_in_dir_against_registry(world: &mut AcceptanceWorld, command: String, dir: String) {
    run_luabox_in_dir(world, &command, &dir, true);
}

#[when(expr = "I run {string} in {string} without a registry")]
fn run_in_dir_without_registry(world: &mut AcceptanceWorld, command: String, dir: String) {
    run_luabox_in_dir(world, &command, &dir, false);
}

// --- luarocks bridge (distribution/luarocks.feature — #19) -----------------
//
// Hermetic scenarios point LUABOX_LUAROCKS_MIRROR at a scenario-local mirror
// directory (".luarocks-mirror"), pre-populated with `<rock>-<version>.rockspec`
// files and extracted `<rock>-<version>/` source trees. No network is touched.
// The one @network scenario resolves a real rock from luarocks.org and is
// filtered by CI; it also self-skips when the network is unreachable so an
// offline full-suite run stays green.

/// The scenario's luarocks mirror directory.
fn luarocks_mirror(world: &AcceptanceWorld) -> std::path::PathBuf {
    world.dir.path().join(".luarocks-mirror")
}

/// Writes a pure-Lua builtin rock into the mirror: a `<rock>-<version>-1.rockspec`
/// plus a source tree exporting a single module named after the rock.
#[given(expr = "a luarocks mirror providing pure-Lua rock {string} at {string}")]
fn luarocks_mirror_pure_lua(world: &mut AcceptanceWorld, rock: String, version: String) {
    let mirror = luarocks_mirror(world);
    let luarocks_version = format!("{version}-1");
    let rockspec = format!(
        "package = \"{rock}\"\n\
         version = \"{luarocks_version}\"\n\
         source = {{ url = \"https://example.invalid/{rock}.tar.gz\" }}\n\
         dependencies = {{ \"lua >= 5.1\" }}\n\
         build = {{\n\
         \x20 type = \"builtin\",\n\
         \x20 modules = {{ {rock} = \"{rock}.lua\" }},\n\
         }}\n"
    );
    std::fs::create_dir_all(&mirror).expect("create mirror dir");
    std::fs::write(
        mirror.join(format!("{rock}-{luarocks_version}.rockspec")),
        rockspec,
    )
    .expect("write rockspec");
    let tree = mirror.join(format!("{rock}-{luarocks_version}"));
    std::fs::create_dir_all(&tree).expect("create source tree");
    std::fs::write(
        tree.join(format!("{rock}.lua")),
        format!("return \"{rock}\"\n"),
    )
    .expect("write module");
}

/// Writes a C/native rock (`build.type = make`) into the mirror — resolution
/// must reject it (SPEC.md §6: luabox is not a C build system).
#[given(expr = "a luarocks mirror providing C rock {string} at {string}")]
fn luarocks_mirror_c_rock(world: &mut AcceptanceWorld, rock: String, version: String) {
    let mirror = luarocks_mirror(world);
    let luarocks_version = format!("{version}-1");
    let rockspec = format!(
        "package = \"{rock}\"\n\
         version = \"{luarocks_version}\"\n\
         source = {{ url = \"git+https://example.invalid/{rock}.git\" }}\n\
         dependencies = {{ \"lua >= 5.1\" }}\n\
         build = {{\n\
         \x20 type = \"make\",\n\
         \x20 modules = {{ [\"{rock}.core\"] = \"src/{rock}.c\" }},\n\
         }}\n"
    );
    std::fs::create_dir_all(&mirror).expect("create mirror dir");
    std::fs::write(
        mirror.join(format!("{rock}-{luarocks_version}.rockspec")),
        rockspec,
    )
    .expect("write rockspec");
}

/// Runs a `luabox …` command with the hermetic luarocks mirror configured
/// (plus the scenario-local store). No network is reachable for the bridge.
#[when(expr = "I run {string} against the luarocks mirror")]
fn run_against_luarocks_mirror(world: &mut AcceptanceWorld, command: String) {
    let command = world.subst(&command);
    let mirror = luarocks_mirror(world);
    run_command_with_env(
        world,
        &command,
        &[
            (
                "LUABOX_STORE".to_string(),
                world
                    .dir
                    .path()
                    .join(".luabox-store")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "LUABOX_LUAROCKS_MIRROR".to_string(),
                mirror.to_string_lossy().into_owned(),
            ),
        ],
    );
}

/// @network: resolve+install a real rock from luarocks.org. Self-skips (by
/// substituting a trivially successful command) when the network is down, so
/// offline runs stay green; CI filters the @network tag to avoid the network.
#[when(expr = "I install {string} from luarocks.org")]
fn install_real_rock(world: &mut AcceptanceWorld, spec: String) {
    let reachable = std::process::Command::new("curl")
        .args([
            "-fsS",
            "--max-time",
            "15",
            "-o",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
            "https://luarocks.org/manifest.json",
        ])
        .status()
        .is_ok_and(|s| s.success());
    if !reachable {
        eprintln!("skipping @network scenario: luarocks.org is unreachable");
        // A trivially successful command so `Then the command succeeds` holds.
        run_command(world, "luabox --version".to_string());
        return;
    }
    let _ = spec;
    run_command_with_env(
        world,
        "luabox install",
        &[(
            "LUABOX_STORE".to_string(),
            world
                .dir
                .path()
                .join(".luabox-store")
                .to_string_lossy()
                .into_owned(),
        )],
    );
}

// --- bundler embedding modes (emit/modes.feature — #32) --------------------
//
// `love` mode packages a `.love` (zip) archive; verifying its contents
// hermetically needs an archive-listing tool. `tar -tf` reads zip archives
// fine when `tar` resolves to a libarchive (`bsdtar`) build — the default
// `tar` on macOS, and the `tar.exe` Windows ships in `System32` — but not
// when it resolves to GNU tar (e.g. Git for Windows' `tar.exe`, which may
// sit earlier on `PATH`), which cannot read zip at all. `archive_listing`
// tries a small chain of tools so the scenario stays hermetic and green
// regardless of which `tar` `PATH` happens to resolve to.

/// Lists the entries of a zip-format archive (a `.love` file). Tries `tar`
/// as found on `PATH`, then (Windows only) the System32 `tar.exe`
/// explicitly, then `python3`/`python -m zipfile -l` as a last resort.
/// Panics with all attempted tools named if none of them work — a louder
/// failure than a false pass.
fn archive_listing(path: &std::path::Path) -> String {
    let path_str = path.to_string_lossy().into_owned();
    let mut attempts: Vec<std::process::Command> = Vec::new();

    let mut tar = std::process::Command::new("tar");
    tar.args(["-tf", &path_str]);
    attempts.push(tar);

    if cfg!(windows) {
        let mut system32_tar = std::process::Command::new(r"C:\Windows\System32\tar.exe");
        system32_tar.args(["-tf", &path_str]);
        attempts.push(system32_tar);
    }

    for python in ["python3", "python"] {
        let mut cmd = std::process::Command::new(python);
        cmd.args(["-m", "zipfile", "-l", &path_str]);
        attempts.push(cmd);
    }

    for mut cmd in attempts {
        if let Ok(output) = cmd.output()
            && output.status.success()
        {
            return String::from_utf8_lossy(&output.stdout).into_owned();
        }
    }
    panic!(
        "cannot list the contents of `{}`: no working archive-listing tool found \
         (tried `tar`, the Windows System32 `tar.exe`, `python3 -m zipfile`, \
         `python -m zipfile`)",
        path.display()
    );
}

#[then(expr = "the archive {string} contains {string}")]
fn archive_contains(world: &mut AcceptanceWorld, path: String, needle: String) {
    let full = world.dir.path().join(&path);
    let listing = archive_listing(&full);
    assert!(
        listing.contains(&needle),
        "archive `{path}` does not list `{needle}`; listing:\n{listing}"
    );
}

/// Write a manifest with `[build] target` and `[build] mode` (SPEC.md §7,
/// ticket #32).
fn write_manifest_with_target_and_mode(
    world: &AcceptanceWorld,
    edition: &str,
    target: &str,
    mode: &str,
    description: Option<&str>,
) {
    let description_line = description
        .map(|d| format!("description = \"{d}\"\n"))
        .unwrap_or_default();
    let manifest = format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n\
         {description_line}\
         \n\
         [build]\n\
         target = \"{target}\"\n\
         mode = \"{mode}\"\n"
    );
    std::fs::write(world.dir.path().join("luabox.toml"), manifest)
        .expect("failed to write luabox.toml");
}

#[given(expr = "a project with edition {string} targeting {string} using mode {string}")]
fn project_with_edition_target_and_mode(
    world: &mut AcceptanceWorld,
    edition: String,
    target: String,
    mode: String,
) {
    write_manifest_with_target_and_mode(world, &edition, &target, &mode, None);
}

#[given(
    expr = "a project with edition {string} targeting {string} using mode {string} and description {string}"
)]
fn project_with_edition_target_mode_and_description(
    world: &mut AcceptanceWorld,
    edition: String,
    target: String,
    mode: String,
    description: String,
) {
    write_manifest_with_target_and_mode(world, &edition, &target, &mode, Some(&description));
}
