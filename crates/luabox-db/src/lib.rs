//! Salsa-style incremental computation database — **Semantics** bounded
//! context (SPEC.md §8, §16).
//!
//! The single analysis database backing check, lint, LSP, fmt, and doc:
//! every query memoized, fine-grained invalidation, no full re-analysis on
//! keystroke. Boundary contract: the query interface (DB traits) — this is
//! how the Frontend context consumes Semantics.
//!
//! Status: placeholder — P1.
