//! Integration test for `luabox check --watch` (ticket #64).
//!
//! Cucumber is impractical here: the watcher runs forever and the
//! acceptance harness (`tests/acceptance.rs`) drives one-shot commands.
//! Instead this spawns the *real* `luabox` binary against a temp project,
//! waits for the first (immediate) run to finish, touches a watched file,
//! and asserts a `--- watching: rerun ... ---` header shows up within a
//! generous timeout — then kills the process. The debouncer/filter logic
//! itself is unit tested directly in `src/watch.rs` with synthetic event
//! lists; this test is only responsible for proving the real OS watcher +
//! debounce loop wiring works end to end.
//!
//! ## Flakiness (issue #91 — root-caused and fixed)
//!
//! This test used to fail intermittently, and the failure was NOT random
//! event-delivery latency: `watch::run` armed the OS watcher only *after*
//! the first run had already printed `watch: ok`. This test (correctly)
//! treats that line as "the watcher is installed" and writes immediately,
//! so on a loaded machine the rewrite could land in the gap before
//! `notify` had issued `ReadDirectoryChangesW` — an event that is then
//! lost forever, no timeout wide enough to see it. Reproduced
//! deterministically by inserting a 400ms sleep in that gap (3/3
//! failures); fixed by arming the watcher *before* the first run, which
//! makes `watch: ok` a true synchronization barrier (`Watcher::watch`
//! only returns once the OS watch is registered).
//!
//! What remains genuinely timing-dependent is only *how long* delivery
//! takes, never *whether* it happens, so the timeouts below stay generous
//! (15s for the first run, 20s for the rerun) and the assertion only
//! requires *a* rerun header — not a specific line count or timing.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Read lines from `reader` on a background thread and forward them to a
/// channel, so the test can enforce a real wall-clock timeout with
/// `recv_timeout` (a plain `BufRead::lines().next()` call blocks
/// indefinitely and ignores any deadline check around it).
fn spawn_line_reader<R: std::io::Read + Send + 'static>(reader: R) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// Wait up to `timeout` for a line matching `pred`, draining (and
/// discarding) everything that doesn't match along the way.
fn wait_for_line(
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
    pred: impl Fn(&str) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        match rx.recv_timeout(remaining) {
            Ok(line) if pred(&line) => return true,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return false,
        }
    }
}

#[test]
fn check_watch_reruns_on_file_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("luabox.toml"),
        "[package]\nname = \"tmp\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    )
    .expect("write luabox.toml");
    std::fs::write(root.join("main.lua"), "local x = 1\n").expect("write main.lua");

    let bin = env!("CARGO_BIN_EXE_luabox");
    let mut child = Command::new(bin)
        .arg("check")
        .arg("--watch")
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `luabox check --watch`");

    let stdout = spawn_line_reader(child.stdout.take().expect("piped stdout"));
    // Drain stderr so the child never blocks on a full pipe buffer.
    let _stderr = spawn_line_reader(child.stderr.take().expect("piped stderr"));

    // Wait for the immediate first run to finish (`watch::run` prints
    // `watch: ok`/`watch: failed: ...` after every run, including the
    // first) before touching anything, so the watcher is guaranteed to be
    // installed before the write below.
    let saw_initial_run = wait_for_line(&stdout, Duration::from_secs(15), |line| {
        line.starts_with("watch: ")
    });
    assert!(
        saw_initial_run,
        "expected the initial run to complete (a `watch: ...` line on stdout) within 15s"
    );

    std::fs::write(root.join("main.lua"), "local x = 2\n").expect("rewrite main.lua");

    let saw_rerun = wait_for_line(&stdout, Duration::from_secs(20), |line| {
        line.contains("watching: rerun")
    });

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        saw_rerun,
        "expected a `--- watching: rerun (<n> files changed) ---` header after touching \
         main.lua within 20s; see the flakiness note at the top of this file if this fails \
         intermittently rather than consistently"
    );
}
