//! Lowering: `luabox-syntax` typed AST → HIR, with name resolution, `require`
//! extraction, and `goto`/label resolution all performed in the same walk.
//!
//! Resolution runs eagerly alongside lowering because Lua's scoping rules are
//! position-sensitive (a `local`'s initializer cannot see the `local` itself;
//! `repeat`'s `until` *can* see the body's locals; loop variables are fresh
//! per loop). `goto` targets are resolved in a second, structure-aware pass
//! once every label in a function is known (so forward gotos resolve).

use std::collections::HashMap;

use luabox_syntax::lua::ast::{self, AstNode as _};
use luabox_syntax::lua::{Parse, SyntaxKind, SyntaxToken};
use rowan::TextRange;

use crate::arena::Arena;
use crate::file::LoweredFile;
use crate::hir::{
    Attrib, BinOp, Binding, BindingId, BindingKind, Block, Body, BodyId, DynamicRequire, Expr,
    ExprId, HirId, IfBranch, Label, LabelId, Literal, LocalBinding, RequireEdge, Resolution, Stmt,
    StmtId, TableEntry, UnOp,
};
use crate::literal::{LitStr, decode_string, parse_number};
use crate::source_map::SourceMap;

/// Lower a parsed Lua file into its [`LoweredFile`].
///
/// The boundary function of the Semantics context: it takes a syntax
/// [`Parse`] and returns position-free HIR plus the resolution, source-map,
/// and `require`-graph side tables. No token kinds cross this boundary.
#[must_use]
pub fn lower(parse: &Parse) -> LoweredFile {
    let tree = parse.tree();
    let mut lowerer = Lowerer::new();
    let chunk = lowerer.start_body(None);
    let block = lowerer.lower_block_in_scope(tree.block().as_ref());
    lowerer.body_mut(chunk).block = block;
    lowerer.finish_body();
    lowerer.finish(chunk)
}

/// One lexical scope: the value bindings visible in a block (or a function's
/// parameter list). Whether a hit is a local or an upvalue is decided by
/// comparing the binding's owning body against the current one.
struct Scope {
    names: HashMap<String, BindingId>,
}

struct Lowerer {
    /// Bodies indexed by `BodyId`; `Option` because a body is reserved (id
    /// minted) before its contents finish lowering.
    bodies: Vec<Option<Body>>,
    /// The stack of enclosing function bodies (innermost last).
    body_stack: Vec<BodyId>,
    bindings: Arena<Binding>,
    labels: Arena<Label>,
    scopes: Vec<Scope>,
    source_map: SourceMap,
    resolutions: HashMap<HirId, Resolution>,
    requires: Vec<RequireEdge>,
    dynamic_requires: Vec<DynamicRequire>,
}

impl Lowerer {
    fn new() -> Self {
        Self {
            bodies: Vec::new(),
            body_stack: Vec::new(),
            bindings: Arena::new(),
            labels: Arena::new(),
            scopes: Vec::new(),
            source_map: SourceMap::default(),
            resolutions: HashMap::new(),
            requires: Vec::new(),
            dynamic_requires: Vec::new(),
        }
    }

    // === Body / scope bookkeeping ===

    fn cur_body(&self) -> BodyId {
        *self.body_stack.last().expect("no open body")
    }

    fn body_mut(&mut self, id: BodyId) -> &mut Body {
        self.bodies[id.raw() as usize]
            .as_mut()
            .expect("body present")
    }

    fn body_ref(&self, id: BodyId) -> &Body {
        self.bodies[id.raw() as usize]
            .as_ref()
            .expect("body present")
    }

    /// Reserve a fresh body id, push it as the current function, and open its
    /// root scope (which will hold the parameters).
    fn start_body(&mut self, parent: Option<BodyId>) -> BodyId {
        let raw = u32::try_from(self.bodies.len()).expect("body count overflowed u32");
        let id = BodyId::from_raw(raw);
        self.bodies.push(Some(Body::new(parent)));
        self.body_stack.push(id);
        self.scopes.push(Scope {
            names: HashMap::new(),
        });
        id
    }

    fn finish_body(&mut self) {
        self.scopes.pop();
        self.body_stack.pop();
    }

    fn push_block_scope(&mut self) {
        self.scopes.push(Scope {
            names: HashMap::new(),
        });
    }

    fn pop_block_scope(&mut self) {
        self.scopes.pop();
    }

