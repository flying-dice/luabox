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
pub const MAX_EXECUTION_LOG_ENTRIES: usize = 1024;

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
    logs: Arc<Mutex<VecDeque<String>>>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn push_log(&self, message: String) {
        let mut logs = self
            .logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if logs.len() >= MAX_EXECUTION_LOG_ENTRIES {
            logs.pop_front();
        }
        logs.push_back(message);
    }
}

impl RootDatabase {
    /// Drain and return the execution trace collected since the last call.
    pub(crate) fn take_logs(&self) -> Vec<String> {
        let taken: VecDeque<String> = std::mem::take(
            &mut *self
                .logs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        taken.into()
    }
}
