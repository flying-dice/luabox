//! Cucumber acceptance tests — the executable spec (SPEC.md §16.2).
//!
//! Black-box: every scenario drives the real `luabox` binary against a
//! temp-dir fixture project. No internal API shortcuts.

// Cucumber step functions receive owned captures by signature contract.
#![allow(clippy::needless_pass_by_value)]
// test code — panics document assumptions
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::string_slice
)]

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
}

#[given("an empty directory")]
fn empty_directory(_world: &mut AcceptanceWorld) {
    // Each scenario starts with a fresh temp dir; nothing to do.
}

#[given(expr = "I run {string}")]
#[when(expr = "I run {string}")]
fn run_command(world: &mut AcceptanceWorld, command: String) {
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("empty command");
    assert_eq!(program, "luabox", "scenarios drive the luabox binary only");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(parts)
        // Scenarios assert on stderr text. `anyhow` appends a full stack
        // backtrace to every `Error:` line when `RUST_BACKTRACE` is set in
        // the developer's (or CI's) environment, which would leak that
        // environment into the assertions — pin it off so a scenario reads
        // the same everywhere.
        .env("RUST_BACKTRACE", "0")
        .current_dir(world.dir.path())
        .output()
        .expect("failed to spawn luabox");
    world.output = Some(output);
}

/// Exit codes are part of the CLI contract and are not all the same kind of
/// failure: clap rejects a malformed invocation with 2, while a command that
/// ran and reported problems exits 1 (SPEC.md §14).
#[then(expr = "the command exits with code {int}")]
fn command_exits_with_code(world: &mut AcceptanceWorld, expected: i32) {
    let actual = world.output().status.code();
    assert_eq!(
        actual,
        Some(expected),
        "expected exit code {expected}, got {actual:?}\nstderr: {}",
        world.stderr()
    );
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

/// Assert the project root has at least one `*.rockspec` — the package
/// manifest `luabox init`/`new` scaffolds (SPEC.md §6). Used where the
/// scaffolded name (hence the exact rockspec filename) is not fixed.
#[then("a rockspec file exists")]
fn rockspec_file_exists(world: &mut AcceptanceWorld) {
    let found = std::fs::read_dir(world.dir.path())
        .expect("read project root")
        .flatten()
        .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rockspec"));
    assert!(found, "expected a *.rockspec in the project root");
}

#[then(expr = "{string} contains {string}")]
fn file_contains(world: &mut AcceptanceWorld, path: String, needle: String) {
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
    let content = docstring(step);
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

#[then(expr = "stderr does not contain {string}")]
fn stderr_does_not_contain(world: &mut AcceptanceWorld, needle: String) {
    let stderr = world.stderr();
    assert!(
        !stderr.contains(&needle),
        "stderr should not contain `{needle}`; stderr:\n{stderr}"
    );
}

/// Cucumber's `{string}` parameter accepts a `'`-delimited literal as well as
/// a `"`-delimited one, and keeps backslash escapes verbatim either way — so
/// an expected message that itself contains `"` (the checker renders string
/// types and literals with their quotes: ``expected `"on"`, found `"off"` ``)
/// is written with single quotes rather than escaped double ones.
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

/// A `luabox.toml` built from the smallest valid manifest — `[package]` with
/// only the required `edition` — plus one caller-supplied line, so the
/// manifest-validation Examples tables can vary a single key at a time.
/// `section` names the table the line belongs to; `package` extends the
/// existing table rather than repeating its header. Captured with a regex so
/// the line's `"` quotes and TOML braces arrive verbatim.
#[given(regex = r"^a manifest whose \[([a-z-]+)\] table contains '(.*)'$")]
fn manifest_section_containing(world: &mut AcceptanceWorld, section: String, line: String) {
    let manifest = if section == "package" {
        format!("[package]\nedition = \"5.4\"\n{line}\n")
    } else {
        format!("[package]\nedition = \"5.4\"\n\n[{section}]\n{line}\n")
    };
    std::fs::write(world.dir.path().join("luabox.toml"), manifest)
        .expect("failed to write luabox.toml");
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

#[then("zero diagnostics are reported")]
fn zero_diagnostics(world: &mut AcceptanceWorld) {
    let stdout = world.stdout();
    assert!(
        !stdout.contains("LB"),
        "expected no diagnostics at all; stdout:\n{stdout}\nstderr:\n{}",
        world.stderr()
    );
}

/// The machine-readable report contract (`--format json`) must parse.
#[then("stdout is valid JSON")]
fn stdout_is_valid_json(world: &mut AcceptanceWorld) {
    let stdout = world.stdout();
    if let Err(error) = serde_json::from_str::<serde_json::Value>(&stdout) {
        panic!("stdout is not valid JSON: {error}\nstdout:\n{stdout}");
    }
}

#[tokio::main]
async fn main() {
    // @wip gates feature files written ahead of implementation (spec-first,
    // SPEC.md §16.2). Remove the tag when the behaviour ships.
    //
    // `features/lsp/` is driven by the separate `lsp_acceptance` harness — it
    // speaks LSP over stdio and needs its own World — so this CLI harness
    // skips it rather than reporting every one of its steps as unmatched.
    // `fail_on_skipped`: an undefined or ambiguous step is a *failure*,
    // not a quiet skip. Without it a feature file could describe behaviour
    // no step definition implements and the suite would still go green.
    AcceptanceWorld::cucumber()
        .fail_on_skipped()
        .filter_run_and_exit("tests/features", |feature, _rule, scenario| {
            let tagged = |tag: &str| {
                feature.tags.iter().any(|t| t == tag) || scenario.tags.iter().any(|t| t == tag)
            };
            let is_lsp = feature
                .path
                .as_ref()
                .is_some_and(|path| path.components().any(|c| c.as_os_str() == "lsp"));
            !tagged("wip") && !is_lsp
        })
        .await;
}

// --- build fixtures (emit/build.feature — #22) ----------------------------

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

/// Write a manifest with `[build] target` and `bundle = true` — the
/// config-driven half of the unified `luabox build` (flying-dice/luabox#4),
/// so a bare `luabox build` bundles and `--no-bundle` can override it.
#[given(expr = "a project with edition {string} targeting {string} bundling")]
fn project_with_edition_target_bundling(
    world: &mut AcceptanceWorld,
    edition: String,
    target: String,
) {
    let manifest = format!(
        "[package]\n\
         name = \"fixture\"\n\
         version = \"0.1.0\"\n\
         edition = \"{edition}\"\n\
         \n\
         [build]\n\
         target = \"{target}\"\n\
         bundle = true\n"
    );
    std::fs::write(world.dir.path().join("luabox.toml"), manifest)
        .expect("failed to write luabox.toml");
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

/// Counts occurrences in the report rather than asserting mere presence —
/// the difference between "the lowering warned" and "the lowering warned
/// once per lowered construct".
#[then(expr = "stdout contains exactly {int} occurrence of {string}")]
fn stdout_contains_exactly(world: &mut AcceptanceWorld, count: usize, needle: String) {
    let stdout = world.stdout();
    let found = stdout.matches(&needle).count();
    assert_eq!(
        found, count,
        "stdout contains {found} occurrence(s) of `{needle}`, expected {count}; stdout:\n{stdout}"
    );
}

#[then(expr = "stdout does not contain {string}")]
fn stdout_does_not_contain(world: &mut AcceptanceWorld, needle: String) {
    let stdout = world.stdout();
    assert!(
        !stdout.contains(&needle),
        "stdout should not contain `{needle}`; stdout:\n{stdout}"
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