    // === Allocation with source mapping ===

    fn alloc_expr(&mut self, expr: Expr, range: TextRange) -> ExprId {
        let body = self.cur_body();
        let id = self.body_mut(body).alloc_expr(expr);
        self.source_map.insert(HirId::expr(body, id), range);
        id
    }

    fn alloc_stmt(&mut self, stmt: Stmt, range: TextRange) -> StmtId {
        let body = self.cur_body();
        let id = self.body_mut(body).alloc_stmt(stmt);
        self.source_map.insert(HirId::stmt(body, id), range);
        id
    }

    // === Bindings & resolution ===

    fn declare_binding(&mut self, name: String, kind: BindingKind, range: TextRange) -> BindingId {
        let body = self.cur_body();
        let id = self.bindings.alloc(Binding {
            name: name.clone(),
            body,
            kind,
            range,
        });
        self.scopes
            .last_mut()
            .expect("no open scope")
            .names
            .insert(name, id);
        id
    }

    /// Allocate a name expression and record its resolution.
    fn alloc_name_expr(&mut self, text: &str, range: TextRange) -> ExprId {
        let id = self.alloc_expr(Expr::Name(text.to_string()), range);
        let body = self.cur_body();
        self.resolve_name(text, HirId::expr(body, id));
        id
    }

    fn resolve_name(&mut self, name: &str, at: HirId) {
        let cur = self.cur_body();
        for scope in self.scopes.iter().rev() {
            if let Some(&binding) = scope.names.get(name) {
                let bind_body = self.bindings[binding].body;
                let resolution = if bind_body == cur {
                    Resolution::Local(binding)
                } else {
                    Resolution::Upvalue {
                        binding,
                        depth: self.function_depth(cur, bind_body),
                    }
                };
                self.resolutions.insert(at, resolution);
                return;
            }
        }
        self.resolutions
            .insert(at, Resolution::Global(name.to_string()));
    }

    /// Number of function boundaries between `from` (current) and its ancestor
    /// `to`, i.e. the upvalue depth (1 = immediately enclosing function).
    fn function_depth(&self, from: BodyId, to: BodyId) -> u32 {
        let pos = |target: BodyId| {
            self.body_stack
                .iter()
                .position(|&b| b == target)
                .expect("body on stack")
        };
        u32::try_from(pos(from) - pos(to)).unwrap_or(u32::MAX)
    }

    // === Blocks ===

    /// Lower a block's statements into the *current* scope (used for function
    /// and chunk bodies, whose parameter scope doubles as the block scope).
    fn lower_block_in_scope(&mut self, block: Option<&ast::Block>) -> Block {
        let mut stmts = Vec::new();
        if let Some(block) = block {
            for stmt in block.stmts() {
                stmts.push(self.lower_stmt(&stmt));
            }
        }
        Block { stmts }
    }

    /// Lower a block inside a fresh nested scope (`do`, loop bodies, `if`
    /// arms).
    fn lower_scoped_block(&mut self, block: Option<&ast::Block>) -> Block {
        self.push_block_scope();
        let block = self.lower_block_in_scope(block);
        self.pop_block_scope();
        block
    }

    // === Statements ===

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> StmtId {
        let range = stmt.syntax().text_range();
        match stmt {
            ast::Stmt::Local(local) => self.lower_local(local, range),
            ast::Stmt::Assign(assign) => self.lower_assign(assign, range),
            ast::Stmt::Call(call) => {
                let expr = self.lower_expr_opt(call.expr(), range);
                self.alloc_stmt(Stmt::ExprStmt(expr), range)
            }
            ast::Stmt::Do(do_stmt) => {
                let body = self.lower_scoped_block(do_stmt.body().as_ref());
                self.alloc_stmt(Stmt::Do { body }, range)
            }
            ast::Stmt::While(while_stmt) => self.lower_while(while_stmt, range),
            ast::Stmt::Repeat(repeat) => self.lower_repeat(repeat, range),
            ast::Stmt::If(if_stmt) => self.lower_if(if_stmt, range),
            ast::Stmt::NumericFor(for_stmt) => self.lower_numeric_for(for_stmt, range),
            ast::Stmt::GenericFor(for_stmt) => self.lower_generic_for(for_stmt, range),
            ast::Stmt::FunctionDecl(decl) => self.lower_function_decl(decl, range),
            ast::Stmt::LocalFunction(decl) => self.lower_local_function(decl, range),
            ast::Stmt::Return(ret) => {
                let exprs = self.lower_expr_list(ret.exprs());
                self.alloc_stmt(Stmt::Return(exprs), range)
            }
            ast::Stmt::Break(_) => self.alloc_stmt(Stmt::Break, range),
            ast::Stmt::Goto(goto) => {
                let (name, _) = token_text(goto.label().as_ref(), range);
                self.alloc_stmt(Stmt::Goto { name, target: None }, range)
            }
            ast::Stmt::Label(label) => self.lower_label(label, range),
        }
    }

