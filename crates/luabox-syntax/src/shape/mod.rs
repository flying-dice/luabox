//! The `.lb` shape DSL grammar (SHAPES.md).
//!
//! Analyser-only Rust-style struct/trait declarations. Additive module: own
//! syntax-kind space ([`ShapeSyntaxKind`]), same rowan infrastructure, zero
//! coupling to the Lua grammar (SHAPES.md §9). Diagnostic block `LB2xxx` is
//! reserved for shapes.
//!
//! Implemented: the token vocabulary and lossless lexer ([`lex`]). The
//! parser (green-tree over the SHAPES.md §3 grammar, rejecting bodies with
//! LB2010) and the canonical `.lb` formatter are the P0 shape work items.

mod kind;
mod lexer;

pub use kind::{ShapeLanguage, ShapeSyntaxKind};
pub use lexer::{ShapeToken, lex};
