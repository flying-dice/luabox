//! The Lua grammar: every supported dialect (5.1–5.4, LuaJIT), one grammar
//! with dialect feature-gating (SPEC.md §2, §16).
//!
//! Pipeline: [`lex`] → [`parse`] (error-resilient green trees; the tree text
//! is always byte-identical to the input) → [`ast`] (typed node wrappers).
//! Dialect legality of parsed constructs is a later validation pass; the
//! parser accepts the union grammar.

pub mod ast;
mod dialect;
pub mod fmt;
mod grammar;
mod kind;
mod lexer;
mod parser;
pub mod validate;

pub use dialect::Dialect;
pub use kind::{LuaLanguage, SyntaxKind};
pub use lexer::{Token, lex};
pub use parser::{Parse, ParseError, SyntaxElement, SyntaxNode, SyntaxToken, parse};