    fn lower_local(&mut self, local: &ast::LocalStmt, range: TextRange) -> StmtId {
        // Initializers are evaluated before the new names come into scope.
        let init = self.lower_expr_list(local.values());
        let mut names = Vec::new();
        for local_name in local.names() {
            let (text, name_range) = token_text(local_name.name().as_ref(), range);
            let binding = self.declare_binding(text, BindingKind::Local, name_range);
            let attrib = local_name.attrib().and_then(|a| {
                if a.is_const() {
                    Some(Attrib::Const)
                } else if a.is_close() {
                    Some(Attrib::Close)
                } else {
                    None
                }
            });
            names.push(LocalBinding { binding, attrib });
        }
        self.alloc_stmt(Stmt::Local { names, init }, range)
    }

    fn lower_assign(&mut self, assign: &ast::AssignStmt, range: TextRange) -> StmtId {
        let targets = self.lower_expr_list_opt(assign.targets());
        let values = self.lower_expr_list_opt(assign.values());
        self.alloc_stmt(Stmt::Assign { targets, values }, range)
    }

    fn lower_while(&mut self, while_stmt: &ast::WhileStmt, range: TextRange) -> StmtId {
        let cond = self.lower_expr_opt(while_stmt.condition(), range);
        let body = self.lower_scoped_block(while_stmt.body().as_ref());
        self.alloc_stmt(Stmt::While { cond, body }, range)
    }

    fn lower_repeat(&mut self, repeat: &ast::RepeatStmt, range: TextRange) -> StmtId {
        // The `until` condition shares the body's scope (it sees the body's
        // locals), so we do not use `lower_scoped_block` here.
        self.push_block_scope();
        let body = self.lower_block_in_scope(repeat.body().as_ref());
        let cond = self.lower_expr_opt(repeat.condition(), range);
        self.pop_block_scope();
        self.alloc_stmt(Stmt::Repeat { body, cond }, range)
    }

    fn lower_if(&mut self, if_stmt: &ast::IfStmt, range: TextRange) -> StmtId {
        let mut branches = Vec::new();
        let cond = self.lower_expr_opt(if_stmt.condition(), range);
        let block = self.lower_scoped_block(if_stmt.then_block().as_ref());
        branches.push(IfBranch { cond, block });
        for clause in if_stmt.elseif_clauses() {
            let clause_range = clause.syntax().text_range();
            let cond = self.lower_expr_opt(clause.condition(), clause_range);
            let block = self.lower_scoped_block(clause.block().as_ref());
            branches.push(IfBranch { cond, block });
        }
        let else_block = if_stmt
            .else_clause()
            .map(|clause| self.lower_scoped_block(clause.block().as_ref()));
        self.alloc_stmt(
            Stmt::If {
                branches,
                else_block,
            },
            range,
        )
    }

    fn lower_numeric_for(&mut self, for_stmt: &ast::NumericForStmt, range: TextRange) -> StmtId {
        // Range expressions evaluate in the outer scope.
        let start = self.lower_expr_opt(for_stmt.start(), range);
        let end = self.lower_expr_opt(for_stmt.end(), range);
        let step = for_stmt.step().map(|e| self.lower_expr(&e));
        self.push_block_scope();
        let (text, var_range) = token_text(for_stmt.var().as_ref(), range);
        let var = self.declare_binding(text, BindingKind::ForVar, var_range);
        let body = self.lower_block_in_scope(for_stmt.body().as_ref());
        self.pop_block_scope();
        self.alloc_stmt(
            Stmt::NumericFor {
                var,
                start,
                end,
                step,
                body,
            },
            range,
        )
    }

