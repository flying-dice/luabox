//! The salsa database: the storage owner plus the [`Db`] view every tracked
//! query runs against.

use std::sync::{Arc, Mutex};

use salsa::Storage;

/// The database view seen by tracked queries.
///
/// It is `salsa::Database` plus one extra capability: an execution trace
/// ([`Db::push_log`]). Queries record when they *execute* (as opposed to being
/// served from the memo cache), which powers the incrementality tests and
/// doubles as an LSP tracing hook. Recording into a shared `Mutex` is a pure
/// side channel — it never feeds back into a query result, so it does not
/// affect memoization.
#[salsa::db]
pub trait Db: salsa::Database {
    /// Append `message` to the execution trace.
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
    logs: Arc<Mutex<Vec<String>>>,
}

#[salsa::db]
impl salsa::Database for RootDatabase {}

#[salsa::db]
impl Db for RootDatabase {
    fn push_log(&self, message: String) {
        self.logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }
}

impl RootDatabase {
    /// Drain and return the execution trace collected since the last call.
    pub(crate) fn take_logs(&self) -> Vec<String> {
        std::mem::take(
            &mut self
                .logs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}
