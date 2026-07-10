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
//! `*.lua`, `*.luab`, and the manifest `luabox.toml`. Everything else is
//! noise and is ignored by [`is_relevant`]:
//! - dot-directories and dot-files anywhere under the root (`.git/`,
//!   `.luabox/`, editor state) — same "hidden" convention `check_cmd`'s
//!   and `fmt_cmd`'s file walk already use;
//! - the manifest's `[build]` output directory — generated, not source;
//! - editor temp/lock files: `*.tmp`, `*~` (Emacs backups), `.#*` (Emacs
//!   lock files — also covered by the dot-file rule above), and vim's
//!   `4913` existence-probe file.
//!
//! A manifest (`luabox.toml`) change is not special-cased in the filter —
//! it is deliberately treated as just another relevant file. Re-reading
//! the manifest (edition, strictness, `[build] out`, shape paths) is the
//! closure's job: `check_cmd::run_once`/`fmt_cmd::run_once` already
//! rediscover the project from scratch on every call.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

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
    report(on_change());

    let (tx, rx) = mpsc::channel::<Event>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            // The other end only ever disappears when `run` itself is
            // unwinding (e.g. the caller dropped everything), so a send
            // failure here is not actionable.
            let _ = tx.send(event);
        }
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    loop {
        let Ok(first) = rx.recv() else {
            return Ok(());
        };
        let mut raw = first.paths;
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

        let batch = filter_and_dedupe(raw, root, out_dir);
        if batch.is_empty() {
            continue;
        }
        println!("--- watching: rerun ({} files changed) ---", batch.len());
        report(on_change());
    }
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

/// Whether a changed path should trigger a rerun: a `.lua`/`.luab` source or
/// `luabox.toml`, not under a dot-directory/dot-file or the build output
/// directory, and not an editor temp/lock file.
pub(crate) fn is_relevant(path: &Path, root: &Path, out_dir: Option<&Path>) -> bool {
    if let Some(out) = out_dir
        && path.starts_with(out)
    {
        return false;
    }
    if let Ok(rel) = path.strip_prefix(root)
        && rel.components().any(|c| is_dotfile(c.as_os_str()))
    {
        return false;
    }

    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if is_editor_temp(name) {
        return false;
    }

    name == "luabox.toml"
        || matches!(
            path.extension().and_then(OsStr::to_str),
            Some("lua" | "luab")
        )
}

fn is_dotfile(component: &OsStr) -> bool {
    component.to_str().is_some_and(|s| s.starts_with('.'))
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
    use super::{DEBOUNCE_WINDOW, filter_and_dedupe, is_relevant, partition_batches};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

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
    fn relevant_lua_lb_and_manifest() {
        let root = Path::new("/proj");
        assert!(is_relevant(Path::new("/proj/src/foo.lua"), root, None));
        assert!(is_relevant(Path::new("/proj/shapes/foo.luab"), root, None));
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
}