    fn lower_generic_for(&mut self, for_stmt: &ast::GenericForStmt, range: TextRange) -> StmtId {
        let exprs = self.lower_expr_list(for_stmt.exprs());
        self.push_block_scope();
        let mut vars = Vec::new();
        for var in for_stmt.vars() {
            let name = var.text().to_string();
            vars.push(self.declare_binding(name, BindingKind::ForVar, var.text_range()));
        }
        let body = self.lower_block_in_scope(for_stmt.body().as_ref());
        self.pop_block_scope();
        self.alloc_stmt(Stmt::GenericFor { vars, exprs, body }, range)
    }

    /// `function a.b:c(x) … end` → `a.b.c = function(self, x) … end`.
    fn lower_function_decl(&mut self, decl: &ast::FunctionDeclStmt, range: TextRange) -> StmtId {
        let name = decl.name();
        let is_method = name.as_ref().is_some_and(ast::FunctionName::is_method);
        let self_range = name
            .as_ref()
            .and_then(ast::FunctionName::method_name)
            .map(|t| t.text_range());
        let func = self.lower_function(
            decl.param_list(),
            decl.body().as_ref(),
            is_method,
            range,
            self_range,
        );
        let target = self.lower_function_path(name.as_ref(), range);
        self.alloc_stmt(
            Stmt::Assign {
                targets: vec![target],
                values: vec![func],
            },
            range,
        )
    }

    /// Build the `a.b.c` target path of a function declaration: the first
    /// segment is a resolved name, each subsequent segment a field index.
    fn lower_function_path(
        &mut self,
        name: Option<&ast::FunctionName>,
        fallback: TextRange,
    ) -> ExprId {
        let segments: Vec<SyntaxToken> = name.map(|n| n.segments().collect()).unwrap_or_default();
        let mut iter = segments.into_iter();
        let Some(first) = iter.next() else {
            return self.alloc_expr(Expr::Error, fallback);
        };
        let mut base = self.alloc_name_expr(first.text(), first.text_range());
        for segment in iter {
            let seg_range = segment.text_range();
            let key = self.alloc_expr(
                Expr::Literal(Literal::String(LitStr {
                    value: Some(segment.text().as_bytes().to_vec()),
                    is_long: false,
                })),
                seg_range,
            );
            base = self.alloc_expr(
                Expr::Index {
                    base,
                    index: key,
                    from_field: true,
                },
                seg_range,
            );
        }
        base
    }

    fn lower_local_function(&mut self, decl: &ast::LocalFunctionStmt, range: TextRange) -> StmtId {
        // The name is in scope inside the body (recursion), so declare first.
        let (text, name_range) = token_text(decl.name().as_ref(), range);
        let binding = self.declare_binding(text, BindingKind::LocalFunction, name_range);
        let func = self.lower_function(decl.param_list(), decl.body().as_ref(), false, range, None);
        self.alloc_stmt(Stmt::LocalFunction { binding, func }, range)
    }

    fn lower_label(&mut self, label: &ast::LabelStmt, range: TextRange) -> StmtId {
        let (name, name_range) = token_text(label.name().as_ref(), range);
        let body = self.cur_body();
        let id = self.labels.alloc(Label {
            name: name.clone(),
            body,
            range: name_range,
        });
        self.alloc_stmt(Stmt::Label { name, label: id }, range)
    }

    // === Functions ===

    fn lower_function(
        &mut self,
        param_list: Option<ast::ParamList>,
        body: Option<&ast::Block>,
        implicit_self: bool,
        range: TextRange,
        self_range: Option<TextRange>,
    ) -> ExprId {
        let parent = self.cur_body();
        let id = self.start_body(Some(parent));

        if implicit_self {
            let self_binding = self.declare_binding(
                "self".to_string(),
                BindingKind::SelfParam,
                self_range.unwrap_or(range),
            );
            self.body_mut(id).params.push(self_binding);
        }
        if let Some(param_list) = param_list {
            for param in param_list.params() {
                if param.is_vararg() {
                    self.body_mut(id).is_vararg = true;
                } else {
                    let (text, param_range) = token_text(param.name().as_ref(), range);
                    let binding = self.declare_binding(text, BindingKind::Param, param_range);
                    self.body_mut(id).params.push(binding);
                }
            }
        }

        let block = self.lower_block_in_scope(body);
        self.body_mut(id).block = block;
        self.finish_body();

        self.alloc_expr(Expr::Function(id), range)
    }

    // === Expressions ===

