//! The `.lb` shape DSL grammar (SHAPES.md).
//!
//! Analyser-only Rust-style struct/trait declarations. Additive module: own
//! syntax-kind space ([`ShapeSyntaxKind`]), same rowan infrastructure, zero
//! coupling to the Lua grammar (SHAPES.md §9). Diagnostic block `LB2xxx` is
//! reserved for shapes.
//!
//! Pipeline: [`lex`] tiles source into tokens; [`parse`] builds a lossless
//! green tree (rejecting bodies with `LB2010`, resynchronising on error);
//! [`ast`] gives a typed view over that tree; [`fmt::format`] pretty-prints it
//! to the canonical form (SHAPES.md §8).

pub mod ast;
mod fmt;
mod kind;
mod lexer;
mod parser;

pub use fmt::format;
pub use kind::{
    ShapeLanguage, ShapeSyntaxElement, ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken,
};
pub use lexer::{ShapeToken, lex};
pub use parser::{LB2010_MESSAGE, ParseError, ShapeParse, parse};
