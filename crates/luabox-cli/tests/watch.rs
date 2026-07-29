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

/// One edit must produce a *bounded* number of reruns and then quiet down.
///
/// The bug this pins (round-4 F2): notify's inotify backend also subscribes
/// to `OPEN`/`CLOSE_NOWRITE`/`ATTRIB`, so a rerun's own **reads** of the
/// watched `*.lua` and `luabox.toml` came back as events and re-triggered the
/// next rerun — forever, at debounce cadence. Measured before the fix: ~5
/// reruns per second, 148 of them over 30 s of an otherwise idle project
/// after a single edit.
///
/// The assertion is deliberately timing-tolerant. It does not demand exactly
/// one rerun — a single save can legitimately produce a couple of batches,
/// and this counts headers, not edits. It asserts only that the count *stops
/// growing* over a quiet window several debounce periods long. With the loop
/// live, that window alone would add ~15 headers, so a small bound separates
/// the two cleanly.
#[test]
fn check_watch_stops_rerunning_once_the_edit_has_settled() {
    /// How long to sit still and count reruns after the first one.
    /// ~15 debounce windows (`watch::DEBOUNCE_WINDOW` is 200 ms).
    const QUIET: Duration = Duration::from_secs(3);
    /// Reruns tolerated *during* the quiet window. The broken loop produced
    /// ~5 per second, so it would blow through this several times over.
    const MAX_RERUNS_WHILE_QUIET: usize = 3;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(
        root.join("luabox.toml"),
        "[package]\nname = \"tmp\"\nversion = \"0.1.0\"\nedition = \"5.4\"\n",
    )
    .expect("write luabox.toml");
    std::fs::write(root.join("main.lua"), "local x = 1\nreturn x\n").expect("write main.lua");

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
    let _stderr = spawn_line_reader(child.stderr.take().expect("piped stderr"));

    let saw_initial_run = wait_for_line(&stdout, Duration::from_secs(15), |line| {
        line.starts_with("watch: ")
    });

    // One edit, then never touch the project again.
    std::fs::write(root.join("main.lua"), "local x = 2\nreturn x\n").expect("rewrite main.lua");

    let saw_rerun = wait_for_line(&stdout, Duration::from_secs(20), |line| {
        line.contains("watching: rerun")
    });

    // Now sit still: everything arriving from here on is the watcher
    // reacting to itself.
    let deadline = Instant::now() + QUIET;
    let mut reruns_while_quiet = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match stdout.recv_timeout(remaining) {
            Ok(line) if line.contains("watching: rerun") => reruns_while_quiet += 1,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(saw_initial_run, "expected the initial run to complete");
    assert!(saw_rerun, "expected a rerun for the one edit");
    assert!(
        reruns_while_quiet <= MAX_RERUNS_WHILE_QUIET,
        "the watcher kept rerunning with nothing changing: {reruns_while_quiet} reruns in \
         {QUIET:?} of quiet (tolerance {MAX_RERUNS_WHILE_QUIET}). That is the self-triggering \
         loop: a rerun's own reads coming back as filesystem events."
    );
}
