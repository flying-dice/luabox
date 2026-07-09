//! Lossless syntax trees for every Lua dialect — the **Syntax** bounded
//! context (SPEC.md §16).
//!
//! One grammar with dialect feature-gating; rowan green/red trees preserving
//! every byte (comments, whitespace) so the same tree feeds the formatter,
//! linter, fixes, and refactors. Error-resilient: broken code still parses.
//!
//! Currently implemented: the lossless lexer ([`lex`]) and the
//! [`SyntaxKind`]/[`LuaLanguage`] vocabulary. The parser proper (green-tree
//! construction over these tokens) is the next P0 milestone.
//!
//! Boundary contract: tree types + parse API; nothing above this crate knows
//! token details.

mod dialect;
mod kind;
mod lexer;

pub use dialect::Dialect;
pub use kind::{LuaLanguage, SyntaxKind};
pub use lexer::{Token, lex};
