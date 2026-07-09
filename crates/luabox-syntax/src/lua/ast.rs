//! Typed AST: thin, zero-cost wrappers over [`SyntaxNode`]s
//! (rust-analyzer style). Every accessor returns `Option`/iterators because
//! trees from recovered parses can miss any child.

use super::SyntaxKind;
use super::parser::{SyntaxNode, SyntaxToken};

/// A typed view over a [`SyntaxNode`] of one specific kind (or a closed set
/// of kinds, for the enums).
pub trait AstNode: Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(syntax: SyntaxNode) -> Option<Self>;
    fn syntax(&self) -> &SyntaxNode;
}

fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
    parent.children().find_map(N::cast)
}

fn nth_child<N: AstNode>(parent: &SyntaxNode, n: usize) -> Option<N> {
    parent.children().filter_map(N::cast).nth(n)
}

fn children<N: AstNode>(parent: &SyntaxNode) -> impl Iterator<Item = N> {
    parent.children().filter_map(N::cast)
}

fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|it| it.kind() == kind)
}

fn tokens(parent: &SyntaxNode, kind: SyntaxKind) -> impl Iterator<Item = SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(move |it| it.kind() == kind)
}

/// The first non-trivia token child — for nodes whose only significant
/// token is an operator (`BIN_EXPR`, `PREFIX_EXPR`).
fn first_token(parent: &SyntaxNode) -> Option<SyntaxToken> {
    parent
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|it| !it.kind().is_trivia())
}

macro_rules! ast_node {
    ($(#[$attr:meta])* $name:ident, $kind:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(SyntaxNode);

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                kind == SyntaxKind::$kind
            }
            fn cast(syntax: SyntaxNode) -> Option<Self> {
                Self::can_cast(syntax.kind()).then(|| Self(syntax))
            }
            fn syntax(&self) -> &SyntaxNode {
                &self.0
            }
        }
    };
}

/// Defines a closed enum over node wrappers, dispatching `cast` by kind.
macro_rules! ast_enum {
    ($(#[$attr:meta])* $name:ident { $($variant:ident($ty:ident),)* }) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant($ty),)*
        }

        impl AstNode for $name {
            fn can_cast(kind: SyntaxKind) -> bool {
                $($ty::can_cast(kind))||*
            }
            fn cast(syntax: SyntaxNode) -> Option<Self> {
                $(if let Some(node) = $ty::cast(syntax.clone()) {
                    return Some(Self::$variant(node));
                })*
                None
            }
            fn syntax(&self) -> &SyntaxNode {
                match self {
                    $(Self::$variant(node) => node.syntax(),)*
                }
            }
        }
    };
}

// === Root & block ===

ast_node!(SourceFile, SOURCE_FILE);
ast_node!(Block, BLOCK);

