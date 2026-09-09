//! The salsa database: the storage owner plus the [`Db`] view every tracked
//! query runs against.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use salsa::Storage;

/// Hard cap on the retained execution trace (round 5 review N23). The trace
/// has exactly one real consumer — the incrementality tests in
/// `crates/luabox-db/tests/analysis.rs`, which assert *which* queries ran by
/// draining it after one or two triggered queries — and zero non-test
/// callers: `AnalysisHost::take_execution_log` is never called from
/// `luabox-lsp`, so a real editor session pushes into it every revision and
/// never drains it. Before this cap, that made it an unbounded `Vec` for the
/// life of the process: measured on a one-file project, RSS grew ~0.84 KiB
/// per keystroke over 3000 edits, linear, no plateau. A `VecDeque` capped at
/// this many entries, dropping the oldest on overflow, turns that into a
/// fixed ceiling — a real session now pays a bounded, constant amount of
/// memory instead of a monotonically growing one — while staying miles above
/// what any test round-trips before its own drain (round-tripped after 1-2
/// queries in every existing test; the widest, `project_types_checked`'s
/// N=25-file display pass, drains at 26 entries).
///
/// Rejected: gating the whole mechanism behind a test-only Cargo feature
/// (zero-cost in production, but this workspace has no precedent for
/// feature-flagged crates, and the only way to *pin* "compiled out in a
/// normal build" from a test is either a Cargo-metadata assertion or an
/// RSS measurement — the latter is exactly what this comment's own number
/// came from, and it is confounded by salsa's own revision-retention
/// behaviour at unit-test scale, not a reliable per-commit gate). Rejected:
/// having the LSP server drain it every request — the trace is a list of
/// internal salsa query names, not user-facing information the way
/// `luabox-lsp`'s merged-ambient rebuild line (`server.rs`, round 5 review
/// N22) is, so surfacing it via `window/logMessage` would reproduce N22's
/// exact per-keystroke spam this same PR fixed, and draining it to nowhere
/// is the same fixed-memory outcome as this cap, just paid on the server's
/// request-handling thread instead of compiled in once here.
///
/// **This cap makes the trace lossy** (M46, round 6 review): capping it
/// (`Vec` → `VecDeque`, oldest entry dropped on overflow) silently broke the
/// "complete list of queries since the last drain" invariant the trace
/// originally had — an assertion that some query is *absent* from a drained
/// trace can no longer distinguish "truly never ran" from "ran, but was
/// evicted to make room for a newer entry", and both outcomes can be
/// arbitrarily likely once a test (or workload) pushes more than
/// `MAX_EXECUTION_LOG_ENTRIES` entries between drains. [`RootDatabase::log_overflowed`]
/// is the caller's way to tell the two apart — read *before*
/// [`RootDatabase::take_logs`], since draining resets it along with the
/// entries — and `crates/luabox-db/tests/analysis.rs`'s `assert_absent`
/// helper refuses to answer an absence question at all once it is set.
pub const MAX_EXECUTION_LOG_ENTRIES: usize = 1024;

/// The execution trace's payload: the capped entry queue plus whether an
/// entry has been evicted since the last drain (M46, round 6 review).
///
/// Before the overflow flag, capping the trace (`Vec` → `VecDeque`, oldest
/// dropped) silently broke the trace's original "complete list of queries
/// since the last drain" invariant: a test — or any caller — asserting a
/// query is *absent* from the drained log cannot tell "truly never ran"
/// from "ran, but its entry was evicted to make room for newer ones" from
/// the `Vec<String>` alone. Both look identical. `overflowed` is the second
/// signal an absence assertion must check first; an absence claim made over
/// a trace that overflowed is not proof of anything and must be refused,
/// not trusted (see `crates/luabox-db/tests/analysis.rs`'s `assert_absent`
/// for the refusing helper this exists for).
#[derive(Default)]
struct ExecutionTrace {
    entries: VecDeque<String>,
    /// Set the moment an entry is evicted to stay within
    /// [`MAX_EXECUTION_LOG_ENTRIES`]; cleared only by
    /// [`RootDatabase::take_logs`] draining the entries it describes —
    /// mirroring the entries' own "since the last drain" lifecycle exactly,
    /// so a caller that checks this flag before draining always reads the
    /// overflow state of the batch it is about to read.
    overflowed: bool,
}

/// The database view seen by tracked queries.
///
/// It is `salsa::Database` plus one extra capability: an execution trace
/// ([`Db::push_log`]). Queries record when they *execute* (as opposed to being
/// served from the memo cache) into a capped [`VecDeque`]
/// ([`MAX_EXECUTION_LOG_ENTRIES`]) — this powers the incrementality tests.
/// Recording into a shared `Mutex` is a pure side channel — it never feeds
/// back into a query result, so it does not affect memoization.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Append `message` to the execution trace, dropping the oldest entry
    /// first if the trace is already at [`MAX_EXECUTION_LOG_ENTRIES`].
    fn push_log(&self, message: String);
}

/// The concrete incremental database backing [`AnalysisHost`](crate::AnalysisHost).
///
/// Cloning is cheap and structural (salsa's `Storage` is `Arc`-backed): a
/// clone is the snapshot an [`Analysis`](crate::Analysis) runs queries on while
/// the host keeps applying edits. The execution trace is shared across clones.
#[salsa::db]
#[derive(Clone, Default)]
pub struct RootDatabase {
    storage: Storage<Self>,
    logs: Arc<Mutex<ExecutionTrace>>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn push_log(&self, message: String) {
        let mut trace = self
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if trace.entries.len() >= MAX_EXECUTION_LOG_ENTRIES {
            trace.entries.pop_front();
            trace.overflowed = true;
        }
        trace.entries.push_back(message);
    }
}

impl RootDatabase {
    /// Drain and return the execution trace collected since the last call.
    ///
    /// This is [`Self::log_overflowed`]'s companion half: it resets the
    /// overflow flag along with the entries (M46), so a caller that wants
    /// to know whether the batch it is about to drain lost anything must
    /// call `log_overflowed` *first* — draining clears the very state it
    /// would read.
    pub(crate) fn take_logs(&self) -> Vec<String> {
        let mut trace = self
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        trace.overflowed = false;
        std::mem::take(&mut trace.entries).into()
    }

    /// Whether the execution trace has evicted at least one entry since the
    /// last [`Self::take_logs`] drain (M46, round 6 review) — the trace's
    /// "complete list of queries since the last drain" invariant no longer
    /// holds once this is `true`: some absence in the next `take_logs()`
    /// may be an eviction, not a query that never ran.
    pub(crate) fn log_overflowed(&self) -> bool {
        self.logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .overflowed
    }
}
