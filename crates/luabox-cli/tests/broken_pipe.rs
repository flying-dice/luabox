//! `luabox <cmd> | head` must not blow up — and must not lie (round-5 F2,
//! round-8 F2).
//!
//! Cucumber can't express either half: the acceptance harness collects a
//! command's whole output, and both defects only exist when the *reader* goes
//! away mid-stream. So this spawns the real binary against a project with a
//! report far larger than a pipe buffer, reads a token amount, drops the read
//! end, and asserts on what the process left behind.
//!
//! **The crash (round-5).** `println!` panics when the write fails, and a
//! closed pipe makes every write fail with `EPIPE`. Before that fix, `luabox
//! check | head -2` on this project printed a Rust panic plus a backtrace on
//! stderr and exited 101 — and, in the shipped release profile
//! (`panic = "abort"`), died on `SIGABRT`, status 134. Any CI job with
//! `set -o pipefail` and a routine `| head` or `| grep -q` went red for it, in
//! all five `--format`s.
//!
//! **The lie (round-8).** The first fix exited **0** on a departed reader,
//! unconditionally. So `set -o pipefail; luabox check | head -1` over a tree
//! with thousands of errors *succeeded*, and a CI gate reported green over a
//! broken tree. A truncated report now costs the output and nothing else: the
//! exit status is still the verdict the run reached (`crate::emit`).
//!
//! Both halves are pinned here, in both directions — a failing run piped to an
//! early reader exits 1, a *clean* run piped to the same reader exits 0 — for
//! `check`, `lint` and `fmt --check`, the three commands that record a verdict.
//!
//! Test-profile note: these run against a `panic = "unwind"` build, so the
//! round-5 failure they would have caught is exit 101 with `panicked` on
//! stderr rather than signal 6. Asserting on both — the expected code and no
//! panic text — covers the release shape too, since abort and unwind differ
//! only in how the same panic ends.

// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::io::Read;
use std::process::{Command, Stdio};

/// How many findings each fixture below carries. 2000 is ≈ 300 KB of human
/// output and ≈ 800 KB of JSON — comfortably past the 64 KiB Linux pipe
/// buffer, which is what makes the child guaranteed to still be writing when
/// the reader disappears.
const FINDINGS: usize = 2000;

/// A project whose `check` report is far larger than a pipe buffer.
///
/// `strict` decides the *verdict* without changing the report's size: strict
/// makes every finding an error (the command fails), warn mode makes the same
/// findings warnings (the command succeeds). That is exactly the pair the
/// round-8 finding is about — the truncated output is identical, the exit
/// status must not be.
fn check_project(strict: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let types = if strict {
        "\n[types]\nstrict = true\n"
    } else {
        ""
    };
    std::fs::write(
        dir.path().join("luabox.toml"),
        format!("[package]\nname = \"big\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n{types}"),
    )
    .expect("write luabox.toml");

    let mut source = String::from("---@param x number\nlocal function f(x) end\n");
    for index in 0..FINDINGS {
        let _ = writeln!(source, "f(\"bad{index}\")");
    }
    std::fs::write(dir.path().join("main.lua"), source).expect("write main.lua");
    dir
}

/// A project whose `lint` report is far larger than a pipe buffer.
///
/// `deny` decides the verdict the same way: an `---@luabox-ignore` without a
/// reason is a correctness-tier finding (LB0500, an error), while a plain
/// unused local is style-tier (a warning, exit 0).
fn lint_project(deny: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("luabox.toml"),
        "[package]\nname = \"big\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    )
    .expect("write luabox.toml");

    let mut source = String::new();
    for index in 0..FINDINGS {
        if deny {
            let _ = writeln!(source, "---@luabox-ignore unused-local");
        }
        let _ = writeln!(source, "local unused_{index} = {index}");
    }
    source.push_str("return 0\n");
    std::fs::write(dir.path().join("main.lua"), source).expect("write main.lua");
    dir
}

/// A project whose `fmt --check` report is far larger than a pipe buffer: one
/// long `would reformat …` line per file. `formatted` decides the verdict.
fn fmt_project(formatted: bool) -> tempfile::TempDir {
    /// Enough long-named files that the listing out-runs a pipe buffer.
    const FILES: usize = 1400;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("luabox.toml"),
        "[package]\nname = \"big\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    )
    .expect("write luabox.toml");
    let src = dir.path().join("src");
    std::fs::create_dir(&src).expect("mkdir src");

    let body = if formatted {
        "local x = 1\nprint(x)\n"
    } else {
        "local    x=1\nprint( x )\n"
    };
    for index in 0..FILES {
        let name = format!("a_source_file_with_a_deliberately_long_name_{index:04}.lua");
        std::fs::write(src.join(name), body).expect("write source");
    }
    dir
}

