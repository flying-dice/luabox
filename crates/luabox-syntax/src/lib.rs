//! Lossless syntax trees for the luabox toolchain — the **Syntax** bounded
//! context (SPEC.md §16).
//!
//! The Lua grammar shares the rowan infrastructure:
//!
//! - [`lua`] — every supported Lua dialect (5.1–5.4, LuaJIT), one grammar
//!   with dialect feature-gating. Luau is out of scope toolchain-wide
//!   (SPEC.md §1).
//!
//! The tree is lossless (comments/whitespace preserved; tree text is
//! byte-identical to input) and error-resilient: broken code still yields a
//! tree.
//!
//! Boundary contract: tree types + parse API; nothing above this crate knows
//! token details.

#[macro_use]
mod kind_macro;

pub mod lua;
pub mod luacats;

pub use lua::{Dialect, LuaLanguage, Parse, ParseError, SyntaxKind, Token, lex, parse};
