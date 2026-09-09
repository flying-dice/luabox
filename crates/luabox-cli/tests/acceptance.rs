//! Cucumber acceptance tests — the executable spec (SPEC.md §16.2).
//!
//! Black-box: every scenario drives the real `luabox` binary against a
//! temp-dir fixture project. No internal API shortcuts.
//!
//! Which binary is [`support::luabox_bin`]'s call: the cargo-built one by
//! default, or whatever `LUABOX_E2E_BIN` points at — which is how `release.yml`
//! runs this same suite against the binary its install script pulled out of the
//! draft release, before that release is allowed to go live.

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

/// Fixture writers shared with the `lsp_acceptance` harness. Cucumber binds a
/// step attribute to one `World`, so the `#[given]` shims below stay here;
/// only their bodies are shared.
mod support;

use support::{docstring, luabox_bin, package_table, write_file};

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

/// Run the `luabox` binary in the scenario's project directory with an
/// explicit `RUST_BACKTRACE` value.
///
/// Scenarios assert on stderr text, so the variable is always set rather than
/// inherited: whatever the developer (or CI) exports must not leak into the
/// assertions. `"0"` is the default; the one scenario that pins the
/// no-backtrace-leak contract sets `"1"` instead.
fn run_luabox(world: &mut AcceptanceWorld, command: &str, backtrace: &str) {
    let mut parts = command.split_whitespace();
    let program = parts.next().expect("empty command");
    assert_eq!(program, "luabox", "scenarios drive the luabox binary only");
    let output = std::process::Command::new(luabox_bin())
        .args(parts)
        .env("RUST_BACKTRACE", backtrace)
        .current_dir(world.dir.path())
        .output()
        .expect("failed to spawn luabox");
    world.output = Some(output);
}

#[given(expr = "I run {string}")]
#[when(expr = "I run {string}")]
fn run_command(world: &mut AcceptanceWorld, command: String) {
    run_luabox(world, &command, "0");
}

