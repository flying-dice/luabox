//! Desugared IR and name resolution — the entry to the **Semantics** bounded
//! context (SPEC.md §16).
//!
//! Lowers `luabox-syntax` trees into a compact, dialect-neutral HIR and
//! resolves names (locals, upvalues, globals, `require` edges). Consumed by
//! `luabox-types` for inference and by `luabox-lower` for emit.
//!
//! # Boundary contract
//!
//! [`lower`] takes a syntax [`Parse`](luabox_syntax::lua::Parse) and returns a
//! [`LoweredFile`]; no token kinds or syntax nodes leak out. The only syntax
//! artifact consumers see is [`rowan::TextRange`], via the [`SourceMap`], for
//! diagnostics.
//!
//! # Shape
//!
//! - One [`Body`](hir::Body) per function plus one for the top-level chunk,
//!   each with its own `Expr`/`Stmt` arenas ([`arena::Arena`] + typed
//!   [`arena::Idx`], rust-analyzer style — no `Rc`, no cycles).
//! - Positions live only in the [`SourceMap`] side table, keyed by
//!   [`hir::HirId`].
//! - Name resolutions ([`hir::Resolution`]: local / upvalue / global) live in
//!   a side table on [`LoweredFile`], also keyed by `HirId`.
//!
//! # Desugarings
//!
//! | Surface | HIR |
//! |---|---|
//! | `function a.b:c(x) … end` | `Stmt::Assign` of a `Expr::Function` (with a leading implicit `self` param) to `a.b.c` |
//! | `a.b` | `Expr::Index` with a synthesized string key, `from_field: true` |
//! | `o:m(x)` | `Expr::MethodCall` kept distinct (receiver evaluated once), receiver recorded as the implicit `self` argument |
//! | `(expr)` | erased; `Expr::Truncate` kept only around multi-value producers (`(f())`, `(...)`) where the paren truncates to one value |
//! | `if/elseif/else` | one `Stmt::If` with a flat `Vec` of branches |
//! | number/string literals | decoded values (`literal::Number`, `literal::LitStr`) — hex, hex floats, LuaJIT suffixes, escapes |

pub mod arena;
mod file;
pub mod hir;
pub mod literal;
mod lower;
mod source_map;

pub use file::LoweredFile;
pub use hir::{
    Attrib, BinOp, Binding, BindingId, BindingKind, Block, Body, BodyId, DynamicRequire, Expr,
    ExprId, HirId, IfBranch, Label, LabelId, Literal, LocalBinding, NodeId, RequireEdge,
    Resolution, Stmt, StmtId, TableEntry, UnOp,
};
pub use literal::{LitStr, Number};
pub use lower::lower;
pub use source_map::SourceMap;
