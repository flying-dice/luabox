//! The Lua grammar: every supported dialect (5.1–5.4, LuaJIT), one grammar
//! with dialect feature-gating (SPEC.md §2, §16).
//!
//! Implemented: the lossless lexer ([`lex`]) and the [`SyntaxKind`] /
//! [`LuaLanguage`] vocabulary. The parser proper (green-tree construction
//! over these tokens) is the next P0 milestone.

mod dialect;
mod kind;
mod lexer;

pub use dialect::Dialect;
pub use kind::{LuaLanguage, SyntaxKind};
pub use lexer::{Token, lex};
