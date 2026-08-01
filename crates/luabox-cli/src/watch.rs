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
//! The two agreeing is not a claim, it is an **assertion**: `BATCHING_CASES`
//! (test-only) is one table of timed event logs, and
//! `the_model_and_the_live_loop_batch_every_case_identically` feeds every case
//! to *both* — `partition_batches` instantly, [`next_batch`] over a real
//! channel fed by a sender thread on a stretched window — and requires the same
//! batches out of each. A model that drifted from the loop it models would be
//! worse than no model at all, so it has to be caught, not hoped for. (The
//! window is a parameter of [`next_batch`] for exactly this: the live leg runs
//! it long enough that scheduler jitter cannot move an event across a boundary.
//! Production always passes [`DEBOUNCE_WINDOW`].)
//!
//! ## Filtering
//!
//! Two filters, and both are load-bearing.
//!
//! **By event kind** ([`triggers_rerun`]) — only events that describe a
//! *change* count. This is not a nicety: notify's inotify backend watches
//! `OPEN | CLOSE_NOWRITE | ATTRIB` alongside the mutation events, so every
//! rerun's own *reads* of `*.lua` and `luabox.toml` come straight back as
//! events. Forwarding those made one edit enough to pin the watcher in a
//! permanent rerun loop at debounce cadence — measured at ~99 reruns over
//! 30 s of an otherwise idle project, with not a single MODIFY among them.
//!
//! **By path** ([`is_relevant`]) — only sources that can affect the
//! command's outcome:
//! `*.lua` and the manifest `luabox.toml`. Everything else is noise and is
//! ignored by [`is_relevant`], which is `luabox_manifest::layout`'s
//! first-party-source rule plus one watch-only decoration:
//! - what the file walk already skips — dot-directories and dot-files
//!   anywhere under the root (`.git/`, `.luabox/`, editor state), the
//!   manifest's `[build]` output directory (generated, not source), and
//!   vendored `lua_modules/` trees, with one carve-out: `.lua` files under
//!   the root's own `lua_modules/share/lua/<X.Y>/` ARE relevant, because
//!   since #30 the rerun *reads* them for the rock type harvest
//!   ([`layout::is_rock_source`]). A rerun for a file the command would not
//!   read is a rerun for nothing — and a rock install is a file it does;
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
//!
//! ## Self-inflicted events, and why there is no post-run drain
//!
//! A rerun reads (and, for `fmt --watch`, rewrites) the very files being
//! watched, so its own filesystem activity is a candidate trigger for the
//! *next* rerun. That used to be handled twice: by the kind filter above,
//! and — belt and braces — by a post-run *drain*, a sliding 200ms window
//! that received events and threw them away.
//!
//! The drain was unsound and has been deleted. It could not tell a run's own
//! noise from a user's save, so an edit landing in that window was discarded
//! outright: `check --watch` sat on a stale `watch: ok` over a tree the user
//! had just broken, and `fmt --watch` silently skipped formatting the file
//! that had just been saved. Reproduced deterministically at a ~300ms edit
//! gap, and by an IDE "save all" spreading five files ~120ms apart.
//!
//! [`triggers_rerun`] alone is enough, because on every backend luabox ships
//! a binary for, a *read* is not reported as a change at all:
//!
//! | backend | platform | what a read produces | verdict |
//! | --- | --- | --- | --- |
//! | inotify | linux | `IN_OPEN` / `IN_ACCESS` / `IN_CLOSE_NOWRITE`, i.e. `Access(_)` | filtered out |
//! | `FSEvents` | macOS | nothing — the API reports content and directory changes, not opens | nothing to filter |
//! | `ReadDirectoryChangesW` | windows | nothing — notify subscribes to name, attribute, size, write, creation and security changes, never `FILE_NOTIFY_CHANGE_LAST_ACCESS` | nothing to filter |
//!
//! What a run *writes* does still trigger a rerun, and that is correct rather
//! than a loop: `fmt` only writes a file whose formatting actually changes,
//! so a rewrite costs exactly one extra rerun and then converges (run →
//! rewrite → rerun → nothing left to rewrite → quiet), and `check` writes
//! nothing at all.
//!
//! That is a claim about backends, so it comes with its boundary. notify's
//! `kqueue` backend (BSD, and macOS under its `macos_kqueue` feature) maps
//! `NOTE_ATTRIB` onto `Modify(Metadata(Any))`, which *is* a trigger, so a
//! platform where a plain read makes an `atime` bump visible could feed
//! itself. luabox ships linux, macOS and windows binaries only
//! (`.github/workflows/release.yml`), and `notify::recommended_watcher`
//! never selects `kqueue` for those. If that ever changes, the answer is a
//! *filtering sweep* — collect the post-run window, keep whatever still
//! passes [`triggers_rerun`] and [`is_relevant`], and rerun if anything
//! survives — never a drain that discards.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use luabox_manifest::layout;
use notify::event::{AccessKind, AccessMode, EventKind, MetadataKind, ModifyKind};
use notify::{Event, RecursiveMode, Watcher};