    fn lower_expr_list(&mut self, list: Option<ast::ExprList>) -> Vec<ExprId> {
        list.map(|list| list.exprs().map(|e| self.lower_expr(&e)).collect())
            .unwrap_or_default()
    }

    fn lower_expr_list_opt(&mut self, list: Option<ast::ExprList>) -> Vec<ExprId> {
        self.lower_expr_list(list)
    }

    fn lower_expr_opt(&mut self, expr: Option<ast::Expr>, fallback: TextRange) -> ExprId {
        match expr {
            Some(expr) => self.lower_expr(&expr),
            None => self.alloc_expr(Expr::Error, fallback),
        }
    }

    fn lower_expr(&mut self, expr: &ast::Expr) -> ExprId {
        let range = expr.syntax().text_range();
        match expr {
            ast::Expr::Name(name) => {
                let (text, _) = token_text(name.name().as_ref(), range);
                self.alloc_name_expr(&text, range)
            }
            ast::Expr::Literal(lit) => {
                let value = lower_literal(lit);
                self.alloc_expr(Expr::Literal(value), range)
            }
            ast::Expr::Vararg(_) => self.alloc_expr(Expr::Vararg, range),
            ast::Expr::Paren(paren) => self.lower_paren(paren, range),
            ast::Expr::Prefix(prefix) => {
                let op = unary_op(prefix.op_token().as_ref());
                let operand = self.lower_expr_opt(prefix.operand(), range);
                self.alloc_expr(Expr::Unary { op, operand }, range)
            }
            ast::Expr::Bin(bin) => {
                let op = binary_op(bin.op_token().as_ref());
                let lhs = self.lower_expr_opt(bin.lhs(), range);
                let rhs = self.lower_expr_opt(bin.rhs(), range);
                self.alloc_expr(Expr::Binary { op, lhs, rhs }, range)
            }
            ast::Expr::Function(func) => {
                self.lower_function(func.param_list(), func.body().as_ref(), false, range, None)
            }
            ast::Expr::Table(table) => self.lower_table(table, range),
            ast::Expr::Call(call) => self.lower_call(call, range),
            ast::Expr::MethodCall(call) => self.lower_method_call(call, range),
            ast::Expr::Index(index) => {
                let base = self.lower_expr_opt(index.base(), range);
                let key = self.lower_expr_opt(index.index(), range);
                self.alloc_expr(
                    Expr::Index {
                        base,
                        index: key,
                        from_field: false,
                    },
                    range,
                )
            }
            ast::Expr::Field(field) => self.lower_field(field, range),
        }
    }

    fn lower_paren(&mut self, paren: &ast::ParenExpr, range: TextRange) -> ExprId {
        let Some(inner) = paren.inner() else {
            return self.alloc_expr(Expr::Error, range);
        };
        let inner = self.lower_expr(&inner);
        // Parens are erased, except when they truncate a multi-value producer
        // to a single value.
        if self.is_multi_value(inner) {
            self.alloc_expr(Expr::Truncate(inner), range)
        } else {
            inner
        }
    }

    fn is_multi_value(&self, expr: ExprId) -> bool {
        matches!(
            self.body_ref(self.cur_body()).expr(expr),
            Expr::Call { .. } | Expr::MethodCall { .. } | Expr::Vararg
        )
    }

    fn lower_field(&mut self, field: &ast::FieldExpr, range: TextRange) -> ExprId {
        let base = self.lower_expr_opt(field.base(), range);
        let (name, name_range) = token_text(field.field_name().as_ref(), range);
        let key = self.alloc_expr(
            Expr::Literal(Literal::String(LitStr {
                value: Some(name.into_bytes()),
                is_long: false,
            })),
            name_range,
        );
        self.alloc_expr(
            Expr::Index {
                base,
                index: key,
                from_field: true,
            },
            range,
        )
    }

    fn lower_call(&mut self, call: &ast::CallExpr, range: TextRange) -> ExprId {
        let callee = self.lower_expr_opt(call.callee(), range);
        let args = self.lower_args(call.args());
        self.detect_require(callee, &args, range);
        self.alloc_expr(Expr::Call { callee, args }, range)
    }

