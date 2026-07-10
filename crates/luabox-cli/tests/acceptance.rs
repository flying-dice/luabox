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

#[given(regex = r#"^a shape module "(\w+)" declaring (.+)$"#)]
fn shape_module_declaring(world: &mut AcceptanceWorld, name: String, decl: String) {
    write_file(world, &format!("src/{name}.lb"), &format!("{decl}\n"));
}

#[given(regex = r"^a Lua file binding a table (\{.*\}) with ---@struct (\w+)$")]
fn lua_file_binding_table(world: &mut AcceptanceWorld, table: String, struct_name: String) {
    let source = format!("---@use geometry\n\n---@struct {struct_name}\nlocal value = {table}\n");
    write_file(world, "src/main.lua", &source);
}

#[given(regex = r"^trait Shape with fns (\w+) and (\w+)$")]
fn trait_shape_with_fns(world: &mut AcceptanceWorld, first: String, second: String) {
    let module = format!(
        "struct Circle {{ }}\n\
         trait Shape {{\n    fn {first}(self) -> number;\n    fn {second}(self) -> number;\n}}\n"
    );
    write_file(world, "src/geometry.lb", &module);
}

#[given(regex = r"^trait Shape with fn area\(self\) -> number$")]
fn trait_shape_with_area(world: &mut AcceptanceWorld) {
    write_file(
        world,
        "src/geometry.lb",
        "struct Circle { }\ntrait Shape {\n    fn area(self) -> number;\n}\n",
    );
}

#[given(regex = r#"^trait Drawable: Shape in "geometry\.lb"$"#)]
fn trait_drawable_supertrait(world: &mut AcceptanceWorld) {
    write_file(
        world,
        "src/geometry.lb",
        "struct Circle { }\n\
         trait Shape {\n    fn area(self) -> number;\n}\n\
         trait Drawable: Shape {\n    fn draw(self);\n}\n",
    );
}

#[given(regex = r#"^trait Shape in "geometry\.lb"$"#)]
fn trait_shape_in_geometry(world: &mut AcceptanceWorld) {
    write_file(
        world,
        "src/geometry.lb",
        "trait Shape {\n    fn area(self) -> number;\n}\n",
    );
}

#[given(regex = r#"^struct Point in "geometry\.lb"$"#)]
fn struct_point_in_geometry(world: &mut AcceptanceWorld) {
    write_file(
        world,
        "src/geometry.lb",
        "struct Point { x: number, y: number }\n",
    );
}

/// The common carrier preamble: `---@struct Circle` bound to a class table.
const CARRIER_PREAMBLE: &str = "\
---@use geometry

---@struct Circle
local Circle = {}
Circle.__index = Circle
";

#[given(regex = r"^a carrier table with ---@impl Shape for Circle defining only (\w+)$")]
fn carrier_defining_only(world: &mut AcceptanceWorld, only: String) {
    let source = format!(
        "{CARRIER_PREAMBLE}\n---@impl Shape for Circle\nfunction Circle:{only}()\n  return 1\nend\n"
    );
    write_file(world, "src/main.lua", &source);
}

#[given("a carrier table with ---@impl Shape for Circle whose area returns a string")]
fn carrier_area_returns_string(world: &mut AcceptanceWorld) {
    let source = format!(
        "{CARRIER_PREAMBLE}\n\
         ---@impl Shape for Circle\n\
         ---@return string\n\
         function Circle:area()\n  return \"round\"\nend\n"
    );
    write_file(world, "src/main.lua", &source);
}

#[given("a carrier table with ---@impl Drawable for Circle but no Shape impl")]
fn carrier_drawable_without_shape(world: &mut AcceptanceWorld) {
    let source =
        format!("{CARRIER_PREAMBLE}\n---@impl Drawable for Circle\nfunction Circle:draw()\nend\n");
    write_file(world, "src/main.lua", &source);
}

#[given("a carrier table with ---@impl Shape for Circle defining area and an inherent helper")]
fn carrier_with_inherent_helper(world: &mut AcceptanceWorld) {
    let source = format!(
        "{CARRIER_PREAMBLE}\n\
         ---@impl Shape for Circle\n\
         function Circle:area()\n  return 1\nend\n\
         \n\
         function Circle:helper()\n  return 2\nend\n"
    );
    write_file(world, "src/main.lua", &source);
}

#[given("a ---@class annotated table with ---@impl Shape for Square")]
fn class_table_with_impl(world: &mut AcceptanceWorld) {
    let source = "\
---@use geometry

---@class Square
---@field side number
local Square = {}
Square.__index = Square

---@impl Shape for Square
function Square:area()
  return self.side * self.side
end
";
    write_file(world, "src/main.lua", source);
}

#[given("a Lua function annotated ---@param p Point reading p.x")]
fn lua_function_reading_point(world: &mut AcceptanceWorld) {
    let source = "\
---@use geometry

---@param p Point
---@return number
local function get_x(p)
  return p.x
end

get_x({ x = 1, y = 2 })
";
    write_file(world, "src/main.lua", source);
}

#[given("a Lua file with ---@use missing_module")]
fn lua_file_with_missing_use(world: &mut AcceptanceWorld) {
    write_file(world, "src/main.lua", "---@use missing_module\n");
}

#[given("a shape module containing a fn with a body")]
fn shape_module_with_body(world: &mut AcceptanceWorld) {
    write_file(
        world,
        "src/bad.lb",
        "trait Shape {\n    fn area(self) -> number { return 1 }\n}\n",
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
    for file in ["main.lua", "geometry.lb"] {
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
/// SHAPES.md §1 invariant 1: `.lb` shapes never affect emitted output.
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