use crate::emit::{errln, outln};

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
            // Drop non-changes at the source, so they never even start a
            // debounce window — see `triggers_rerun`.
            if !triggers_rerun(event.kind) {
                return;
            }
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

    // Nothing is drained between a run and the next batch — not here and not
    // in the loop below. A drain cannot distinguish a run's own noise from a
    // save made while the run was working, so it swallowed real edits; see
    // the module docs for the per-backend argument that makes the kind filter
    // sufficient on its own.
    let result = on_change();
    report(result);

    while let Some(batch) = next_batch(&rx, root, out_dir, DEBOUNCE_WINDOW) {
        if batch.is_empty() {
            continue;
        }
        outln!("--- watching: rerun ({} files changed) ---", batch.len());
        let result = on_change();
        report(result);
    }
    Ok(())
}

/// Whether a filesystem event describes a *change* worth rerunning for.
///
/// notify reports far more than mutations. Its inotify backend asks for
/// `OPEN | CLOSE_NOWRITE | ATTRIB` on top of the mutation events, so simply
/// reading a watched file — which is all `luabox check` does — produces a
/// steady stream of events for it. Forwarding those turned one edit into a
/// permanent rerun loop: every rerun's reads re-triggered the next.
///
/// The rule, deliberately:
/// - **`Create` / `Remove`** — a file appearing or vanishing changes what the
///   next run sees. Rerun.
/// - **`Modify`, except `Metadata(AccessTime)`** — content, renames, write
///   time, permissions and ownership are all real changes, and an
///   *unclassified* metadata change (`MetadataKind::Any`, which is what
///   inotify's `ATTRIB` becomes) covers `touch`, so an mtime-only touch
///   still reruns. Access *time* is the one metadata change a plain read
///   causes, so it alone is ignored.
/// - **`Access`, except `Close(Write)`** — notify defines this whole variant
///   as "non-mutating access operations": opens, reads, and read-closes are
///   what the rerun itself generates. `Close(Write)` is the exception and is
///   honoured: it means a writer just closed the file, which is a change
///   however it was classified.
/// - **`Any` / `Other`** — a backend that could not classify the event.
///   Rerunning once too often is a far smaller failure than silently missing
///   an edit, so these trigger. Nothing downstream discards them, so this is
///   the whole defence against a self-sustaining loop: it holds because no
///   backend luabox ships on reports a *read* at all — see the module docs
///   for the per-backend audit and where it stops being true.
fn triggers_rerun(kind: EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_)
        | EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime)) => false,
        EventKind::Create(_)
        | EventKind::Modify(_)
        | EventKind::Remove(_)
        | EventKind::Any
        | EventKind::Other => true,
    }
}

