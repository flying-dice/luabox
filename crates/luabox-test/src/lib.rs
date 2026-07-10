//! Test runner and runtime-matrix orchestration — **Execution** bounded
//! context (SPEC.md §11, §16).
//!
//! `luabox test` is not a runtime: it discovers test files, resolves a Lua
//! interpreter (PATH-based until the toolchain manager, #27), and drives an
//! embedded, maximally-portable Lua harness that provides both the native
//! flat `test(name, fn)` API and busted-compatible `describe`/`it`/`assert`
//! shims. Results come back over a line protocol, are aggregated across
//! files (one process per file, rayon-parallel) and, with `--matrix`,
//! across every Lua found on PATH.
//!
//! This is the only bounded context allowed to spawn runtimes (SPEC.md §16).
//!
//! ## Not yet implemented
//!
//! Coverage (source-map-aware instrumentation) and JUnit/JSON reporters are
//! later slices of the same phase (SPEC.md §11); output is human-only today.
//!
//! ## Module map
//!
//! * [`discovery`] — zero-config test-file discovery.
//! * [`runtime`] — PATH-based runtime resolution + `--matrix` probing.
//! * [`protocol`] — the harness's line protocol and its parser.
//! * [`runner`] — process fan-out, pattern filtering, aggregation.
//! * [`report`] — human-readable rendering.

pub mod discovery;
pub mod protocol;
pub mod report;
pub mod runner;
pub mod runtime;

/// The embedded Lua test harness, prepended-and-run by the runner. Written
/// to a temp file per run and passed the test files as arguments. Kept as a
/// separate `.lua` asset (not an inline string) so it can be linted/edited
/// as real Lua.
pub const HARNESS_SOURCE: &str = include_str!("harness.lua");

pub use discovery::discover;
pub use protocol::{CaseResult, Outcome, ParsedRun};
pub use report::{Summary, render};
pub use runner::{FileOutcome, RuntimeReport, SuiteOptions, run_suite};
pub use runtime::{
    MatrixResolution, ResolveError, RuntimeSpec, candidate_names, resolve_default, resolve_matrix,
};
