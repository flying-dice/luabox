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
    forward_lines(reader, tx);
    rx
}

/// `spawn_line_reader`'s body against a caller-supplied sender, so several
/// readers can feed one channel — `watch::run` prints successes to stdout and
/// failures to stderr, and a test that cares about "whatever the watcher said
/// next" has to see both in one stream.
fn forward_lines<R: std::io::Read + Send + 'static>(reader: R, tx: mpsc::Sender<String>) {
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

/// Collect one phase without assuming stdout and stderr reader ordering.
/// Each marker is a conjunction of substrings on one line. Keep every line
/// across phases so a timeout reports the complete observed transcript.
fn wait_for_markers(
    rx: &mpsc::Receiver<String>,
    timeout: Duration,
    markers: &[&[&str]],
    transcript: &mut Vec<String>,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    let mut seen = vec![false; markers.len()];
    while seen.iter().any(|matched| !matched) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = if remaining.is_zero() {
            None
        } else {
            rx.recv_timeout(remaining).ok()
        };
        let Some(line) = line else {
            let missing: Vec<_> = markers
                .iter()
                .zip(&seen)
                .filter_map(|(marker, seen)| (!seen).then_some(marker))
                .collect();
            return Err(format!(
                "watch did not report {missing:?}\nFull transcript:\n{}",
                transcript.join("\n")
            ));
        };
        for (marker, matched) in markers.iter().zip(&mut seen) {
            *matched |= marker.iter().all(|needle| line.contains(needle));
        }
        transcript.push(line);
    }
    Ok(())
}

#[test]
fn watch_phase_collects_diagnostic_and_verdict_in_either_order() {
    for lines in [
        ["watch: failed: check failed", "broken.lua: LB0001"],
        ["broken.lua: LB0001", "watch: failed: check failed"],
    ] {
        let (tx, rx) = mpsc::channel();
        for line in lines {
            tx.send(line.to_owned()).unwrap();
        }
        drop(tx);
        let mut transcript = Vec::new();
        wait_for_markers(
            &rx,
            Duration::from_secs(1),
            &[&["LB0001"], &["watch: failed:"]],
            &mut transcript,
        )
        .unwrap();
        assert_eq!(transcript, lines);
    }
}

#[test]
fn watch_phase_reports_all_lines_and_rejects_an_unrelated_failure() {
    let (tx, rx) = mpsc::channel();
    tx.send("watch: failed: Lua check failed".into()).unwrap();
    tx.send("other output".into()).unwrap();
    drop(tx);
    let mut transcript = vec!["previous phase".into()];
    let error = wait_for_markers(
        &rx,
        Duration::from_secs(1),
        &[&["watch: failed:", "luabox.toml"]],
        &mut transcript,
    )
    .unwrap_err();
    assert!(error.contains("luabox.toml"));
    assert!(error.contains("previous phase\nwatch: failed: Lua check failed\nother output"));
}

/// File-set and manifest changes must invalidate the project, not merely an
/// existing file's cached syntax. Each assertion waits for the expected verdict.
#[test]
fn check_watch_tracks_create_rename_delete_and_manifest_recovery() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let manifest = root.join("luabox.toml");
    std::fs::write(&manifest, "[package]\nedition = \"5.4\"\n").unwrap();
    std::fs::write(root.join("main.lua"), "print('ok')\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_luabox"))
        .args(["check", "--watch"])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    forward_lines(child.stdout.take().unwrap(), tx.clone());
    forward_lines(child.stderr.take().unwrap(), tx);
    let outcome = std::panic::catch_unwind(|| {
        let mut transcript = Vec::new();
        let mut wait = |markers: &[&[&str]]| {
            if let Err(error) =
                wait_for_markers(&rx, Duration::from_secs(20), markers, &mut transcript)
            {
                panic!("{error}");
            }
        };
        wait(&[&["watch: ok"]]);
        std::fs::write(root.join("broken.lua"), "local x = (\n").unwrap();
        wait(&[&["LB0001"], &["watch: failed:"]]);
        std::fs::rename(root.join("broken.lua"), root.join("renamed.lua")).unwrap();
        wait(&[&["renamed.lua"], &["watch: failed:"]]);
        std::fs::remove_file(root.join("renamed.lua")).unwrap();
        wait(&[&["watch: ok"]]);
        std::fs::write(&manifest, "[package\n").unwrap();
        // A previous parse failure can still arrive from the stderr reader
        // after stdout's success. Demand the manifest-specific failure, not
        // any stale generic verdict from the broken Lua file.
        wait(&[&["watch: failed:", "luabox.toml"]]);
        std::fs::write(&manifest, "[package]\nedition = \"5.4\"\n").unwrap();
        wait(&[&["watch: ok"]]);
    });
    let _ = child.kill();
    let _ = child.wait();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error);
    }
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

/// An edit made shortly after the previous one must be *reported*, not
/// swallowed.
///
/// The bug this pins (round-5 F1, a wave-10 regression): every run was
/// followed by a `drain_self_inflicted` sweep — a sliding 200 ms window that
/// received events and discarded them, added to stop a rerun feeding off its
/// own reads. It could not tell a run's own noise from a save, so an edit
/// landing in that window was thrown away outright: `check --watch` went on
/// reporting `watch: ok` over a tree the user had just broken, forever, and
/// `fmt --watch` silently skipped formatting the file that had just been
/// saved. The kind filter (`watch::triggers_rerun`) is what actually stops
/// the loop; the drain was the unsound half and is gone.
///
/// The shape: edit A, then a *breaking* edit B one debounce-and-a-bit later,
/// so B lands after A's batch has closed but while the old drain was still
/// sweeping. B's failure has to surface. Timing-tolerant by construction — it
/// waits for the failure rather than counting reruns or timing them, and the
/// only way to satisfy it is to actually rerun for B.
#[test]
fn check_watch_reports_an_edit_made_while_the_previous_rerun_was_settling() {
    /// Gap between the two edits. Past `watch::DEBOUNCE_WINDOW` (200 ms), so
    /// B cannot merely join A's batch and pass this test for free — and well
    /// inside the ~200 ms sweep the old drain ran afterwards.
    const EDIT_GAP: Duration = Duration::from_millis(300);

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

    // A failing run is reported on stderr (`watch: failed: ...`) and a
    // successful one on stdout, so both feed one channel here.
    let (tx, output) = mpsc::channel();
    forward_lines(child.stdout.take().expect("piped stdout"), tx.clone());
    forward_lines(child.stderr.take().expect("piped stderr"), tx);

    let saw_initial_run = wait_for_line(&output, Duration::from_secs(15), |line| {
        line.starts_with("watch: ")
    });

    // Edit A: still valid, so the watcher reports `watch: ok` for it.
    std::fs::write(root.join("main.lua"), "local x = 2\nreturn x\n").expect("edit A");
    std::thread::sleep(EDIT_GAP);
    // Edit B: a syntax error. Nothing touches the project after this, so the
    // only thing that can report it is a rerun triggered by B itself.
    std::fs::write(root.join("main.lua"), "local x = = 2\nreturn x\n").expect("edit B");

    let saw_failure = wait_for_line(&output, Duration::from_secs(20), |line| {
        line.contains("watch: failed")
    });

    let _ = child.kill();
    let _ = child.wait();

    assert!(saw_initial_run, "expected the initial run to complete");
    assert!(
        saw_failure,
        "the breaking edit made {EDIT_GAP:?} after the previous one was never reported: \
         `check --watch` is still claiming the tree is fine 20s later. That is an edit \
         swallowed between runs — see this test's doc comment."
    );
}
