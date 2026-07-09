//! Diagnostic vocabulary for the whole toolchain (SPEC.md §14).
//!
//! Cross-cutting support crate (the `rustc_errors` analog, not a bounded
//! context): every producer (syntax, types, lint, lower) emits these; the
//! Frontend context renders them. Every error has an `LB` code, an explain
//! page, and span-rich labels/suggestions. Machine formats: JSON, SARIF,
//! GitHub Actions, GitLab Code Quality. Block `LB2xxx` is reserved for
//! shapes (SHAPES.md §5).
//!
//! Status: under construction — P0.
