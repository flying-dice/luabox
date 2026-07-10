//! The HIR data model: desugared, dialect-neutral, arena-addressed.
//!
//! Every function (and the top-level chunk) is a [`Body`] holding its own
//! arenas of [`Expr`] and [`Stmt`]. Nodes reference one another only by index
//! ([`ExprId`], [`StmtId`], [`BodyId`]); no ranges live inside HIR nodes — the
//! [`crate::SourceMap`] side table maps a [`HirId`] back to a syntax range.
//!
//! ## Desugarings (see also crate docs)
//! - **Field access** `a.b` is folded into [`Expr::Index`] with a synthesized
//!   string-key literal and `from_field: true`; there is no separate `Field`
//!   node.
//! - **Method call** `o:m(x)` stays a distinct [`Expr::MethodCall`], but the
//!   receiver is recorded explicitly so consumers can treat it as the implicit
//!   `self` first argument (`o.m(o, x)`) while preserving single evaluation of
//!   `o`.
//! - **Method / function declaration** `function a.b:c(x) … end` desugars to a
//!   plain [`Stmt::Assign`] of a [`Expr::Function`] (with a leading implicit
//!   `self` param for the `:` form) to the path `a.b.c`. There is no
//!   `FunctionDecl` statement in the HIR.
//! - **Parentheses** are erased. When a paren truncates a multi-value producer
//!   (call / method call / `...`) to one value, an [`Expr::Truncate`] wrapper
//!   records that; single-value parens vanish entirely.

use crate::arena::{Arena, Idx};
use crate::literal::{LitStr, Number};

/// Handle to an [`Expr`] within its owning [`Body`].
pub type ExprId = Idx<Expr>;
/// Handle to a [`Stmt`] within its owning [`Body`].
pub type StmtId = Idx<Stmt>;
/// Handle to a [`Body`] within the [`crate::LoweredFile`].
pub type BodyId = Idx<Body>;
/// Handle to a [`Binding`] within the [`crate::LoweredFile`].
pub type BindingId = Idx<Binding>;
/// Handle to a [`Label`] within the [`crate::LoweredFile`].
pub type LabelId = Idx<Label>;

/// A HIR node identity, unique per file: a body plus an expr-or-stmt index.
///
/// This is the key for both the source map and the resolution table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HirId {
    pub body: BodyId,
    pub node: NodeId,
}

/// The expr-or-stmt discriminant inside a [`HirId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeId {
    Expr(ExprId),
    Stmt(StmtId),
}

impl HirId {
    pub fn expr(body: BodyId, expr: ExprId) -> Self {
        Self {
            body,
            node: NodeId::Expr(expr),
        }
    }

    pub fn stmt(body: BodyId, stmt: StmtId) -> Self {
        Self {
            body,
            node: NodeId::Stmt(stmt),
        }
    }
}

// === Bodies ===

/// One lowered function (or the top-level chunk).
#[derive(Debug, Clone)]
pub struct Body {
    /// The enclosing function's body, or `None` for the chunk.
    pub parent: Option<BodyId>,
    /// Named parameters, in order. For a method, `self` is the first entry.
    pub params: Vec<BindingId>,
    /// Whether the parameter list ends in `...`.
    pub is_vararg: bool,
    /// The function body.
    pub block: Block,
    exprs: Arena<Expr>,
    stmts: Arena<Stmt>,
}

impl Body {
    pub(crate) fn new(parent: Option<BodyId>) -> Self {
        Self {
            parent,
            params: Vec::new(),
            is_vararg: false,
            block: Block::default(),
            exprs: Arena::new(),
            stmts: Arena::new(),
        }
    }

    pub(crate) fn alloc_expr(&mut self, expr: Expr) -> ExprId {
        self.exprs.alloc(expr)
    }

    pub(crate) fn alloc_stmt(&mut self, stmt: Stmt) -> StmtId {
        self.stmts.alloc(stmt)
    }

    pub(crate) fn stmt_mut(&mut self, id: StmtId) -> &mut Stmt {
        &mut self.stmts[id]
    }

    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id]
    }

    pub fn stmt(&self, id: StmtId) -> &Stmt {
        &self.stmts[id]
    }

    /// All expressions with their handles, in allocation order.
    pub fn exprs(&self) -> impl Iterator<Item = (ExprId, &Expr)> {
        self.exprs.iter()
    }

    /// All statements with their handles, in allocation order.
    pub fn stmts(&self) -> impl Iterator<Item = (StmtId, &Stmt)> {
        self.stmts.iter()
    }
}

/// A lexical block: an ordered list of statements.
#[derive(Debug, Clone, Default)]
pub struct Block {
    pub stmts: Vec<StmtId>,
}

