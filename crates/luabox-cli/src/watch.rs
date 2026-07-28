//! Reusable watch driver for `luabox check --watch` / `luabox fmt --watch`
//! (SPEC.md §4: watch is machinery shared by check/test/build; test/build
//! don't exist yet, so this crate wires check + fmt now and leaves the
//! driver generic enough to reuse once they land).
//!
//! [`run`] takes a project root and a closure: it runs the closure once
//! immediately, then watches the root recursively and re-runs the closure
//! after every debounced, filtered batch of filesystem changes, forever
//! (Ctrl-C relies on the process's default SIGINT/console-control handler
//! — there is no graceful shutdown to wire up here).
//!
//! ## Debounce
//!
//! A single "save" in most editors produces several raw filesystem events
//! for the same file (a write, a metadata touch, sometimes a temp-file
//! rename dance), and saving several files via "save all" produces one
//! event per file in a tight burst. Reacting to every raw event would
//! rerun the command several times for what the user experienced as one
//! change.
//!
//! The rule: wait for the first relevant event, then keep collecting for
//! [`DEBOUNCE_WINDOW`] (~200ms) *after that first event* — not a sliding
//! window that resets on every new event, which would let a steady trickle
//! of writes (e.g. a build tool touching files every 150ms) postpone the
//! rerun indefinitely. Once the window closes, whatever was collected
//! becomes one batch and the closure reruns once for it.
//!
//! `partition_batches` (test-only) implements this windowing rule as a
//! pure function over a timestamped event log, so the rule itself is unit
//! tested without any real waiting. The live loop in [`run`] performs the
//! same rule against a real channel and real time (recv, then
//! `recv_timeout` until the deadline) — validated end-to-end by the
//! `tests/watch.rs` integration test, since a live OS watcher can't be
//! driven by synthetic events.
//!
//! ## Filtering
//!
//! Only sources that can affect the command's outcome trigger a rerun:
//! `*.lua` and the manifest `luabox.toml`. Everything else is noise and is
//! ignored by [`is_relevant`], which is `luabox_manifest::layout`'s
//! first-party-source rule plus one watch-only decoration:
//! - what the file walk already skips — dot-directories and dot-files
//!   anywhere under the root (`.git/`, `.luabox/`, editor state), the
//!   manifest's `[build]` output directory (generated, not source), and
//!   vendored `lua_modules/` rock trees at every depth. A rerun for a file
//!   the command would not read is a rerun for nothing;
//! - **watch-only**: editor temp/lock files — `*.tmp`, `*~` (Emacs backups),
//!   `.#*` (Emacs lock files — also covered by the dot-file rule above), and
//!   vim's `4913` existence-probe file. These are a *filesystem-event*
//!   concern; they never survive long enough for a walk to see them, so the
//!   shared predicate has no business knowing about them.
//!
//! A manifest (`luabox.toml`) change is not special-cased in the filter —
//! it is deliberately treated as just another relevant file. Re-reading
//! the manifest (edition, strictness, `[build] out`) is the
//! closure's job: `check_cmd::run_once`/`fmt_cmd::run_once` already
//! rediscover the project from scratch on every call.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use luabox_manifest::layout;
use notify::{Event, RecursiveMode, Watcher};

/// How long to keep collecting events after the first one in a batch.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(200);