/// Block until the first event arrives, then keep collecting for `window`
/// measured from *that* event — not a sliding window, see the module docs —
/// and return the batch's relevant, deduped paths ([`filter_and_dedupe`]). An
/// empty `Vec` means the whole batch was noise.
///
/// `None` once the watcher's channel has closed, which is [`run`]'s only exit.
///
/// Split out of [`run`] so the windowing rule can be driven against a real
/// channel and real time in a unit test — `partition_batches` (test-only)
/// models the same rule over a synthetic event log, and the parity test named
/// in the module docs asserts the two agree on every case in one shared table.
///
/// `window` is a parameter solely so that parity test can stretch it: the live
/// leg drives real threads and real sleeps, and a window measured in hundreds
/// of milliseconds leaves no margin for scheduler jitter. [`run`] always passes
/// [`DEBOUNCE_WINDOW`], which is the only value that ships.
fn next_batch(
    rx: &mpsc::Receiver<Event>,
    root: &Path,
    out_dir: Option<&Path>,
    window: Duration,
) -> Option<Vec<PathBuf>> {
    let mut raw = rx.recv().ok()?.paths;
    let deadline = Instant::now() + window;
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

/// Print a run's outcome. Errors from `on_change` are reported, not
/// propagated — a broken rerun must not kill the watcher.
fn report(result: anyhow::Result<()>) {
    match result {
        Ok(()) => outln!("watch: ok"),
        Err(err) => errln!("watch: failed: {err:#}"),
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
    // The versioned rock tree is the one part of `lua_modules/` the command
    // about to rerun actually reads (#30's type harvest), so a rock install
    // or removal is a real input change — `luarocks install --tree
    // lua_modules <rock>` is precisely the edit a watching developer makes
    // and expects picked up (Shockwave round 10).
    layout::is_project_source(path, root, out_dir) || layout::is_rock_source(path, root)
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
        AccessKind, AccessMode, DEBOUNCE_WINDOW, EventKind, MetadataKind, ModifyKind,
        filter_and_dedupe, is_relevant, next_batch, partition_batches, report, triggers_rerun,
    };
    use notify::event::{CreateKind, DataChange, RemoveKind, RenameMode};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;

    /// A watcher event naming `paths`, as `notify` would deliver it.
    fn event(paths: &[&str]) -> notify::Event {
        notify::Event {
            paths: paths.iter().map(PathBuf::from).collect(),
            ..notify::Event::default()
        }
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    // --- the event-kind filter --------------------------------------------

    #[test]
    fn reads_never_trigger_a_rerun() {
        // Exactly what a rerun's own reads produce on inotify (OPEN,
        // CLOSE_NOWRITE) — the events that made `--watch` loop forever.
        assert!(!triggers_rerun(EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(!triggers_rerun(EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!triggers_rerun(EventKind::Access(AccessKind::Read)));
        assert!(!triggers_rerun(EventKind::Access(AccessKind::Any)));
        assert!(!triggers_rerun(EventKind::Access(AccessKind::Other)));
        // An access *time* bump is what reading a file costs in metadata
        // terms; it is not an edit.
        assert!(!triggers_rerun(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));
    }

    #[test]
    fn writes_creations_removals_and_renames_all_trigger_a_rerun() {
        assert!(triggers_rerun(EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        assert!(triggers_rerun(EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        assert!(triggers_rerun(EventKind::Create(CreateKind::File)));
        assert!(triggers_rerun(EventKind::Remove(RemoveKind::File)));
        // A writer closing the file is a change, whatever notify calls it.
        assert!(triggers_rerun(EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
    }

    #[test]
    fn an_mtime_only_touch_still_triggers_a_rerun() {
        // `touch file.lua` becomes inotify's ATTRIB, which notify reports as
        // an unclassified metadata change. Only AccessTime is ignored, so a
        // touch is still an edit.
        assert!(triggers_rerun(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::Any
        ))));
        assert!(triggers_rerun(EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::WriteTime
        ))));
    }

    #[test]
    fn an_unclassified_event_errs_towards_rerunning() {
        // Missing a real edit is worse than one extra rerun. Nothing
        // downstream throws these away, which is safe only because no shipped
        // backend emits an event for a read at all (see the module docs).
        assert!(triggers_rerun(EventKind::Any));
        assert!(triggers_rerun(EventKind::Other));
        assert!(triggers_rerun(notify::Event::default().kind));
    }

    // --- the batching rule, modelled and lived ----------------------------
    //
    // One table, two consumers. `partition_batches` is the pure model of the
    // windowing rule; `next_batch` is the loop the watcher actually runs. Both
    // are driven over `BATCHING_CASES` below and required to agree — see the
    // module docs for why a silently drifting model is worse than none.

    /// One batching case: a name, the event log as `(offset, path)` pairs in
    /// *window fractions*, and the batches the rule must produce.
    ///
    /// Fractions rather than milliseconds so the same case can be replayed at
    /// two very different window lengths: instantly by the model at the shipped
    /// 200 ms, and over real threads and real sleeps by the live leg at a
    /// stretched window where jitter cannot move an event across a boundary.
    struct BatchingCase {
        name: &'static str,
        events: &'static [(f64, &'static str)],
        batches: &'static [&'static [&'static str]],
    }

    /// Every case both the model and the live loop must batch identically.
    ///
    /// Offsets sit well inside or well outside the window on purpose: the exact
    /// boundary is the one instant a real sender thread cannot be aimed at, so
    /// it is asserted against the model alone
    /// (`the_model_includes_an_event_landing_exactly_on_the_window`).
    const BATCHING_CASES: &[BatchingCase] = &[
        BatchingCase {
            name: "a burst inside the window is one batch",
            events: &[
                (0.0, "/proj/a.lua"),
                (0.25, "/proj/b.lua"),
                (0.6, "/proj/c.lua"),
            ],
            batches: &[&["/proj/a.lua", "/proj/b.lua", "/proj/c.lua"]],
        },
        BatchingCase {
            name: "events far apart are separate batches",
            events: &[(0.0, "/proj/a.lua"), (2.0, "/proj/b.lua")],
            batches: &[&["/proj/a.lua"], &["/proj/b.lua"]],
        },
        BatchingCase {
            // A steady trickle must not extend one batch forever: the window is
            // anchored to the batch's first event, not to the last one seen.
            name: "the window is anchored to the batch start, not sliding",
            events: &[
                (0.0, "/proj/a.lua"),
                (0.6, "/proj/b.lua"),
                (1.4, "/proj/c.lua"),
            ],
            batches: &[&["/proj/a.lua", "/proj/b.lua"], &["/proj/c.lua"]],
        },
        BatchingCase {
            name: "a repeated path inside one batch collapses in first-seen order",
            events: &[
                (0.0, "/proj/b.lua"),
                (0.2, "/proj/a.lua"),
                (0.4, "/proj/b.lua"),
            ],
            batches: &[&["/proj/b.lua", "/proj/a.lua"]],
        },
        BatchingCase {
            name: "noise inside a batch is dropped without splitting it",
            events: &[
                (0.0, "/proj/a.lua"),
                (0.2, "/proj/README.md"),
                (0.4, "/proj/b.lua"),
            ],
            batches: &[&["/proj/a.lua", "/proj/b.lua"]],
        },
    ];

    /// A case's event log at a concrete window length.
    fn scaled(case: &BatchingCase, window: Duration) -> Vec<(Duration, PathBuf)> {
        case.events
            .iter()
            .map(|(fraction, path)| (window.mul_f64(*fraction), p(path)))
            .collect()
    }

    /// A case's expected batches, as owned paths.
    fn expected(case: &BatchingCase) -> Vec<Vec<PathBuf>> {
        case.batches
            .iter()
            .map(|batch| batch.iter().map(|path| p(path)).collect())
            .collect()
    }

    /// The model's batches for `case`, put through the same relevance filter
    /// and dedupe the live loop applies — the model windows, it does not
    /// filter, so this is what makes the two comparable at all.
    fn modelled(case: &BatchingCase) -> Vec<Vec<PathBuf>> {
        partition_batches(&scaled(case, DEBOUNCE_WINDOW), DEBOUNCE_WINDOW)
            .into_iter()
            .map(|batch| filter_and_dedupe(batch, Path::new("/proj"), None))
            .collect()
    }

    #[test]
    fn the_model_batches_every_case_as_documented() {
        for case in BATCHING_CASES {
            assert_eq!(modelled(case), expected(case), "{}", case.name);
        }
    }

    #[test]
    fn the_model_and_the_live_loop_batch_every_case_identically() {
        // The parity assertion the module docs promise, and the reason the
        // model is allowed to exist: a pure function nobody checks against the
        // real loop can drift into describing a watcher that isn't shipped.
        //
        // The window is stretched here so a sender thread's jitter (tens of ms
        // under a loaded runner) cannot move an event across a boundary: every
        // offset in the table is at least 0.4 windows — 320 ms — clear of one.
        const LIVE_WINDOW: Duration = Duration::from_millis(800);
        let root = Path::new("/proj");

        for case in BATCHING_CASES {
            let (tx, rx) = mpsc::channel::<notify::Event>();
            let schedule = scaled(case, LIVE_WINDOW);
            let sender = std::thread::spawn(move || {
                let start = std::time::Instant::now();
                for (offset, path) in schedule {
                    let due = start + offset;
                    let now = std::time::Instant::now();
                    if due > now {
                        std::thread::sleep(due - now);
                    }
                    let _ = tx.send(notify::Event {
                        paths: vec![path],
                        ..notify::Event::default()
                    });
                }
            });

            let want = modelled(case);
            let mut lived = Vec::new();
            for _ in 0..want.len() {
                lived.push(
                    next_batch(&rx, root, None, LIVE_WINDOW)
                        .unwrap_or_else(|| panic!("{}: the channel closed early", case.name)),
                );
            }
            sender.join().expect("sender thread");

            assert_eq!(
                lived, want,
                "{}: the live loop and its model disagree",
                case.name
            );
            assert_eq!(lived, expected(case), "{}", case.name);
        }
    }

    #[test]
    fn the_model_includes_an_event_landing_exactly_on_the_window() {
        // The one case the live leg above cannot replay: `<= t0 + window` is
        // inclusive in the model, and a real sender cannot be aimed at an exact
        // instant. Pinned against the model alone, and against the shipped
        // constant rather than an arbitrary `ms(200)`.
        let events = vec![(Duration::ZERO, p("a.lua")), (DEBOUNCE_WINDOW, p("b.lua"))];
        assert_eq!(
            partition_batches(&events, DEBOUNCE_WINDOW),
            vec![vec![p("a.lua"), p("b.lua")]]
        );
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
    fn relevant_versioned_rock_tree_since_the_harvest_reads_it() {
        // Since #30 the rerun READS `lua_modules/share/lua/<X.Y>/**.lua` for
        // the rock type harvest, so `luarocks install --tree lua_modules` is
        // an input change the watcher must see (Shockwave round 10).
        let root = Path::new("/proj");
        assert!(is_relevant(
            Path::new("/proj/lua_modules/share/lua/5.4/pl/tablex.lua"),
            root,
            None
        ));
        assert!(is_relevant(
            Path::new("/proj/lua_modules/share/lua/5.1/rk.lua"),
            root,
            None
        ));
    }

    #[test]
    fn irrelevant_unread_vendored_paths_ignored() {
        // Only the versioned tree is read; everything else under
        // `lua_modules/` — flat layouts, rockspecs, a NESTED tree inside a
        // workspace member — still is not, and neither is a rock's manifest.
        let root = Path::new("/proj");
        assert!(!is_relevant(
            Path::new("/proj/lua_modules/dep/src/init.lua"),
            root,
            None
        ));
        assert!(!is_relevant(
            Path::new("/proj/packages/core/lua_modules/share/lua/5.4/dep.lua"),
            root,
            None
        ));
        assert!(!is_relevant(
            Path::new("/proj/lua_modules/dep/luabox.toml"),
            root,
            None
        ));
        // A non-.lua file inside the versioned tree is not read either.
        assert!(!is_relevant(
            Path::new("/proj/lua_modules/share/lua/5.4/pl-3.7.0-1.rockspec"),
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
        let batch = next_batch(&rx, root, None, DEBOUNCE_WINDOW).expect("a batch");
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
        assert_eq!(
            next_batch(&rx, root, None, DEBOUNCE_WINDOW),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_closed_channel_ends_the_watch_loop() {
        let (tx, rx) = mpsc::channel::<notify::Event>();
        drop(tx);
        assert_eq!(
            next_batch(&rx, Path::new("/proj"), None, DEBOUNCE_WINDOW),
            None
        );
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

        assert_eq!(
            next_batch(&rx, root, None, DEBOUNCE_WINDOW),
            Some(vec![p("/proj/a.lua")])
        );
        assert_eq!(
            next_batch(&rx, root, None, DEBOUNCE_WINDOW),
            Some(vec![p("/proj/b.lua")])
        );
        sender.join().expect("sender thread");
    }

    #[test]
    fn an_edit_that_landed_while_a_run_was_working_is_still_the_next_batch() {
        // The wave-10 regression, at the level the loop is built from: a save
        // made *during* a run is already sitting in the channel when the run
        // finishes. The post-run drain used to receive it and throw it away,
        // leaving `check --watch` reporting a stale `watch: ok` over a tree
        // the user had just broken. Nothing between `on_change` and
        // `next_batch` may consume events, so it comes back as a batch.
        let root = Path::new("/proj");
        let (tx, rx) = mpsc::channel::<notify::Event>();
        tx.send(event(&["/proj/broken.lua"])).expect("send");
        drop(tx);

        assert_eq!(
            next_batch(&rx, root, None, DEBOUNCE_WINDOW),
            Some(vec![p("/proj/broken.lua")]),
            "an edit queued during a run must survive to trigger the next rerun"
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