    fn lower_method_call(&mut self, call: &ast::MethodCallExpr, range: TextRange) -> ExprId {
        let receiver = self.lower_expr_opt(call.receiver(), range);
        let (method, _) = token_text(call.method_name().as_ref(), range);
        let args = self.lower_args(call.args());
        self.alloc_expr(
            Expr::MethodCall {
                receiver,
                method,
                args,
            },
            range,
        )
    }

    fn lower_args(&mut self, args: Option<ast::ArgList>) -> Vec<ExprId> {
        let Some(args) = args else {
            return Vec::new();
        };
        if let Some(list) = args.expr_list() {
            list.exprs().map(|e| self.lower_expr(&e)).collect()
        } else if let Some(table) = args.table_arg() {
            let range = table.syntax().text_range();
            vec![self.lower_table(&table, range)]
        } else if let Some(string) = args.string_arg() {
            let value = decode_string(string.text());
            vec![self.alloc_expr(Expr::Literal(Literal::String(value)), string.text_range())]
        } else {
            Vec::new()
        }
    }

    fn lower_table(&mut self, table: &ast::TableExpr, range: TextRange) -> ExprId {
        let mut entries = Vec::new();
        for field in table.fields() {
            let field_range = field.syntax().text_range();
            match field {
                ast::TableField::Item(item) => {
                    let value = self.lower_expr_opt(item.value(), field_range);
                    entries.push(TableEntry::Positional(value));
                }
                ast::TableField::Name(named) => {
                    let (name, _) = token_text(named.name().as_ref(), field_range);
                    let value = self.lower_expr_opt(named.value(), field_range);
                    entries.push(TableEntry::Named { name, value });
                }
                ast::TableField::Key(keyed) => {
                    let key = self.lower_expr_opt(keyed.key(), field_range);
                    let value = self.lower_expr_opt(keyed.value(), field_range);
                    entries.push(TableEntry::Keyed { key, value });
                }
            }
        }
        self.alloc_expr(Expr::Table { entries }, range)
    }

    /// Record a `require` edge when a call is `require(<literal>)` with the
    /// global `require`. Non-literal or shadowed-`require` calls are recorded
    /// as dynamic requires.
    fn detect_require(&mut self, callee: ExprId, args: &[ExprId], range: TextRange) {
        let body = self.cur_body();
        let is_require =
            matches!(self.body_ref(body).expr(callee), Expr::Name(n) if n == "require");
        if !is_require {
            return;
        }
        if !matches!(
            self.resolutions.get(&HirId::expr(body, callee)),
            Some(Resolution::Global(_))
        ) {
            return;
        }
        let module = if args.len() == 1 {
            match self.body_ref(body).expr(args[0]) {
                Expr::Literal(Literal::String(s)) => s
                    .value
                    .as_ref()
                    .map(|bytes| String::from_utf8_lossy(bytes).into_owned()),
                _ => None,
            }
        } else {
            None
        };
        match module {
            Some(module) => self.requires.push(RequireEdge { module, range }),
            None => self.dynamic_requires.push(DynamicRequire { range }),
        }
    }

    // === Finalization ===

    fn finish(mut self, chunk: BodyId) -> LoweredFile {
        let mut bodies = Arena::new();
        for body in self.bodies.drain(..) {
            bodies.alloc(body.expect("body finalized"));
        }
        resolve_gotos(&mut bodies);
        LoweredFile::new(
            bodies,
            self.bindings,
            self.labels,
            chunk,
            self.source_map,
            self.resolutions,
            self.requires,
            self.dynamic_requires,
        )
    }
}

// === goto/label resolution (post-pass) ===

/// Resolve every `goto` in every body to a visible label. A label is visible
/// in the block that defines it and all nested blocks (Lua scoping); we walk
/// each function's block tree with a stack of label sets and bind each goto to
/// the innermost matching label. All labels of a block are collected before
/// its gotos, so forward gotos resolve.
fn resolve_gotos(bodies: &mut Arena<Body>) {
    let ids: Vec<BodyId> = bodies.iter().map(|(id, _)| id).collect();
    for id in ids {
        let fixes = {
            let body = &bodies[id];
            let mut fixes = Vec::new();
            let mut stack = Vec::new();
            collect_goto_fixes(body, &body.block, &mut stack, &mut fixes);
            fixes
        };
        for (stmt, label) in fixes {
            if let Stmt::Goto { target, .. } = bodies[id].stmt_mut(stmt) {
                *target = Some(label);
            }
        }
    }
}