/// Run `on_change` once immediately, then again after every debounced,
/// filtered batch of filesystem changes under `root`. `out_dir`, if the
/// project has a `[build]` output directory, is excluded from triggering
/// reruns (it's watched at the point `run` is called; if a manifest edit
/// later changes `[build] out`, the *new* files it points to start
/// triggering reruns too, but the *old* directory keeps being ignored
/// until the watcher restarts — a rare edge case, not worth the
/// complexity of re-arming the filter mid-watch).
///
/// A failing `on_change` is reported to stderr; watching continues. This
/// function only returns (`Ok(())`) if the watcher's event channel closes
/// on its own, which in practice doesn't happen — the process exits via
/// the default Ctrl-C handler instead.
pub fn run(
    root: &Path,
    out_dir: Option<&Path>,
    mut on_change: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            // The other end only ever disappears when `run` itself is
            // unwinding (e.g. the caller dropped everything), so a send
            // failure here is not actionable.
            let _ = tx.send(event);
        }
    })?;
    // Arm the watcher *before* the first run, not after. `Watcher::watch`
    // blocks until the OS has genuinely registered the watch (on Windows,
    // until `ReadDirectoryChangesW` has actually been issued — see
    // notify's `ReadDirectoryChangesWatcher::watch_inner`/`add_watch`), so
    // once it returns, a change made anywhere from this point on is
    // guaranteed to be observed. Watching only *after* the first run (as
    // this used to) left a gap between "the caller sees `watch: ok`
    // printed" and "the watcher actually exists" — a change landing in
    // that gap (an editor, or `tests/watch.rs`) would silently never
    // trigger a rerun. This is exactly the race that made the integration
    // test flaky under load: reproduced deterministically by inserting an
    // artificial delay in that old gap (3/3 failures), and gone once the
    // order below was fixed.
    watcher.watch(root, RecursiveMode::Recursive)?;

    let result = on_change();
    // The run above may itself have touched watched files (`fmt --watch`
    // rewrites in place), and the watcher was already armed to catch
    // exactly that. Drain those self-inflicted events before reporting
    // the run as done, so they aren't mistaken for a user edit made
    // afterwards and don't trigger a spurious immediate rerun.
    drain_self_inflicted(&rx);
    report(result);

    while let Some(batch) = next_batch(&rx, root, out_dir) {
        if batch.is_empty() {
            continue;
        }
        println!("--- watching: rerun ({} files changed) ---", batch.len());
        report(on_change());
    }
    Ok(())
}

/// Block until the first event arrives, then keep collecting for
/// [`DEBOUNCE_WINDOW`] measured from *that* event — not a sliding window, see
/// the module docs — and return the batch's relevant, deduped paths
/// ([`filter_and_dedupe`]). An empty `Vec` means the whole batch was noise.
///
/// `None` once the watcher's channel has closed, which is [`run`]'s only exit.
///
/// Split out of [`run`] so the windowing rule can be driven against a real
/// channel and real time in a unit test — `partition_batches` (test-only)
/// models the same rule over a synthetic event log, and the two agreeing is
/// the point.
fn next_batch(
    rx: &mpsc::Receiver<Event>,
    root: &Path,
    out_dir: Option<&Path>,
) -> Option<Vec<PathBuf>> {
    let mut raw = rx.recv().ok()?.paths;
    let deadline = Instant::now() + DEBOUNCE_WINDOW;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(event) => raw.extend(event.paths),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    Some(filter_and_dedupe(raw, root, out_dir))
}

/// Wait out a full [`DEBOUNCE_WINDOW`] of silence on `rx`, resetting on
/// every event received, so that any events already queued (or arriving
/// shortly after) are consumed without triggering anything. Unlike the
/// batching in [`run`]'s own loop (anchored to the first event, not
/// sliding — see the module docs), this must fully settle before treating
/// later events as real changes, since it exists to swallow a run's own
/// side effects rather than to group a burst of unrelated ones.
fn drain_self_inflicted(rx: &mpsc::Receiver<Event>) {
    while rx.recv_timeout(DEBOUNCE_WINDOW).is_ok() {}
}

/// Print a run's outcome. Errors from `on_change` are reported, not
/// propagated — a broken rerun must not kill the watcher.
fn report(result: anyhow::Result<()>) {
    match result {
        Ok(()) => println!("watch: ok"),
        Err(err) => eprintln!("watch: failed: {err:#}"),
    }
}

