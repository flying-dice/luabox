//! Diagnostic vocabulary for the whole toolchain (SPEC.md §14).
//!
//! Cross-cutting support crate (the `rustc_errors` analog, not a bounded
//! context): every producer (syntax, types, lint, lower) emits these; the
//! Frontend context renders them. Every error has an `LB` code, an explain
//! page, and span-rich labels/suggestions. Machine formats: JSON, SARIF,
//! GitHub Actions, GitLab Code Quality. Block `LB2xxx` is reserved for
//! shapes (SHAPES.md §5).
//!
//! The pieces:
//! - [`Code`] / [`Severity`] — the coded vocabulary.
//! - [`Span`] / [`Label`] / [`Suggestion`] / [`Diagnostic`] — the payload.
//! - [`registry::explain`] — title + Markdown explain page per code.
//! - [`render`] — one function per output [`Format`].

mod code;
mod diagnostic;
pub mod registry;
mod render;

pub use code::{Code, CodeParseError, Severity};
pub use diagnostic::{Diagnostic, Label, Span, Suggestion};
pub use registry::{Entry, explain};
pub use render::{
    Format, SourceLookup, render, render_github_actions, render_gitlab_code_quality, render_human,
    render_json, render_sarif,
};