/// The same invocation with `RUST_BACKTRACE=1` — the environment a developer
/// debugging something else already has exported. A failing command must
/// render the same error chain either way, with no backtrace appended.
#[when(expr = "I run {string} with RUST_BACKTRACE set")]
fn run_command_with_backtrace(world: &mut AcceptanceWorld, command: String) {
    run_luabox(world, &command, "1");
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

#[given(expr = "a file {string} containing:")]
fn file_containing(world: &mut AcceptanceWorld, path: String, step: &Step) {
    write_file(world.dir.path(), &path, &docstring(step));
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

/// Ordering, not just presence: the merged legality passes must render in
/// source order, whichever dialect run produced each finding (Shockwave
/// round 4 — the target's verdict used to print before the edition's).
#[then(expr = "stdout contains {string} before {string}")]
fn stdout_contains_in_order(world: &mut AcceptanceWorld, first: String, second: String) {
    let stdout = world.stdout();
    let at_first = stdout
        .find(&first)
        .unwrap_or_else(|| panic!("stdout does not contain `{first}`; stdout:\n{stdout}"));
    let at_second = stdout
        .find(&second)
        .unwrap_or_else(|| panic!("stdout does not contain `{second}`; stdout:\n{stdout}"));
    assert!(
        at_first < at_second,
        "expected `{first}` before `{second}`; stdout:\n{stdout}"
    );
}

// --- project fixtures (check.feature, dialect-validation.feature) --------

#[given(expr = "a project with edition {string}")]
fn project_with_edition(world: &mut AcceptanceWorld, edition: String) {
    support::write_manifest(world.dir.path(), &edition, false);
}

#[given(expr = "a strict project with edition {string}")]
fn strict_project_with_edition(world: &mut AcceptanceWorld, edition: String) {
    support::write_manifest(world.dir.path(), &edition, true);
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
    write_file(world.dir.path(), "luabox.toml", &manifest);
}

/// A one-line Lua source (used by the dialect-legality Examples tables).
/// Captured with a regex so backslash escapes in the source (`"a\x41"`)
/// arrive verbatim.
#[given(regex = r"^a Lua file containing '(.*)'$")]
fn lua_file_containing(world: &mut AcceptanceWorld, source: String) {
    write_file(world.dir.path(), "src/main.lua", &format!("{source}\n"));
}

/// A Lua file whose first bytes are a UTF-8 byte-order mark.
///
/// The mark gets its own step rather than living in the docstring: written
/// there it would be an invisible character in the `.feature` file, which
/// editors and `git` normalization add and strip at will — exactly the kind
/// of accident these scenarios exist to pin down.
#[given(expr = "a Lua file with a UTF-8 BOM containing:")]
fn lua_file_with_bom(world: &mut AcceptanceWorld, step: &Step) {
    write_file(
        world.dir.path(),
        "src/main.lua",
        &format!("\u{feff}{}", docstring(step)),
    );
}

/// The same, for any path — a *dependency* carrying a byte-order mark,
/// which the bundler has to cut out of the middle of its output.
#[given(expr = "a file {string} with a UTF-8 BOM containing:")]
fn file_with_bom(world: &mut AcceptanceWorld, path: String, step: &Step) {
    write_file(
        world.dir.path(),
        &path,
        &format!("\u{feff}{}", docstring(step)),
    );
}

/// Assert on a file's *first* bytes. Position is the whole point for a `#!`
/// line: a shebang anywhere but byte 0 is not a shebang, so a "contains"
/// assertion would pass on output no kernel would honour.
#[then(expr = "{string} starts with {string}")]
fn file_starts_with(world: &mut AcceptanceWorld, path: String, prefix: String) {
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    assert!(
        content.starts_with(&prefix),
        "`{path}` does not start with `{prefix}`; it starts:\n{}",
        content.chars().take(120).collect::<String>()
    );
}

/// No mark anywhere in the file — not just not at byte 0. Spelled as a step
/// rather than as `does not contain "<U+FEFF>"` for the same reason the step
/// above exists: an invisible character in a `.feature` file is an accident
/// waiting to be normalized away.
#[then(expr = "{string} carries no UTF-8 byte-order mark")]
fn file_has_no_bom(world: &mut AcceptanceWorld, path: String) {
    let full = world.dir.path().join(&path);
    let content =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    assert!(
        !content.contains('\u{feff}'),
        "`{path}` carries a byte-order mark; content:\n{content}"
    );
}

/// The counterpart assertion: the file's content is the docstring with a
/// leading UTF-8 BOM, byte for byte.
#[then(expr = "{string} equals, with a leading UTF-8 BOM:")]
fn file_equals_with_bom(world: &mut AcceptanceWorld, path: String, step: &Step) {
    let full = world.dir.path().join(&path);
    let actual =
        std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("cannot read `{path}`: {e}"));
    let expected = format!("\u{feff}{}", docstring(step));
    assert_eq!(
        actual, expected,
        "`{path}` does not match the expected content"
    );
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

/// The control-flow legality codes (#44): unresolved `goto`, repeated label,
/// `break` outside a loop. "No control-flow diagnostic" means none of these —
/// a program may still be rejected for a syntax or type reason.
const CONTROL_FLOW_CODES: &[&str] = &["LB0020", "LB0021", "LB0022"];

#[then("no control-flow diagnostic is reported")]
fn no_control_flow_diagnostic(world: &mut AcceptanceWorld) {
    let output = format!("{}\n{}", world.stdout(), world.stderr());
    for code in CONTROL_FLOW_CODES {
        assert!(
            !output.contains(code),
            "expected no control-flow diagnostic, found `{code}`; output:\n{output}"
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

/// The issues in a `--format gitlab` report, parsed.
fn gitlab_issues(stdout: &str) -> Vec<serde_json::Value> {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout is not a GitLab report: {e}\nstdout:\n{stdout}"))
}

/// GitLab keys a finding to a place in the merge-request diff through
/// `location.lines.begin`, so the report has to name the line the diagnostic
/// is actually on — a contract `stdout contains` cannot express.
#[then(expr = "the gitlab report places a finding for {string} on line {int}")]
fn gitlab_finding_on_line(world: &mut AcceptanceWorld, path: String, line: u64) {
    let stdout = world.stdout();
    let issues = gitlab_issues(&stdout);
    assert!(
        issues.iter().any(|issue| {
            issue["location"]["path"].as_str() == Some(path.as_str())
                && issue["location"]["lines"]["begin"].as_u64() == Some(line)
        }),
        "no finding for `{path}` on line {line}; stdout:\n{stdout}"
    );
}

/// The negative half of the contract: line 1 was the placeholder every finding
/// used to collapse onto, so a fixture with nothing on line 1 must show none.
#[then(expr = "no gitlab finding sits on line {int}")]
fn no_gitlab_finding_on_line(world: &mut AcceptanceWorld, line: u64) {
    let stdout = world.stdout();
    let issues = gitlab_issues(&stdout);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["location"]["lines"]["begin"].as_u64() == Some(line)),
        "a finding unexpectedly sits on line {line}; stdout:\n{stdout}"
    );
}

/// GitLab's Code Quality parser rejects an issue with a missing or empty
/// required field — `location.path: ""` above all — so a report can be valid
/// JSON, look plausible, and be thrown away whole. `stdout is valid JSON`
/// cannot see that; this asserts the format's actual contract.
#[then("the gitlab report satisfies the code quality schema")]
fn gitlab_report_is_schema_valid(world: &mut AcceptanceWorld) {
    const SEVERITIES: [&str; 5] = ["info", "minor", "major", "critical", "blocker"];
    let stdout = world.stdout();
    let issues = gitlab_issues(&stdout);
    assert!(!issues.is_empty(), "an empty report proves nothing");
    for issue in &issues {
        for field in ["description", "check_name", "fingerprint"] {
            let value = issue[field].as_str().unwrap_or_else(|| {
                panic!("`{field}` is missing or not a string in {issue}\nstdout:\n{stdout}")
            });
            assert!(!value.is_empty(), "`{field}` is empty in {issue}");
        }
        let severity = issue["severity"].as_str().unwrap_or_default();
        assert!(
            SEVERITIES.contains(&severity),
            "`{severity}` is not a GitLab severity in {issue}"
        );
        let path = issue["location"]["path"].as_str().unwrap_or_default();
        assert!(
            !path.is_empty(),
            "`location.path` is empty — GitLab drops this issue: {issue}"
        );
        let begin = issue["location"]["lines"]["begin"].as_u64().unwrap_or(0);
        assert!(begin >= 1, "`location.lines.begin` is {begin} in {issue}");
    }
}

/// Fingerprints are GitLab's identity for a finding: one per issue, and two
/// issues must never share one or the report silently loses a finding.
#[then("every gitlab fingerprint is distinct")]
fn gitlab_fingerprints_are_distinct(world: &mut AcceptanceWorld) {
    let stdout = world.stdout();
    let issues = gitlab_issues(&stdout);
    let mut prints: Vec<&str> = issues
        .iter()
        .map(|issue| issue["fingerprint"].as_str().unwrap_or_default())
        .collect();
    let total = prints.len();
    prints.sort_unstable();
    prints.dedup();
    assert_eq!(
        prints.len(),
        total,
        "{} of {total} fingerprints collide — GitLab keeps one issue per \
         fingerprint, so the rest are dropped; stdout:\n{stdout}",
        total - prints.len()
    );
}

/// Every machine format has to carry a finding's **severity** faithfully,
/// whatever the command's own exit code did with it: `lint` exits 0 on a
/// warn-tier finding, so a CI consumer that wants to gate on warnings can
/// only do so by reading the severity back out of the report.
///
/// One step over the three JSON-shaped formats, since the contract is one
/// contract and only the spelling of the two fields differs.
#[then(expr = "the {word} report marks {string} as {string}")]
fn report_marks_severity(world: &mut AcceptanceWorld, format: String, code: String, level: String) {
    let stdout = world.stdout();
    let value: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstdout:\n{stdout}"));
    // (the code field, the severity field, the findings) per format.
    let (code_key, level_key, findings) = match format.as_str() {
        "json" => ("code", "severity", value.as_array().cloned()),
        "gitlab" => ("check_name", "severity", value.as_array().cloned()),
        "sarif" => (
            "ruleId",
            "level",
            value["runs"][0]["results"].as_array().cloned(),
        ),
        other => panic!("no severity contract defined for the `{other}` format"),
    };
    let findings = findings.unwrap_or_else(|| panic!("no findings array\nstdout:\n{stdout}"));
    let matching: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|f| f[code_key].as_str() == Some(code.as_str()))
        .collect();
    assert!(
        !matching.is_empty(),
        "no `{code}` finding in the {format} report; stdout:\n{stdout}"
    );
    for finding in matching {
        assert_eq!(
            finding[level_key].as_str(),
            Some(level.as_str()),
            "`{code}` is not reported as `{level}` in {finding}"
        );
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
        "{}\n[build]\ntarget = \"{target}\"\n\n[types]\nstrict = {strict}\n",
        package_table(edition)
    );
    write_file(world.dir.path(), "luabox.toml", &manifest);
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
        "{}\n[build]\ntarget = \"{target}\"\nbundle = true\n",
        package_table(&edition)
    );
    write_file(world.dir.path(), "luabox.toml", &manifest);
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

/// A ceiling on the *width* of the report, which is how the human renderer's
/// long-line windowing is observable from outside: without it a diagnostic on
/// a 400-character line printed that whole line plus a 400-character caret
/// indent, and n diagnostics on one long line cost O(n x line) of output.
#[then(expr = "no line of stdout is longer than {int} characters")]
fn stdout_lines_bounded(world: &mut AcceptanceWorld, max: usize) {
    let stdout = world.stdout();
    for line in stdout.lines() {
        let width = line.chars().count();
        assert!(
            width <= max,
            "a {width}-character line exceeds the {max}-character bound:\n{line}"
        );
    }
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
    run_luabox(
        world,
        &format!("luabox unmap {path} {path}:{last}: synthetic-error"),
        "0",
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
        "{}{description_line}\n[build]\ntarget = \"{target}\"\nmode = \"{mode}\"\n",
        package_table(edition)
    );
    write_file(world.dir.path(), "luabox.toml", &manifest);
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

#[then(expr = "entry {string} in archive {string} contains {string}")]
fn archive_entry_contains(
    world: &mut AcceptanceWorld,
    entry: String,
    archive: String,
    needle: String,
) {
    let path = world.dir.path().join(&archive);
    for python in ["python3", "python"] {
        let output = std::process::Command::new(python)
            .args(["-c", "import sys, zipfile; sys.stdout.buffer.write(zipfile.ZipFile(sys.argv[1]).read(sys.argv[2]))"])
            .arg(&path)
            .arg(&entry)
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let text = String::from_utf8(output.stdout).expect("UTF-8 Lua archive entry");
            assert!(
                text.contains(&needle),
                "entry {entry} of {archive} does not contain {needle}: {text}"
            );
            return;
        }
    }
    panic!(
        "cannot read {entry} from {archive}; a working Python zipfile implementation is required"
    );
}