// === Statements ===

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `local a <const>, b = 1, 2`.
    Local {
        names: Vec<LocalBinding>,
        init: Vec<ExprId>,
    },
    /// `targets = values` (also the desugared form of `function a.b:c() … end`).
    Assign {
        targets: Vec<ExprId>,
        values: Vec<ExprId>,
    },
    /// A bare expression in statement position (a call).
    ExprStmt(ExprId),
    /// `if … then … {elseif … then …} [else …] end`, flattened into a `Vec`
    /// of branches (first = the `if`) plus an optional trailing `else`.
    If {
        branches: Vec<IfBranch>,
        else_block: Option<Block>,
    },
    While {
        cond: ExprId,
        body: Block,
    },
    Repeat {
        body: Block,
        /// The `until` condition — resolved in the body's own scope (it can
        /// see the loop body's locals).
        cond: ExprId,
    },
    NumericFor {
        var: BindingId,
        start: ExprId,
        end: ExprId,
        step: Option<ExprId>,
        body: Block,
    },
    GenericFor {
        vars: Vec<BindingId>,
        exprs: Vec<ExprId>,
        body: Block,
    },
    Do {
        body: Block,
    },
    Return(Vec<ExprId>),
    Break,
    /// `goto name`; `target` is resolved to the visible label, or `None` if no
    /// matching label exists in the function (legality checking is a TODO for
    /// diagnostics).
    Goto {
        name: String,
        target: Option<LabelId>,
    },
    /// `::name::`.
    Label {
        name: String,
        label: LabelId,
    },
    /// `local function f(…) … end` — the binding is in scope inside `func`
    /// (recursion), unlike `local f = function …`.
    LocalFunction {
        binding: BindingId,
        func: ExprId,
    },
    /// A statement the parser could not recover into a real construct.
    Error,
}

/// One `if`/`elseif` arm: a condition and its block.
#[derive(Debug, Clone)]
pub struct IfBranch {
    pub cond: ExprId,
    pub block: Block,
}

/// One name introduced by a `local` statement, with its optional attribute.
#[derive(Debug, Clone)]
pub struct LocalBinding {
    pub binding: BindingId,
    pub attrib: Option<Attrib>,
}

/// A 5.4 local attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attrib {
    Const,
    Close,
}

// === Expressions ===

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(Literal),
    /// A name reference; its [`Resolution`](crate::Resolution) lives in the
    /// resolution side table keyed by this expr's [`HirId`].
    Name(String),
    /// `base[index]`. `from_field` is `true` when this came from `base.name`
    /// sugar (with `index` a synthesized string literal).
    Index {
        base: ExprId,
        index: ExprId,
        from_field: bool,
    },
    /// `callee(args)`.
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    /// `receiver:method(args)`. The receiver is the implicit `self` argument.
    MethodCall {
        receiver: ExprId,
        method: String,
        args: Vec<ExprId>,
    },
    /// A closure — the referenced [`Body`] holds its params and block.
    Function(BodyId),
    Table {
        entries: Vec<TableEntry>,
    },
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Unary {
        op: UnOp,
        operand: ExprId,
    },
    /// `...`.
    Vararg,
    /// A parenthesized multi-value producer truncated to a single value —
    /// `(f())`, `(...)`. Single-value parens are erased and never wrapped.
    Truncate(ExprId),
    /// A missing/unrecoverable expression (from a broken parse).
    Error,
}

/// One table-constructor entry.
#[derive(Debug, Clone)]
pub enum TableEntry {
    /// A positional item: `{ v }`.
    Positional(ExprId),
    /// `name = v` (sugar for `["name"] = v`).
    Named { name: String, value: ExprId },
    /// `[k] = v`.
    Keyed { key: ExprId, value: ExprId },
}

/// A literal value carried directly on the HIR node.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Nil,
    Bool(bool),
    Number(Number),
    String(LitStr),
}

/// Binary operators (`op` enums, not token kinds — the token boundary stops
/// here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// `//`
    IDiv,
    Mod,
    Pow,
    /// `..`
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// `&`
    BAnd,
    /// `|`
    BOr,
    /// `~` (binary)
    BXor,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    Not,
    /// `#`
    Len,
    /// `~` (unary)
    BNot,
}

// === Resolution & bindings ===

/// The outcome of resolving a [`Expr::Name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// A binding in the same function body.
    Local(BindingId),
    /// A binding in an enclosing function; `depth` counts function boundaries
    /// crossed (1 = immediately enclosing function).
    Upvalue { binding: BindingId, depth: u32 },
    /// A free name — a global / `_ENV` field access.
    Global(String),
}

/// A resolved value binding (local, parameter, loop variable, …).
///
/// Bindings carry their declaring identifier's range directly (they are
/// resolution metadata, not HIR expr/stmt nodes, so this does not violate the
/// "no ranges inside HIR nodes" rule).
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    /// The function body that owns this binding.
    pub body: BodyId,
    pub kind: BindingKind,
    pub range: rowan::TextRange,
}

/// What introduced a [`Binding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// `local x`.
    Local,
    /// A function parameter.
    Param,
    /// The implicit `self` of a `:` method.
    SelfParam,
    /// A numeric- or generic-`for` control variable.
    ForVar,
    /// `local function f`.
    LocalFunction,
}

/// A `::label::` definition.
#[derive(Debug, Clone)]
pub struct Label {
    pub name: String,
    pub body: BodyId,
    pub range: rowan::TextRange,
}

// === require graph ===

/// A static `require("module")` edge with a literal argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireEdge {
    /// The literal module string (e.g. `"a.b.c"`).
    pub module: String,
    /// The range of the whole `require(...)` call.
    pub range: rowan::TextRange,
}

/// A `require(<non-literal>)` call whose target cannot be resolved statically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicRequire {
    /// The range of the whole `require(...)` call.
    pub range: rowan::TextRange,
}