fn collect_goto_fixes(
    body: &Body,
    block: &Block,
    stack: &mut Vec<HashMap<String, LabelId>>,
    fixes: &mut Vec<(StmtId, LabelId)>,
) {
    let mut labels = HashMap::new();
    for &stmt in &block.stmts {
        if let Stmt::Label { name, label } = body.stmt(stmt) {
            labels.insert(name.clone(), *label);
        }
    }
    stack.push(labels);
    for &stmt in &block.stmts {
        match body.stmt(stmt) {
            Stmt::Goto { name, .. } => {
                if let Some(label) = stack.iter().rev().find_map(|m| m.get(name).copied()) {
                    fixes.push((stmt, label));
                }
            }
            Stmt::Do { body: inner }
            | Stmt::While { body: inner, .. }
            | Stmt::Repeat { body: inner, .. }
            | Stmt::NumericFor { body: inner, .. }
            | Stmt::GenericFor { body: inner, .. } => {
                collect_goto_fixes(body, inner, stack, fixes);
            }
            Stmt::If {
                branches,
                else_block,
            } => {
                for branch in branches {
                    collect_goto_fixes(body, &branch.block, stack, fixes);
                }
                if let Some(else_block) = else_block {
                    collect_goto_fixes(body, else_block, stack, fixes);
                }
            }
            _ => {}
        }
    }
    stack.pop();
}

// === Token / operator helpers ===

/// The text and range of an optional token, falling back to `fallback` range
/// (and an empty name) when the parse recovered a missing token.
fn token_text(token: Option<&SyntaxToken>, fallback: TextRange) -> (String, TextRange) {
    match token {
        Some(token) => (token.text().to_string(), token.text_range()),
        None => (String::new(), fallback),
    }
}

fn lower_literal(lit: &ast::LiteralExpr) -> Literal {
    let Some(token) = lit.token() else {
        return Literal::Nil;
    };
    match token.kind() {
        SyntaxKind::NUMBER => Literal::Number(parse_number(token.text())),
        SyntaxKind::STRING => Literal::String(decode_string(token.text())),
        SyntaxKind::TRUE_KW => Literal::Bool(true),
        SyntaxKind::FALSE_KW => Literal::Bool(false),
        _ => Literal::Nil,
    }
}

fn binary_op(token: Option<&SyntaxToken>) -> BinOp {
    match token.map(SyntaxToken::kind) {
        // The `_` arm is PLUS plus the defensive default for broken parses
        // (the parser only builds BIN_EXPR around a real operator token).
        Some(SyntaxKind::MINUS) => BinOp::Sub,
        Some(SyntaxKind::STAR) => BinOp::Mul,
        Some(SyntaxKind::SLASH) => BinOp::Div,
        Some(SyntaxKind::SLASH_SLASH) => BinOp::IDiv,
        Some(SyntaxKind::PERCENT) => BinOp::Mod,
        Some(SyntaxKind::CARET) => BinOp::Pow,
        Some(SyntaxKind::DOT_DOT) => BinOp::Concat,
        Some(SyntaxKind::EQ_EQ) => BinOp::Eq,
        Some(SyntaxKind::TILDE_EQ) => BinOp::Ne,
        Some(SyntaxKind::LT) => BinOp::Lt,
        Some(SyntaxKind::LT_EQ) => BinOp::Le,
        Some(SyntaxKind::GT) => BinOp::Gt,
        Some(SyntaxKind::GT_EQ) => BinOp::Ge,
        Some(SyntaxKind::AND_KW) => BinOp::And,
        Some(SyntaxKind::OR_KW) => BinOp::Or,
        Some(SyntaxKind::AMP) => BinOp::BAnd,
        Some(SyntaxKind::PIPE) => BinOp::BOr,
        Some(SyntaxKind::TILDE) => BinOp::BXor,
        Some(SyntaxKind::LT_LT) => BinOp::Shl,
        Some(SyntaxKind::GT_GT) => BinOp::Shr,
        _ => BinOp::Add,
    }
}

fn unary_op(token: Option<&SyntaxToken>) -> UnOp {
    match token.map(SyntaxToken::kind) {
        Some(SyntaxKind::NOT_KW) => UnOp::Not,
        Some(SyntaxKind::HASH) => UnOp::Len,
        Some(SyntaxKind::TILDE) => UnOp::BNot,
        // MINUS, or a defensive default.
        _ => UnOp::Neg,
    }
}