/// Filter raw event paths down to the ones that should trigger a rerun
/// ([`is_relevant`]), then dedupe while preserving first-seen order (a
/// save commonly fires more than one event for the same path).
fn filter_and_dedupe(raw: Vec<PathBuf>, root: &Path, out_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    raw.into_iter()
        .filter(|p| is_relevant(p, root, out_dir))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Whether a changed path should trigger a rerun: a first-party project
/// source ([`layout::is_project_source`]) or the project's `luabox.toml`, and
/// not an editor temp/lock file.
///
/// The location rules are the file walk's, not this module's — a rerun is only
/// worth doing for a file the command about to rerun would actually read. The
/// manifest is the one relevant file that is not *source*: editing it changes
/// the edition, strictness or `[build] out` the next run reads, so it is
/// matched on [`layout::is_in_project_tree`] instead.
pub(crate) fn is_relevant(path: &Path, root: &Path, out_dir: Option<&Path>) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if is_editor_temp(name) {
        return false;
    }
    if name == "luabox.toml" {
        return layout::is_in_project_tree(path, root, out_dir);
    }
    layout::is_project_source(path, root, out_dir)
}

/// Vim probes whether it can create files in the target directory by
/// writing (then removing) a file literally named `4913`; Emacs writes
/// `.#lock`-style files and `name~` backups. None of these are source
/// changes worth a rerun.
fn is_editor_temp(name: &str) -> bool {
    let is_tmp = Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tmp"));
    name == "4913" || is_tmp || name.ends_with('~') || name.starts_with(".#")
}

