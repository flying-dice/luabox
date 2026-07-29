//! The commands that rewrite *your* source must never be able to destroy it
//! (round-8 F1).
//!
//! `luabox fmt` and `luabox lint --fix` replace files the user wrote. They used
//! to do it with `fs::write`, which is `open(O_TRUNC)` then `write`: a write
//! that fails part-way leaves a truncated fragment where the source was, and
//! luabox — the only process that still held the bytes — then exits. The
//! reviewer reproduced it with `ulimit -f 8`: a 97,780-byte source came back as
//! 8,192 bytes, unrecoverable.
//!
//! Cucumber cannot express this: the acceptance harness runs commands that
//! succeed or fail cleanly, and the bug only exists when the *write itself*
//! breaks half-way. So this spawns the real binary under a hard `RLIMIT_FSIZE`
//! — the reviewer's own repro — and asserts the source is byte-identical
//! afterwards.
//!
//! Unix only: `ulimit -f` is the injection, and there is no Windows equivalent
//! that a test can arrange without a filesystem quota. The staging/rename
//! machinery itself is unit tested cross-platform in `crate::atomic_write`.

// test code — panics document assumptions
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// Comfortably past the `ulimit -f 8` ceiling below (8 x 512 B = 4 KiB), so
/// the rewrite cannot possibly complete inside it.
const STATEMENTS: usize = 5_000;

/// A project whose single source is large and deliberately mis-formatted, so
/// `fmt` must rewrite it and `lint --fix` has something to fix on every line.
fn project() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("luabox.toml"),
        "[package]\nname = \"atomic\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    )
    .expect("write luabox.toml");

    let mut source = String::from("local function main()\n");
    for index in 0..STATEMENTS {
        // Mis-spaced (so `fmt` rewrites it) *and* an unused local (so
        // `lint --fix` rewrites it).
        let _ = writeln!(source, "    local    unused_{index}={index}");
    }
    source.push_str("end\nreturn main\n");
    std::fs::create_dir(dir.path().join("src")).expect("mkdir src");
    std::fs::write(dir.path().join("src").join("main.lua"), &source).expect("write main.lua");
    (dir, source)
}

/// Run `luabox <args…>` in `root` with `RLIMIT_FSIZE` clamped to 4 KiB, the
/// way the reviewer did. Returns the child's exit code (`None` if a signal
/// ended it — `SIGXFSZ` is what a plain `write` past the limit raises).
fn under_a_file_size_limit(root: &Path, args: &[&str]) -> Option<i32> {
    let command = format!(
        "ulimit -f 8; exec {} {}",
        shell_quote(env!("CARGO_BIN_EXE_luabox")),
        args.join(" ")
    );
    Command::new("bash")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .output()
        .expect("spawn bash")
        .status
        .code()
}

fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Every file the toolchain would ever look at, under `root/src`.
///
/// The staged temp file is dot-prefixed with a `.tmp` extension precisely so
/// that neither the source walk nor the watcher can see it; if a run is killed
/// by `SIGXFSZ` mid-stage, the best-effort cleanup never runs and one is left
/// behind. That is inert — but it must stay invisible, which is what this
/// checks.
fn visible_sources(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("src"))
        .expect("list src")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

#[test]
fn fmt_cannot_destroy_a_source_it_fails_to_rewrite() {
    let (dir, original) = project();
    let source = dir.path().join("src").join("main.lua");

    let code = under_a_file_size_limit(dir.path(), &["fmt"]);

    // The whole finding: whatever happened to the run, the user's file is
    // exactly as they left it. Before the fix this was 4096 bytes of prefix.
    assert_eq!(
        std::fs::read_to_string(&source).expect("read back"),
        original,
        "`luabox fmt` truncated the source it could not rewrite"
    );
    assert_ne!(code, Some(0), "a rewrite that could not happen must fail");
    assert_eq!(visible_sources(dir.path()), ["main.lua"]);
}

#[test]
fn lint_fix_cannot_destroy_a_source_it_fails_to_rewrite() {
    let (dir, original) = project();
    let source = dir.path().join("src").join("main.lua");

    let code = under_a_file_size_limit(dir.path(), &["lint", "--fix"]);

    assert_eq!(
        std::fs::read_to_string(&source).expect("read back"),
        original,
        "`luabox lint --fix` truncated the source it could not rewrite"
    );
    assert_ne!(code, Some(0), "a rewrite that could not happen must fail");
    assert_eq!(visible_sources(dir.path()), ["main.lua"]);
}

#[test]
fn the_same_run_without_a_limit_does_rewrite_the_source() {
    // Guards both tests above: if the fixture were already canonical, or the
    // project were empty, they would pass without ever attempting a write.
    let (dir, original) = project();
    let source = dir.path().join("src").join("main.lua");

    let status = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .arg("fmt")
        .current_dir(dir.path())
        .output()
        .expect("run `luabox fmt`");

    assert!(status.status.success(), "{status:?}");
    assert_ne!(
        std::fs::read_to_string(&source).expect("read back"),
        original,
        "the fixture must actually need reformatting"
    );
    assert_eq!(visible_sources(dir.path()), ["main.lua"]);
}

#[test]
fn a_symlinked_source_is_still_rewritten_through_the_link() {
    // A prior review pinned "writes go through the link" as behaviour, and
    // the atomic replace renames over the *resolved* path to keep it: renaming
    // over the link itself would silently detach the real source.
    let (dir, original) = project();
    let real = dir.path().join("real.lua");
    std::fs::rename(dir.path().join("src").join("main.lua"), &real).expect("move the real source");
    std::os::unix::fs::symlink("../real.lua", dir.path().join("src").join("main.lua"))
        .expect("symlink");

    let status = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .arg("fmt")
        .current_dir(dir.path())
        .output()
        .expect("run `luabox fmt`");

    assert!(status.status.success(), "{status:?}");
    assert_ne!(
        std::fs::read_to_string(&real).expect("read the real file"),
        original,
        "the bytes must land in the file the link names"
    );
    assert!(
        std::fs::symlink_metadata(dir.path().join("src").join("main.lua"))
            .expect("stat link")
            .file_type()
            .is_symlink(),
        "the link must survive the rewrite"
    );
}
