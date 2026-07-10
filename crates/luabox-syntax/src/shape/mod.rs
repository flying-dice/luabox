//! The `.luab` shape DSL grammar (SHAPES-V2.md).
//!
//! Analyser-only TypeScript-adjacent type declarations: a single item form,
//! `export? type Name<T> = <type-expr>`. Additive module: own syntax-kind
//! space ([`ShapeSyntaxKind`]), same rowan infrastructure, zero coupling to
//! the Lua grammar. Diagnostic block `LB2xxx` is reserved for shapes.
//!
//! Pipeline: [`lex`] tiles source into tokens; [`parse`] builds a lossless
//! green tree (rejecting method bodies with `LB2010`, resynchronising on
//! error); [`ast`] gives a typed view over that tree; [`fmt::format`]
//! pretty-prints it to the canonical form.

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
