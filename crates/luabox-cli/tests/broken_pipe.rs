//! `luabox <cmd> | head` must not blow up (round-5 F2).
//!
//! Cucumber can't express this: the acceptance harness collects a command's
//! whole output, and the bug only exists when the *reader* goes away
//! mid-stream. So this spawns the real binary against a project with a report
//! far larger than a pipe buffer, reads a token amount, drops the read end,
//! and asserts the process left quietly.
//!
//! What it is pinning: `println!` panics when the write fails, and a closed
//! pipe makes every write fail with `EPIPE`. Before the fix, `luabox check |
//! head -2` on this project printed a Rust panic plus a backtrace on stderr
//! and exited 101 — and, in the shipped release profile (`panic = "abort"`),
//! died on `SIGABRT`, status 134. Any CI job with `set -o pipefail` and a
//! routine `| head` or `| grep -q` went red for it, in all five `--format`s.
//! A departed reader is now a quiet, successful exit (`crate::emit`).
//!
//! Test-profile note: these run against a `panic = "unwind"` build, so the
//! *pre-fix* failure they would have caught is exit 101 with `panicked` on
//! stderr rather than signal 6. Asserting on both — a clean 0 and no panic
//! text — covers the release shape too, since abort and unwind differ only in
//! how the same panic ends.

// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::io::Read;
use std::process::{Command, Stdio};

/// A project whose report is far larger than the 64 KiB Linux pipe buffer, so
/// the child is guaranteed to still be writing when the reader disappears.
/// `[types] strict = true` makes the findings *errors*, which is what gives
/// the command a nonzero exit code to be distinguished from the quiet one.
fn big_report_project() -> tempfile::TempDir {
    /// 2000 findings ≈ 300 KB of human output, ≈ 800 KB of JSON.
    const FINDINGS: usize = 2000;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("luabox.toml"),
        "[package]\nname = \"big\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n\n\
         [types]\nstrict = true\n",
    )
    .expect("write luabox.toml");

    let mut source = String::from("---@param x number\nlocal function f(x) end\n");
    for index in 0..FINDINGS {
        let _ = writeln!(source, "f(\"bad{index}\")");
    }
    std::fs::write(dir.path().join("main.lua"), source).expect("write main.lua");
    dir
}

/// Run `luabox check --format <format>` in `root`, read `bytes` of stdout,
/// then hang up on it. Returns `(exit code, stderr)`.
fn hang_up_mid_report(root: &std::path::Path, format: &str, bytes: usize) -> (Option<i32>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(["check", "--format", format])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `luabox check`");

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

    let status = child.wait().expect("wait for `luabox check`");
    let stderr = collector.join().expect("stderr collector");
    (status.code(), stderr)
}

#[test]
fn a_reader_that_hangs_up_mid_report_ends_the_command_quietly() {
    let project = big_report_project();
    // Every format goes through the same emitter, and every one of them
    // panicked before the fix, so every one of them is pinned.
    for format in ["human", "json", "sarif", "github", "gitlab"] {
        let (code, stderr) = hang_up_mid_report(project.path(), format, 64);
        assert_eq!(
            code,
            Some(0),
            "`luabox check --format {format} | head` must exit 0 for a reader that left, \
             not {code:?}; stderr was:\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "`luabox check --format {format} | head` panicked instead of exiting quietly:\n{stderr}"
        );
    }
}

#[test]
fn hanging_up_before_a_single_byte_is_read_is_still_quiet() {
    // The `| grep -q needle` shape: the reader can be gone before the first
    // write, not merely part-way through it.
    let project = big_report_project();
    let (code, stderr) = hang_up_mid_report(project.path(), "human", 0);
    assert_eq!(code, Some(0), "stderr was:\n{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn a_reader_that_stays_still_gets_the_whole_report_and_the_real_exit_code() {
    // The other half of the contract: treating `EPIPE` as success must not
    // leak into runs nobody hung up on. This project fails, loudly, in full.
    let project = big_report_project();
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
fn a_closed_stderr_does_not_take_the_command_down_either() {
    // `luabox check 2>&1 | head` puts the diagnostics summary on the same
    // dead pipe as the report; a dead consumer is a dead consumer.
    let project = big_report_project();
    let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .arg("check")
        .current_dir(project.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `luabox check`");
    drop(child.stderr.take().expect("piped stderr"));
    let status = child.wait().expect("wait for `luabox check`");
    // Either the command got to finish normally (exit 1, its own failure) or
    // it found stderr gone and left quietly — never a panic or a signal.
    assert!(
        matches!(status.code(), Some(0 | 1)),
        "a closed stderr must not kill `luabox check`: {status:?}"
    );
}