impl SourceFile {
    pub fn block(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl Block {
    pub fn stmts(&self) -> impl Iterator<Item = Stmt> {
        children(&self.0)
    }
}

// === Statements ===

ast_node!(LocalStmt, LOCAL_STMT);
ast_node!(LocalName, LOCAL_NAME);
ast_node!(NameAttrib, NAME_ATTRIB);
ast_node!(AssignStmt, ASSIGN_STMT);
ast_node!(CallStmt, CALL_STMT);
ast_node!(DoStmt, DO_STMT);
ast_node!(WhileStmt, WHILE_STMT);
ast_node!(RepeatStmt, REPEAT_STMT);
ast_node!(IfStmt, IF_STMT);
ast_node!(ElseifClause, ELSEIF_CLAUSE);
ast_node!(ElseClause, ELSE_CLAUSE);
ast_node!(NumericForStmt, NUMERIC_FOR_STMT);
ast_node!(GenericForStmt, GENERIC_FOR_STMT);
ast_node!(FunctionDeclStmt, FUNCTION_DECL_STMT);
ast_node!(FunctionName, FUNCTION_NAME);
ast_node!(LocalFunctionStmt, LOCAL_FUNCTION_STMT);
ast_node!(ReturnStmt, RETURN_STMT);
ast_node!(BreakStmt, BREAK_STMT);
ast_node!(GotoStmt, GOTO_STMT);
ast_node!(LabelStmt, LABEL_STMT);

ast_enum!(
    /// Any statement node (semicolons are bare tokens, not statements).
    Stmt {
        Local(LocalStmt),
        Assign(AssignStmt),
        Call(CallStmt),
        Do(DoStmt),
        While(WhileStmt),
        Repeat(RepeatStmt),
        If(IfStmt),
        NumericFor(NumericForStmt),
        GenericFor(GenericForStmt),
        FunctionDecl(FunctionDeclStmt),
        LocalFunction(LocalFunctionStmt),
        Return(ReturnStmt),
        Break(BreakStmt),
        Goto(GotoStmt),
        Label(LabelStmt),
    }
);

impl LocalStmt {
    pub fn names(&self) -> impl Iterator<Item = LocalName> {
        children(&self.0)
    }
    pub fn values(&self) -> Option<ExprList> {
        child(&self.0)
    }
}

impl LocalName {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn attrib(&self) -> Option<NameAttrib> {
        child(&self.0)
    }
}

impl NameAttrib {
    /// The attribute identifier (`const` / `close`).
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn is_const(&self) -> bool {
        self.name().is_some_and(|it| it.text() == "const")
    }
    pub fn is_close(&self) -> bool {
        self.name().is_some_and(|it| it.text() == "close")
    }
}

impl AssignStmt {
    /// The assignment targets (the `EXPR_LIST` before `=`).
    pub fn targets(&self) -> Option<ExprList> {
        nth_child(&self.0, 0)
    }
    /// The assigned values (the `EXPR_LIST` after `=`).
    pub fn values(&self) -> Option<ExprList> {
        nth_child(&self.0, 1)
    }
}

impl CallStmt {
    pub fn expr(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl DoStmt {
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl WhileStmt {
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl RepeatStmt {
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
    /// The `until` condition.
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl IfStmt {
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
    pub fn then_block(&self) -> Option<Block> {
        child(&self.0)
    }
    pub fn elseif_clauses(&self) -> impl Iterator<Item = ElseifClause> {
        children(&self.0)
    }
    pub fn else_clause(&self) -> Option<ElseClause> {
        child(&self.0)
    }
}

impl ElseifClause {
    pub fn condition(&self) -> Option<Expr> {
        child(&self.0)
    }
    pub fn block(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl ElseClause {
    pub fn block(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl NumericForStmt {
    pub fn var(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn start(&self) -> Option<Expr> {
        nth_child(&self.0, 0)
    }
    pub fn end(&self) -> Option<Expr> {
        nth_child(&self.0, 1)
    }
    pub fn step(&self) -> Option<Expr> {
        nth_child(&self.0, 2)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl GenericForStmt {
    /// The loop names (direct `IDENT` tokens; the iterator expressions live
    /// inside the `EXPR_LIST`).
    pub fn vars(&self) -> impl Iterator<Item = SyntaxToken> {
        tokens(&self.0, SyntaxKind::IDENT)
    }
    pub fn exprs(&self) -> Option<ExprList> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl FunctionDeclStmt {
    pub fn name(&self) -> Option<FunctionName> {
        child(&self.0)
    }
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl FunctionName {
    /// All path segments, method name included: `a.b.c:d` → `a, b, c, d`.
    pub fn segments(&self) -> impl Iterator<Item = SyntaxToken> {
        tokens(&self.0, SyntaxKind::IDENT)
    }
    /// True for `a.b:m`-style names (declares a method with implicit `self`).
    pub fn is_method(&self) -> bool {
        token(&self.0, SyntaxKind::COLON).is_some()
    }
    /// The name after `:`, when [`Self::is_method`].
    pub fn method_name(&self) -> Option<SyntaxToken> {
        if self.is_method() {
            self.segments().last()
        } else {
            None
        }
    }
}

impl LocalFunctionStmt {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl ReturnStmt {
    pub fn exprs(&self) -> Option<ExprList> {
        child(&self.0)
    }
}

impl GotoStmt {
    pub fn label(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
}

impl LabelStmt {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
}

// === Expressions ===

ast_node!(NameExpr, NAME_EXPR);
ast_node!(LiteralExpr, LITERAL_EXPR);
ast_node!(VarargExpr, VARARG_EXPR);
ast_node!(ParenExpr, PAREN_EXPR);
ast_node!(PrefixExpr, PREFIX_EXPR);
ast_node!(BinExpr, BIN_EXPR);
ast_node!(FunctionExpr, FUNCTION_EXPR);
ast_node!(TableExpr, TABLE_EXPR);
ast_node!(CallExpr, CALL_EXPR);
ast_node!(MethodCallExpr, METHOD_CALL_EXPR);
ast_node!(IndexExpr, INDEX_EXPR);
ast_node!(FieldExpr, FIELD_EXPR);

ast_enum!(
    /// Any expression node.
    Expr {
        Name(NameExpr),
        Literal(LiteralExpr),
        Vararg(VarargExpr),
        Paren(ParenExpr),
        Prefix(PrefixExpr),
        Bin(BinExpr),
        Function(FunctionExpr),
        Table(TableExpr),
        Call(CallExpr),
        MethodCall(MethodCallExpr),
        Index(IndexExpr),
        Field(FieldExpr),
    }
);

impl NameExpr {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
}

impl LiteralExpr {
    /// The literal token: NUMBER, STRING, `nil`, `true`, or `false`.
    pub fn token(&self) -> Option<SyntaxToken> {
        first_token(&self.0)
    }
}

impl ParenExpr {
    pub fn inner(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl PrefixExpr {
    /// The unary operator token (`not`, `#`, `-`, `~`).
    pub fn op_token(&self) -> Option<SyntaxToken> {
        first_token(&self.0)
    }
    pub fn operand(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl BinExpr {
    pub fn lhs(&self) -> Option<Expr> {
        nth_child(&self.0, 0)
    }
    pub fn rhs(&self) -> Option<Expr> {
        nth_child(&self.0, 1)
    }
    /// The operator token (the only non-trivia token child).
    pub fn op_token(&self) -> Option<SyntaxToken> {
        first_token(&self.0)
    }
}

impl FunctionExpr {
    pub fn param_list(&self) -> Option<ParamList> {
        child(&self.0)
    }
    pub fn body(&self) -> Option<Block> {
        child(&self.0)
    }
}

impl TableExpr {
    pub fn fields(&self) -> impl Iterator<Item = TableField> {
        children(&self.0)
    }
}

impl CallExpr {
    pub fn callee(&self) -> Option<Expr> {
        child(&self.0)
    }
    pub fn args(&self) -> Option<ArgList> {
        child(&self.0)
    }
}

impl MethodCallExpr {
    pub fn receiver(&self) -> Option<Expr> {
        child(&self.0)
    }
    /// The method name after `:`.
    pub fn method_name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn args(&self) -> Option<ArgList> {
        child(&self.0)
    }
}

impl IndexExpr {
    pub fn base(&self) -> Option<Expr> {
        nth_child(&self.0, 0)
    }
    pub fn index(&self) -> Option<Expr> {
        nth_child(&self.0, 1)
    }
}

impl FieldExpr {
    pub fn base(&self) -> Option<Expr> {
        child(&self.0)
    }
    /// The field name after `.`.
    pub fn field_name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
}

// === Support ===

ast_node!(ParamList, PARAM_LIST);
ast_node!(Param, PARAM);
ast_node!(ArgList, ARG_LIST);
ast_node!(ExprList, EXPR_LIST);
ast_node!(TableKeyField, TABLE_KEY_FIELD);
ast_node!(TableNameField, TABLE_NAME_FIELD);
ast_node!(TableItemField, TABLE_ITEM_FIELD);

ast_enum!(
    /// One table-constructor entry.
    TableField {
        Key(TableKeyField),
        Name(TableNameField),
        Item(TableItemField),
    }
);

impl ParamList {
    pub fn params(&self) -> impl Iterator<Item = Param> {
        children(&self.0)
    }
}

impl Param {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn is_vararg(&self) -> bool {
        token(&self.0, SyntaxKind::DOT_DOT_DOT).is_some()
    }
}

impl ArgList {
    /// Parenthesized arguments, when present (`f(a, b)`).
    pub fn expr_list(&self) -> Option<ExprList> {
        child(&self.0)
    }
    /// The sole table argument, for `f { … }` calls.
    pub fn table_arg(&self) -> Option<TableExpr> {
        child(&self.0)
    }
    /// The sole string argument, for `f "s"` calls.
    pub fn string_arg(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::STRING)
    }
}

impl ExprList {
    pub fn exprs(&self) -> impl Iterator<Item = Expr> {
        children(&self.0)
    }
}

impl TableKeyField {
    /// The bracketed key expression.
    pub fn key(&self) -> Option<Expr> {
        nth_child(&self.0, 0)
    }
    pub fn value(&self) -> Option<Expr> {
        nth_child(&self.0, 1)
    }
}

impl TableNameField {
    pub fn name(&self) -> Option<SyntaxToken> {
        token(&self.0, SyntaxKind::IDENT)
    }
    pub fn value(&self) -> Option<Expr> {
        child(&self.0)
    }
}

impl TableItemField {
    pub fn value(&self) -> Option<Expr> {
        child(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::{Dialect, parse};

    fn tree(text: &str) -> SourceFile {
        let parse = parse(text, Dialect::Lua54);
        assert_eq!(parse.errors(), &[], "{}", parse.debug_dump());
        parse.tree()
    }

    fn stmts(file: &SourceFile) -> Vec<Stmt> {
        file.block().expect("block").stmts().collect()
    }

    #[test]
    fn if_stmt_accessors() {
        let file = tree("if a < 1 then f() elseif b then g() else h() end");
        let [Stmt::If(if_stmt)] = &stmts(&file)[..] else {
            panic!("expected one if");
        };
        assert_eq!(
            if_stmt.condition().unwrap().syntax().text().to_string(),
            "a < 1"
        );
        assert_eq!(if_stmt.then_block().unwrap().stmts().count(), 1);
        let clauses: Vec<_> = if_stmt.elseif_clauses().collect();
        assert_eq!(clauses.len(), 1);
        assert_eq!(
            clauses[0].condition().unwrap().syntax().text().to_string(),
            "b"
        );
        assert!(clauses[0].block().is_some());
        assert_eq!(
            if_stmt
                .else_clause()
                .unwrap()
                .block()
                .unwrap()
                .stmts()
                .count(),
            1
        );
    }

    #[test]
    fn while_and_numeric_for_accessors() {
        let file = tree("while ready do step() end for i = 1, 10 do end");
        let all = stmts(&file);
        let Stmt::While(while_stmt) = &all[0] else {
            panic!("expected while");
        };
        assert_eq!(
            while_stmt.condition().unwrap().syntax().text().to_string(),
            "ready"
        );
        assert!(while_stmt.body().is_some());
        let Stmt::NumericFor(for_stmt) = &all[1] else {
            panic!("expected numeric for");
        };
        assert_eq!(for_stmt.var().unwrap().text(), "i");
        assert_eq!(for_stmt.start().unwrap().syntax().text().to_string(), "1");
        assert_eq!(for_stmt.end().unwrap().syntax().text().to_string(), "10");
        assert!(for_stmt.step().is_none());
        assert!(for_stmt.body().is_some());
    }

    #[test]
    fn local_stmt_accessors() {
        let file = tree("local a <const>, b = 1, 'two'");
        let [Stmt::Local(local)] = &stmts(&file)[..] else {
            panic!("expected local");
        };
        let names: Vec<_> = local.names().collect();
        assert_eq!(names[0].name().unwrap().text(), "a");
        assert_eq!(names[0].attrib().unwrap().name().unwrap().text(), "const");
        assert!(!names[0].attrib().unwrap().is_close());
        assert_eq!(names[1].name().unwrap().text(), "b");
        assert!(names[1].attrib().is_none());
        let values: Vec<_> = local.values().unwrap().exprs().collect();
        assert!(matches!(values[..], [Expr::Literal(_), Expr::Literal(_)]));
    }

    #[test]
    fn call_and_arg_list_accessors() {
        let file = tree("f(1, 2) g 's' h { x = 1 }");
        let all = stmts(&file);
        let calls: Vec<CallExpr> = all
            .iter()
            .map(|s| {
                let Stmt::Call(call) = s else {
                    panic!("expected call stmt");
                };
                let Some(Expr::Call(expr)) = call.expr() else {
                    panic!("expected call expr");
                };
                expr
            })
            .collect();
        assert_eq!(
            calls[0]
                .args()
                .unwrap()
                .expr_list()
                .unwrap()
                .exprs()
                .count(),
            2
        );
        assert_eq!(calls[1].args().unwrap().string_arg().unwrap().text(), "'s'");
        let table = calls[2].args().unwrap().table_arg().unwrap();
        let [TableField::Name(field)] = &table.fields().collect::<Vec<_>>()[..] else {
            panic!("expected one name field");
        };
        assert_eq!(field.name().unwrap().text(), "x");
        assert_eq!(field.value().unwrap().syntax().text().to_string(), "1");
    }

    #[test]
    fn table_key_field_accessors() {
        let file = tree("t = { [1 + 1] = 'two' }");
        let [Stmt::Assign(assign)] = &stmts(&file)[..] else {
            panic!("expected assignment");
        };
        let Some(Expr::Table(table)) = assign.values().unwrap().exprs().next() else {
            panic!("expected table");
        };
        let [TableField::Key(field)] = &table.fields().collect::<Vec<_>>()[..] else {
            panic!("expected one key field");
        };
        assert_eq!(field.key().unwrap().syntax().text().to_string(), "1 + 1");
        assert_eq!(field.value().unwrap().syntax().text().to_string(), "'two'");
    }

    #[test]
    fn bin_and_prefix_expr_accessors() {
        let file = tree("return -a + b");
        let [Stmt::Return(ret)] = &stmts(&file)[..] else {
            panic!("expected return");
        };
        let Some(Expr::Bin(bin)) = ret.exprs().unwrap().exprs().next() else {
            panic!("expected binary expr");
        };
        assert_eq!(bin.op_token().unwrap().kind(), SyntaxKind::PLUS);
        let Some(Expr::Prefix(prefix)) = bin.lhs() else {
            panic!("expected prefix lhs");
        };
        assert_eq!(prefix.op_token().unwrap().kind(), SyntaxKind::MINUS);
        assert_eq!(prefix.operand().unwrap().syntax().text().to_string(), "a");
        assert_eq!(bin.rhs().unwrap().syntax().text().to_string(), "b");
    }

    #[test]
    fn function_nodes_accessors() {
        let file = tree("function m.n:o(p, ...) end local q = function() end");
        let all = stmts(&file);
        let Stmt::FunctionDecl(decl) = &all[0] else {
            panic!("expected function decl");
        };
        let name = decl.name().unwrap();
        assert!(name.is_method());
        assert_eq!(name.method_name().unwrap().text(), "o");
        let params: Vec<_> = decl.param_list().unwrap().params().collect();
        assert_eq!(params[0].name().unwrap().text(), "p");
        assert!(params[1].is_vararg());
        assert!(decl.body().is_some());
        let Stmt::Local(local) = &all[1] else {
            panic!("expected local");
        };
        let Some(Expr::Function(func)) = local.values().unwrap().exprs().next() else {
            panic!("expected function expr");
        };
        assert_eq!(func.param_list().unwrap().params().count(), 0);
        assert!(func.body().is_some());
    }

    #[test]
    fn misc_stmt_accessors() {
        let file = tree("::top:: do goto top end repeat break until done");
        let all = stmts(&file);
        let Stmt::Label(label) = &all[0] else {
            panic!("expected label");
        };
        assert_eq!(label.name().unwrap().text(), "top");
        let Stmt::Do(do_stmt) = &all[1] else {
            panic!("expected do");
        };
        let [Stmt::Goto(goto_stmt)] = &do_stmt.body().unwrap().stmts().collect::<Vec<_>>()[..]
        else {
            panic!("expected goto");
        };
        assert_eq!(goto_stmt.label().unwrap().text(), "top");
        let Stmt::Repeat(repeat) = &all[2] else {
            panic!("expected repeat");
        };
        assert!(matches!(
            repeat.body().unwrap().stmts().next(),
            Some(Stmt::Break(_))
        ));
        assert_eq!(
            repeat.condition().unwrap().syntax().text().to_string(),
            "done"
        );
    }

    #[test]
    fn generic_for_accessors() {
        let file = tree("for k, v in next, t do end");
        let [Stmt::GenericFor(for_stmt)] = &stmts(&file)[..] else {
            panic!("expected generic for");
        };
        let vars: Vec<_> = for_stmt.vars().map(|t| t.text().to_string()).collect();
        assert_eq!(vars, ["k", "v"]);
        assert_eq!(for_stmt.exprs().unwrap().exprs().count(), 2);
        assert!(for_stmt.body().is_some());
    }

    #[test]
    fn cast_rejects_other_kinds() {
        let file = tree("f()");
        let root = file.syntax().clone();
        assert!(IfStmt::cast(root.clone()).is_none());
        assert!(SourceFile::cast(root).is_some());
    }
}
