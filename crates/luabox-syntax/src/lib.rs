//! Lossless syntax trees for the luabox toolchain — the **Syntax** bounded
//! context (SPEC.md §16).
//!
//! Two independent grammars share the rowan infrastructure and nothing else:
//!
//! - [`lua`] — every supported Lua dialect (5.1–5.4, LuaJIT), one grammar
//!   with dialect feature-gating. Luau is out of scope toolchain-wide
//!   (SPEC.md §1).
//! - [`shape`] — the `.luab` shape DSL (SHAPES.md): analyser-only Rust-style
//!   struct/trait declarations. Own syntax-kind space, zero coupling to the
//!   Lua grammar (SHAPES.md §9).
//!
//! Both trees are lossless (comments/whitespace preserved; tree text is
//! byte-identical to input) and will be error-resilient once the parsers
//! land: broken code still yields a tree.
//!
//! Boundary contract: tree types + parse API; nothing above this crate knows
//! token details.

#[macro_use]
mod kind_macro;

pub mod lua;
pub mod luacats;
pub mod shape;

pub use lua::{Dialect, LuaLanguage, Parse, ParseError, SyntaxKind, Token, lex, parse};