/// Partition a timestamp-ordered raw event log into rerun batches: a new
/// batch starts at the first not-yet-batched event and absorbs every
/// subsequent event within `window` of *that* event — see the module docs
/// for why the window is anchored to the batch's first event rather than
/// sliding. Pure and side-effect free so the windowing rule can be unit
/// tested deterministically, without real sleeps.
#[cfg(test)]
pub(crate) fn partition_batches(
    events: &[(Duration, PathBuf)],
    window: Duration,
) -> Vec<Vec<PathBuf>> {
    let mut batches: Vec<Vec<PathBuf>> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let (t0, path0) = &events[i];
        let mut batch = vec![path0.clone()];
        i += 1;
        while i < events.len() && events[i].0 <= *t0 + window {
            batch.push(events[i].1.clone());
            i += 1;
        }
        batches.push(batch);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::{
        DEBOUNCE_WINDOW, drain_self_inflicted, filter_and_dedupe, is_relevant, next_batch,
        partition_batches, report,
    };
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A watcher event naming `paths`, as `notify` would deliver it.
    fn event(paths: &[&str]) -> notify::Event {
        notify::Event {
            paths: paths.iter().map(PathBuf::from).collect(),
            ..notify::Event::default()
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }
    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn partition_single_burst_into_one_batch() {
        let events = vec![
            (ms(0), p("a.lua")),
            (ms(50), p("b.lua")),
            (ms(190), p("c.lua")),
        ];
        let batches = partition_batches(&events, ms(200));
        assert_eq!(batches, vec![vec![p("a.lua"), p("b.lua"), p("c.lua")]]);
    }

    #[test]
    fn partition_splits_events_far_apart() {
        let events = vec![(ms(0), p("a.lua")), (ms(500), p("b.lua"))];
        let batches = partition_batches(&events, ms(200));
        assert_eq!(batches, vec![vec![p("a.lua")], vec![p("b.lua")]]);
    }

    #[test]
    fn partition_window_anchored_to_batch_start_not_sliding() {
        // A steady trickle every 150ms must not extend one batch forever:
        // once past t0+200ms, the next event starts a fresh batch even
        // though it's well within 150ms of the previous event.
        let events = vec![
            (ms(0), p("a.lua")),
            (ms(150), p("b.lua")),
            (ms(300), p("c.lua")),
        ];
        let batches = partition_batches(&events, ms(200));
        assert_eq!(
            batches,
            vec![vec![p("a.lua"), p("b.lua")], vec![p("c.lua")]]
        );
    }

    #[test]
    fn partition_boundary_event_at_exact_window_is_included() {
        let events = vec![(ms(0), p("a.lua")), (ms(200), p("b.lua"))];
        let batches = partition_batches(&events, ms(200));
        assert_eq!(batches, vec![vec![p("a.lua"), p("b.lua")]]);
    }

    #[test]
    fn partition_matches_production_window_constant() {
        // Sanity check that DEBOUNCE_WINDOW itself behaves as documented
        // (200ms), not just an arbitrary `ms(200)` in the tests above.
        let events = vec![(Duration::ZERO, p("a.lua")), (DEBOUNCE_WINDOW, p("b.lua"))];
        assert_eq!(partition_batches(&events, DEBOUNCE_WINDOW).len(), 1);
    }

    #[test]
    fn relevant_lua_and_manifest() {
        let root = Path::new("/proj");
        assert!(is_relevant(Path::new("/proj/src/foo.lua"), root, None));
        assert!(is_relevant(Path::new("/proj/luabox.toml"), root, None));
    }

    #[test]
    fn irrelevant_extension_ignored() {
        let root = Path::new("/proj");
        assert!(!is_relevant(Path::new("/proj/README.md"), root, None));
    }

    #[test]
    fn irrelevant_dot_dir_and_dot_file_ignored() {
        let root = Path::new("/proj");
        assert!(!is_relevant(Path::new("/proj/.git/HEAD"), root, None));
        assert!(!is_relevant(
            Path::new("/proj/.luabox/cache/x.lua"),
            root,
            None
        ));
        assert!(!is_relevant(Path::new("/proj/.hidden.lua"), root, None));
    }

    #[test]
    fn irrelevant_out_dir_ignored() {
        let root = Path::new("/proj");
        let out = Path::new("/proj/dist");
        assert!(!is_relevant(
            Path::new("/proj/dist/bundle.lua"),
            root,
            Some(out)
        ));
        // A same-named file elsewhere is unaffected.
        assert!(is_relevant(
            Path::new("/proj/src/dist.lua"),
            root,
            Some(out)
        ));
    }

    #[test]
    fn irrelevant_vendored_rock_tree_ignored() {
        // The commands `--watch` reruns skip `lua_modules/` entirely, so a
        // `luarocks install` landing files there is not a source change.
        let root = Path::new("/proj");
        assert!(!is_relevant(
            Path::new("/proj/lua_modules/share/lua/5.4/pl/tablex.lua"),
            root,
            None
        ));
        assert!(!is_relevant(
            Path::new("/proj/packages/core/lua_modules/dep/init.lua"),
            root,
            None
        ));
        assert!(!is_relevant(
            Path::new("/proj/lua_modules/dep/luabox.toml"),
            root,
            None
        ));
        // The exclusion is by directory component, as in the walk.
        assert!(is_relevant(
            Path::new("/proj/src/lua_modules.lua"),
            root,
            None
        ));
    }

    #[test]
    fn irrelevant_editor_temp_ignored() {
        let root = Path::new("/proj");
        assert!(!is_relevant(Path::new("/proj/src/foo.lua.tmp"), root, None));
        assert!(!is_relevant(Path::new("/proj/src/foo.lua~"), root, None));
        assert!(!is_relevant(Path::new("/proj/src/.#foo.lua"), root, None));
        assert!(!is_relevant(Path::new("/proj/src/4913"), root, None));
    }

    #[test]
    fn dedupe_preserves_first_seen_order() {
        let root = Path::new("/proj");
        let raw = vec![p("/proj/a.lua"), p("/proj/b.lua"), p("/proj/a.lua")];
        let out = filter_and_dedupe(raw, root, None);
        assert_eq!(out, vec![p("/proj/a.lua"), p("/proj/b.lua")]);
    }

    #[test]
    fn dedupe_drops_irrelevant_paths() {
        let root = Path::new("/proj");
        let raw = vec![p("/proj/a.lua"), p("/proj/README.md"), p("/proj/.git/HEAD")];
        let out = filter_and_dedupe(raw, root, None);
        assert_eq!(out, vec![p("/proj/a.lua")]);
    }

    #[test]
    fn dedupe_of_an_all_irrelevant_batch_is_empty_so_no_rerun_is_triggered() {
        let root = Path::new("/proj");
        let raw = vec![p("/proj/README.md"), p("/proj/src/foo.lua~")];
        assert!(filter_and_dedupe(raw, root, None).is_empty());
    }

    #[test]
    fn a_path_outside_the_watch_root_is_still_judged_on_its_own_name() {
        // `strip_prefix` fails for a path outside the root, so the
        // dot-component rule is skipped and the extension rule decides.
        let root = Path::new("/proj");
        assert!(is_relevant(Path::new("/elsewhere/a.lua"), root, None));
        assert!(!is_relevant(Path::new("/elsewhere/a.md"), root, None));
    }

    #[test]
    fn a_path_with_no_file_name_is_never_relevant() {
        assert!(!is_relevant(Path::new("/"), Path::new("/proj"), None));
    }

    // --- the live batching loop -------------------------------------------
    //
    // `partition_batches` above models the windowing rule over a synthetic
    // log; these drive the real `next_batch` against a real channel, so the
    // rule the watcher actually runs is tested, not just the model of it.

    #[test]
    fn a_burst_of_events_becomes_one_filtered_deduped_batch() {
        let root = Path::new("/proj");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        tx.send(event(&["/proj/a.lua", "/proj/README.md"]))
            .expect("send");
        tx.send(event(&["/proj/b.lua", "/proj/a.lua"]))
            .expect("send");
        drop(tx);

        // Both sends land inside the window from the first, so they merge;
        // the noise is dropped and the repeat collapses to first-seen order.
        let batch = next_batch(&rx, root, None).expect("a batch");
        assert_eq!(batch, vec![p("/proj/a.lua"), p("/proj/b.lua")]);
    }

    #[test]
    fn an_all_noise_batch_comes_back_empty_rather_than_absent() {
        // `run` distinguishes the two: empty means "no rerun, keep waiting",
        // `None` means the watcher is gone and `run` returns.
        let root = Path::new("/proj");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        tx.send(event(&["/proj/README.md"])).expect("send");
        drop(tx);
        assert_eq!(next_batch(&rx, root, None), Some(Vec::new()));
    }

    #[test]
    fn a_closed_channel_ends_the_watch_loop() {
        let (tx, rx) = mpsc::channel::<notify::Event>();
        drop(tx);
        assert_eq!(next_batch(&rx, Path::new("/proj"), None), None);
    }

    #[test]
    fn events_arriving_after_the_window_form_a_separate_batch() {
        let root = Path::new("/proj");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        tx.send(event(&["/proj/a.lua"])).expect("send");
        let sender = std::thread::spawn(move || {
            // Well past the window from the first event, so this cannot be
            // absorbed into the batch above.
            std::thread::sleep(DEBOUNCE_WINDOW * 2);
            let _ = tx.send(event(&["/proj/b.lua"]));
        });

        assert_eq!(next_batch(&rx, root, None), Some(vec![p("/proj/a.lua")]));
        assert_eq!(next_batch(&rx, root, None), Some(vec![p("/proj/b.lua")]));
        sender.join().expect("sender thread");
    }

    #[test]
    fn draining_returns_immediately_once_the_watcher_channel_is_closed() {
        let (tx, rx) = mpsc::channel::<notify::Event>();
        drop(tx);
        // A disconnected channel must not make the drain wait out a full
        // debounce window before the first run is reported.
        let started = Instant::now();
        drain_self_inflicted(&rx);
        assert!(started.elapsed() < DEBOUNCE_WINDOW);
    }

    #[test]
    fn draining_consumes_events_already_queued_by_the_run_itself() {
        let (tx, rx) = mpsc::channel::<notify::Event>();
        // Two self-inflicted events (e.g. `fmt --watch` rewriting files)
        // are already queued when the run finishes.
        tx.send(notify::Event::default()).expect("send");
        tx.send(notify::Event::default()).expect("send");
        drop(tx);

        drain_self_inflicted(&rx);
        assert!(
            rx.try_recv().is_err(),
            "the queue must be empty so no spurious rerun follows"
        );
    }

    #[test]
    fn reporting_a_run_never_propagates_its_failure() {
        // A broken rerun must not kill the watcher: `report` swallows the
        // error after printing it.
        report(Ok(()));
        report(Err(anyhow::anyhow!("check failed with 3 error(s)")));
    }
}