/// Run `luabox <args…>` in `root`, read `bytes` of stdout, then hang up on it.
/// Returns `(exit code, stderr)`.
fn hang_up_mid_report(
    root: &std::path::Path,
    args: &[&str],
    bytes: usize,
) -> (Option<i32>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(args)
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn luabox");

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // Drain stderr on a thread: the summary is small, but a test that
    // deadlocks on a full stderr pipe would be worse than the bug.
    let collector = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let mut taken = vec![0u8; bytes];
    let mut filled = 0;
    while filled < bytes {
        match stdout.read(&mut taken[filled..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => filled += n,
        }
    }
    // The hang-up itself. Every write the child makes from here on fails.
    drop(stdout);

    let status = child.wait().expect("wait for luabox");
    let stderr = collector.join().expect("stderr collector");
    (status.code(), stderr)
}

/// Assert `luabox <args…>` piped to a reader that takes `bytes` and leaves
/// exits with `expected`, without panicking on the way out.
fn assert_hang_up(root: &std::path::Path, args: &[&str], bytes: usize, expected: i32) {
    let (code, stderr) = hang_up_mid_report(root, args, bytes);
    assert_eq!(
        code,
        Some(expected),
        "`luabox {} | head` must exit {expected} for a reader that left, not {code:?}; \
         stderr was:\n{stderr}",
        args.join(" ")
    );
    assert!(
        !stderr.contains("panicked"),
        "`luabox {}` panicked instead of leaving quietly:\n{stderr}",
        args.join(" ")
    );
}

#[test]
fn a_failing_check_piped_to_an_early_reader_still_reports_the_failure() {
    // The round-8 finding: `set -o pipefail; luabox check | head -1` over a
    // broken tree must not report success. Every format goes through the same
    // emitter, and every one of them exited 0 before the fix, so every one of
    // them is pinned.
    let project = check_project(true);
    for format in ["human", "json", "sarif", "github", "gitlab"] {
        assert_hang_up(project.path(), &["check", "--format", format], 64, 1);
    }
}

#[test]
fn a_clean_check_piped_to_an_early_reader_still_reports_success() {
    // The other direction, and the reason the verdict is *recorded* rather
    // than "EPIPE means failure": the same enormous report, all warnings, is a
    // passing run — and a departed reader must not invent a failure.
    let project = check_project(false);
    for format in ["human", "json", "sarif", "github", "gitlab"] {
        assert_hang_up(project.path(), &["check", "--format", format], 64, 0);
    }
}

#[test]
fn a_failing_lint_piped_to_an_early_reader_still_reports_the_failure() {
    let project = lint_project(true);
    assert_hang_up(project.path(), &["lint"], 64, 1);
}

#[test]
fn a_clean_lint_piped_to_an_early_reader_still_reports_success() {
    let project = lint_project(false);
    assert_hang_up(project.path(), &["lint"], 64, 0);
}

#[test]
fn a_failing_fmt_check_piped_to_an_early_reader_still_reports_the_failure() {
    // `fmt --check` prints a plain list rather than diagnostics, so it records
    // its own verdict — a separate wire that needs its own pin.
    let project = fmt_project(false);
    assert_hang_up(project.path(), &["fmt", "--check"], 64, 1);
}

#[test]
fn a_clean_fmt_check_piped_to_an_early_reader_still_reports_success() {
    let project = fmt_project(true);
    assert_hang_up(project.path(), &["fmt", "--check"], 64, 0);
}

#[test]
fn hanging_up_before_a_single_byte_is_read_still_carries_the_verdict() {
    // The `| grep -q needle` shape: the reader can be gone before the first
    // write, not merely part-way through it.
    assert_hang_up(check_project(true).path(), &["check"], 0, 1);
    assert_hang_up(check_project(false).path(), &["check"], 0, 0);
}

#[test]
fn a_command_whose_report_precedes_an_unconditional_success_still_exits_zero() {
    // `luabox schema` prints one embedded document and cannot fail; it records
    // no verdict, so the default 0 is what a departed reader gets. This is the
    // case that would break if "EPIPE" were mapped onto a failure outright.
    let dir = tempfile::tempdir().expect("tempdir");
    assert_hang_up(dir.path(), &["schema"], 0, 0);
}

#[test]
fn a_reader_that_stays_still_gets_the_whole_report_and_the_real_exit_code() {
    // The other half of the contract: the EPIPE handling must not leak into
    // runs nobody hung up on. This project fails, loudly, in full.
    let project = check_project(true);
    let output = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .arg("check")
        .current_dir(project.path())
        .output()
        .expect("run `luabox check`");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a project with errors must still fail when the report is fully read"
    );
    assert!(
        output.stdout.len() > 64 * 1024,
        "the fixture must out-run a pipe buffer for the tests above to mean anything, \
         got {} bytes",
        output.stdout.len()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("2000 errors"), "{stderr}");
}

#[test]
fn every_fixture_out_runs_a_pipe_buffer() {
    // Guards all six hang-up tests above: if a fixture's report fitted in the
    // 64 KiB pipe buffer, the child would finish writing before the reader
    // left and the tests would pass without ever exercising `EPIPE`.
    for (args, project) in [
        (vec!["check"], check_project(false)),
        (vec!["lint"], lint_project(false)),
        (vec!["fmt", "--check"], fmt_project(false)),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_luabox"))
            .args(&args)
            .current_dir(project.path())
            .output()
            .expect("run luabox");
        assert!(
            output.stdout.len() > 64 * 1024,
            "`luabox {}` produced only {} bytes",
            args.join(" "),
            output.stdout.len()
        );
    }
}

#[test]
fn a_closed_stderr_does_not_take_the_command_down_either() {
    // `luabox check 2>&1 | head` puts the diagnostics summary on the same
    // dead pipe as the report; a dead consumer is a dead consumer.
    let project = check_project(true);
    let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .arg("check")
        .current_dir(project.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `luabox check`");
    drop(child.stderr.take().expect("piped stderr"));
    let status = child.wait().expect("wait for `luabox check`");
    // The summary line is written after the report, by which point the verdict
    // is recorded — so either way out is exit 1, its own failure, never a
    // panic or a signal.
    assert_eq!(
        status.code(),
        Some(1),
        "a closed stderr must not turn a failing check into a pass: {status:?}"
    );
}
