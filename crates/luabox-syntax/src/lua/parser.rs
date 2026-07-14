//! Green-tree parser: builds lossless rowan trees over the [`lex`] tokens.
//!
//! Invariants:
//! - `parse(text, d).syntax().text() == text` for every input (losslessness).
//! - Never panics on any input: unexpected tokens land in `ERROR_NODE`s and
//!   are reported as [`ParseError`]s; a nesting-depth limit keeps recursion
//!   bounded. (Inputs beyond rowan's `u32` offset space are the sole,
//!   documented exception.)

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, Language, TextRange, TextSize, WalkEvent};

use super::{Dialect, LuaLanguage, SyntaxKind, ast, grammar, lex};

/// Syntax tree node over [`LuaLanguage`].
pub type SyntaxNode = rowan::SyntaxNode<LuaLanguage>;
/// Syntax tree token over [`LuaLanguage`].
pub type SyntaxToken = rowan::SyntaxToken<LuaLanguage>;
/// Node-or-token over [`LuaLanguage`].
pub type SyntaxElement = rowan::SyntaxElement<LuaLanguage>;

/// A parse-time diagnostic anchored to a byte range of the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub range: TextRange,
}

/// Result of [`parse`]: the lossless green tree plus collected errors.
///
/// The tree text is byte-identical to the input even when `errors` is
/// non-empty.
#[derive(Debug, Clone)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<ParseError>,
}

impl Parse {
    /// The root as a red (cursor) node; the root kind is always
    /// [`SyntaxKind::SOURCE_FILE`].
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    /// The shared green root (cheap to clone).
    pub fn green(&self) -> GreenNode {
        self.green.clone()
    }

    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// The typed AST root. Infallible: the parser always emits a
    /// `SOURCE_FILE` root.
    pub fn tree(&self) -> ast::SourceFile {
        ast::AstNode::cast(self.syntax())
            .unwrap_or_else(|| unreachable!("the parser always emits a SOURCE_FILE root"))
    }

    /// Indented kind/range/text dump of the tree followed by the errors —
    /// the format tree-shape tests assert against.
    pub fn debug_dump(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let mut indent = 0usize;
        for event in self.syntax().preorder_with_tokens() {
            match event {
                WalkEvent::Enter(element) => {
                    let _ = write!(out, "{:indent$}", "", indent = indent * 2);
                    match element {
                        rowan::NodeOrToken::Node(node) => {
                            let _ = writeln!(out, "{:?}@{:?}", node.kind(), node.text_range());
                            indent += 1;
                        }
                        rowan::NodeOrToken::Token(token) => {
                            let _ = writeln!(
                                out,
                                "{:?}@{:?} {:?}",
                                token.kind(),
                                token.text_range(),
                                token.text()
                            );
                        }
                    }
                }
                WalkEvent::Leave(rowan::NodeOrToken::Node(_)) => indent -= 1,
                WalkEvent::Leave(rowan::NodeOrToken::Token(_)) => {}
            }
        }
        for error in &self.errors {
            let _ = writeln!(out, "error {:?}: {}", error.range, error.message);
        }
        out
    }
}

/// Parse `text` under `dialect` into a lossless tree.
///
/// The grammar is the union of Lua 5.1–5.4 and LuaJIT; constructs illegal in
/// `dialect` still parse (a later validation pass diagnoses them). `dialect`
/// only affects tokenization (`goto` keyword-ness, LuaJIT number suffixes).
pub fn parse(text: &str, dialect: Dialect) -> Parse {
    let mut parser = Parser::new(text, dialect);
    grammar::source_file(&mut parser);
    debug_assert_eq!(
        parser.pos,
        parser.tokens.len(),
        "parser must consume every token"
    );
    Parse {
        green: parser.builder.finish(),
        errors: parser.errors,
    }
}

/// Recursion budget shared by statement and expression nesting; deeper input
/// degrades into `ERROR_NODE`s instead of overflowing the stack.
pub(super) const MAX_DEPTH: u32 = 100;

/// Tree-height budget. Rowan's green trees drop recursively, so unbounded
/// height (e.g. a 50k-term `+` chain, one `BIN_EXPR` level per operator)
/// would overflow the stack on drop; past this budget the parser stops
/// wrapping at checkpoints and appends flat children instead (lossless, one
/// error reported).
const MAX_HEIGHT: u32 = 512;

/// A position that a later node can wrap back to: a rowan checkpoint plus
/// the height-bookkeeping index that mirrors it.
#[derive(Clone, Copy)]
pub(super) struct Marker {
    checkpoint: Checkpoint,
    child_index: usize,
}

pub(super) struct Parser<'a> {
    text: &'a str,
    /// `(kind, start, end)` byte offsets; the tokens tile `text` exactly.
    tokens: Vec<(SyntaxKind, usize, usize)>,
    /// Index of the next unconsumed token (trivia included).
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
    pub(super) depth: u32,
    depth_reported: bool,
    /// Height bookkeeping mirroring the rowan builder's flat child list:
    /// one entry per finished child (tokens are 1); [`Self::finish_node`]
    /// folds a node's slice into a single `max + 1`. Powers
    /// [`Self::can_wrap`].
    child_heights: Vec<u32>,
    /// First-child index in `child_heights` for each open node.
    open_nodes: Vec<usize>,
    height_reported: bool,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, dialect: Dialect) -> Self {
        let mut tokens = Vec::new();
        let mut offset = 0usize;
        for token in lex(text, dialect) {
            let end = offset + token.len as usize;
            tokens.push((token.kind, offset, end));
            offset = end;
        }
        Parser {
            text,
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            depth: 0,
            depth_reported: false,
            child_heights: Vec::new(),
            open_nodes: Vec::new(),
            height_reported: false,
        }
    }

    /// Index of the first non-trivia token at or after `i`.
    fn skip_trivia_from(&self, mut i: usize) -> usize {
        while i < self.tokens.len() && self.tokens[i].0.is_trivia() {
            i += 1;
        }
        i
    }

    /// Kind of the next non-trivia token (`None` at end of input).
    pub(super) fn current(&self) -> Option<SyntaxKind> {
        self.nth(0)
    }

    /// Kind of the `n`th non-trivia token ahead.
    pub(super) fn nth(&self, n: usize) -> Option<SyntaxKind> {
        let mut i = self.skip_trivia_from(self.pos);
        for _ in 0..n {
            i = self.skip_trivia_from(i + 1);
        }
        self.tokens.get(i).map(|&(kind, ..)| kind)
    }

    pub(super) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == Some(kind)
    }

    /// Text of the next non-trivia token (empty at end of input).
    #[expect(
        clippy::string_slice,
        reason = "token (start, end) spans accumulate lexer token lengths, which tile the input on char boundaries"
    )]
    pub(super) fn current_text(&self) -> &str {
        let i = self.skip_trivia_from(self.pos);
        self.tokens
            .get(i)
            .map_or("", |&(_, start, end)| &self.text[start..end])
    }

    /// Byte range of the next non-trivia token (empty range at end of input).
    pub(super) fn current_range(&self) -> TextRange {
        let i = self.skip_trivia_from(self.pos);
        match self.tokens.get(i) {
            Some(&(_, start, end)) => TextRange::new(text_size(start), text_size(end)),
            None => TextRange::empty(text_size(self.text.len())),
        }
    }

    #[expect(
        clippy::string_slice,
        reason = "token (start, end) spans accumulate lexer token lengths, which tile the input on char boundaries"
    )]
    fn push_token(&mut self) {
        let (kind, start, end) = self.tokens[self.pos];
        self.builder
            .token(LuaLanguage::kind_to_raw(kind), &self.text[start..end]);
        self.pos += 1;
        self.child_heights.push(1);
    }

    /// Attach any remaining end-of-input trivia to the open node; called by
    /// the root rule after the top-level block has consumed every non-trivia
    /// token.
    pub(super) fn bump_remaining_trivia(&mut self) {
        self.flush_trivia();
        debug_assert!(
            self.pos == self.tokens.len(),
            "non-trivia left after top-level block"
        );
    }

    /// Attach pending trivia to the currently open node.
    fn flush_trivia(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].0.is_trivia() {
            self.push_token();
        }
    }

    /// Attach leading trivia plus the current token to the open node.
    /// No-op at end of input.
    pub(super) fn bump(&mut self) {
        self.flush_trivia();
        if self.pos < self.tokens.len() {
            self.push_token();
        }
    }

    pub(super) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consume `kind` or report `expected …` at the current token without
    /// consuming anything.
    pub(super) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("expected {}", describe(kind)));
        false
    }

    /// Open a node; pending trivia stays in the parent. The root node opens
    /// before any trivia is flushed, so leading trivia lands inside it.
    pub(super) fn start_node(&mut self, kind: SyntaxKind) {
        if !self.open_nodes.is_empty() {
            self.flush_trivia();
        }
        self.builder.start_node(LuaLanguage::kind_to_raw(kind));
        self.open_nodes.push(self.child_heights.len());
    }

    /// A marker that a later node can wrap back to; pending trivia is
    /// flushed first so it stays outside the wrapped node.
    pub(super) fn checkpoint(&mut self) -> Marker {
        self.flush_trivia();
        Marker {
            checkpoint: self.builder.checkpoint(),
            child_index: self.child_heights.len(),
        }
    }

    /// Wrap everything built since `marker` into a new `kind` node.
    /// Callers in loops must gate this on [`Self::can_wrap`].
    pub(super) fn start_node_at(&mut self, marker: Marker, kind: SyntaxKind) {
        self.builder
            .start_node_at(marker.checkpoint, LuaLanguage::kind_to_raw(kind));
        self.open_nodes.push(marker.child_index);
    }

    pub(super) fn finish_node(&mut self) {
        self.builder.finish_node();
        #[expect(
            clippy::expect_used,
            reason = "every finish_node is structurally paired with a prior start_node/start_node_at that pushed onto open_nodes"
        )]
        let first_child = self
            .open_nodes
            .pop()
            .expect("finish_node without matching start_node");
        let height = self.subtree_height(first_child) + 1;
        self.child_heights.truncate(first_child);
        self.child_heights.push(height);
    }

    /// Max height among the finished children at or after `first_child`.
    fn subtree_height(&self, first_child: usize) -> u32 {
        self.child_heights
            .get(first_child..)
            .unwrap_or(&[])
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
    }

    /// Whether wrapping at `marker` stays inside the tree-height budget;
    /// reports once per parse when the budget runs out. Callers keep
    /// consuming without wrapping after a `false`.
    pub(super) fn can_wrap(&mut self, marker: Marker) -> bool {
        if self.subtree_height(marker.child_index) < MAX_HEIGHT {
            return true;
        }
        if !self.height_reported {
            self.height_reported = true;
            self.error("expression too complex");
        }
        false
    }

    pub(super) fn error(&mut self, message: impl Into<String>) {
        let range = self.current_range();
        self.errors.push(ParseError {
            message: message.into(),
            range,
        });
    }

    /// Report `message` and consume the current token inside an
    /// `ERROR_NODE` (guaranteed progress).
    pub(super) fn error_and_bump(&mut self, message: impl Into<String>) {
        self.error(message);
        self.bump_into_error_node();
    }

    /// Consume the current token inside an `ERROR_NODE` without reporting.
    /// No-op at end of input.
    pub(super) fn bump_into_error_node(&mut self) {
        if self.current().is_some() {
            self.start_node(SyntaxKind::ERROR_NODE);
            self.bump();
            self.finish_node();
        }
    }

    /// Nesting-limit bailout: report once per parse, then consume one token
    /// so every caller's loop still makes progress.
    pub(super) fn depth_error(&mut self) {
        if !self.depth_reported {
            self.depth_reported = true;
            self.error("nesting limit exceeded");
        }
        self.bump_into_error_node();
    }
}

/// Offsets are bounded by rowan's `u32` text sizes; the lexer already
/// enforces per-token bounds, so this only guards pathological >4 GiB input.
#[expect(
    clippy::expect_used,
    reason = "rowan models text offsets as u32 TextSize; the whole syntax tree is built on that bound, so >4 GiB input is out of scope"
)]
fn text_size(offset: usize) -> TextSize {
    TextSize::try_from(offset).expect("input larger than 4 GiB")
}

/// Human-readable token name for `expected …` diagnostics.
fn describe(kind: SyntaxKind) -> &'static str {
    #[allow(
        clippy::enum_glob_use,
        reason = "glob-importing the SyntaxKind variants keeps the match arms below readable"
    )]
    use SyntaxKind::*;
    match kind {
        IDENT => "a name",
        NUMBER => "a number",
        STRING => "a string",
        AND_KW => "'and'",
        BREAK_KW => "'break'",
        DO_KW => "'do'",
        ELSE_KW => "'else'",
        ELSEIF_KW => "'elseif'",
        END_KW => "'end'",
        FALSE_KW => "'false'",
        FOR_KW => "'for'",
        FUNCTION_KW => "'function'",
        GOTO_KW => "'goto'",
        IF_KW => "'if'",
        IN_KW => "'in'",
        LOCAL_KW => "'local'",
        NIL_KW => "'nil'",
        NOT_KW => "'not'",
        OR_KW => "'or'",
        REPEAT_KW => "'repeat'",
        RETURN_KW => "'return'",
        THEN_KW => "'then'",
        TRUE_KW => "'true'",
        UNTIL_KW => "'until'",
        WHILE_KW => "'while'",
        PLUS => "'+'",
        MINUS => "'-'",
        STAR => "'*'",
        SLASH => "'/'",
        PERCENT => "'%'",
        CARET => "'^'",
        HASH => "'#'",
        AMP => "'&'",
        TILDE => "'~'",
        PIPE => "'|'",
        LT_LT => "'<<'",
        GT_GT => "'>>'",
        SLASH_SLASH => "'//'",
        EQ => "'='",
        EQ_EQ => "'=='",
        TILDE_EQ => "'~='",
        LT_EQ => "'<='",
        GT_EQ => "'>='",
        LT => "'<'",
        GT => "'>'",
        L_PAREN => "'('",
        R_PAREN => "')'",
        L_BRACE => "'{'",
        R_BRACE => "'}'",
        L_BRACKET => "'['",
        R_BRACKET => "']'",
        SEMICOLON => "';'",
        COLON => "':'",
        COLON_COLON => "'::'",
        COMMA => "','",
        DOT => "'.'",
        DOT_DOT => "'..'",
        DOT_DOT_DOT => "'...'",
        _ => "a token",
    }
}

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::lua::ast::{self, AstNode};
    use proptest::prelude::*;

    /// Valid Lua 5.4 programs covering the whole statement grammar; reused
    /// by the mutation property test.
    const CORPUS: &[&str] = &[
        "local function fib(n)\n  if n < 2 then return n end\n  return fib(n - 1) + fib(n - 2)\nend\nprint(fib(10))\n",
        "local t <const> = { 1, 2, x = 'y', ['z'] = [[w]], f(); }\nfor i = 1, #t, 2 do t[i] = t[i] * 2 ^ i end\nfor k, v in pairs(t) do io.write(k, '=', tostring(v), '\\n') end\n",
        "::top::\nlocal i = 0\nwhile true do\n  i = i + 1\n  if i & 3 == 0 then goto top end\n  repeat i = i // 2 until i < 1 or i ~ 5 == 0\n  break\nend\n",
        "function obj.ns:method(a, b, ...)\n  local args = { ... }\n  return self, select('#', ...)\nend\nobj = setmetatable({}, { __index = function(_, k) return k end })\nobj:method 'lit' -- string call\nobj:method { 1, 2 }\ndo local x <close> = open() end\nreturn obj\n",
    ];

    fn assert_lossless(parse: &Parse, text: &str) {
        assert_eq!(
            parse.syntax().text().to_string(),
            text,
            "tree text must be byte-identical to the input\n{}",
            parse.debug_dump()
        );
    }

    /// Parse under Lua 5.4; assert losslessness and zero errors.
    fn ok(text: &str) -> Parse {
        let parse = parse(text, Dialect::Lua54);
        assert_lossless(&parse, text);
        assert_eq!(
            parse.errors(),
            &[],
            "unexpected errors\n{}",
            parse.debug_dump()
        );
        parse
    }

    /// Parse under Lua 5.4; assert losslessness and at least one error.
    fn err(text: &str) -> Parse {
        let parse = parse(text, Dialect::Lua54);
        assert_lossless(&parse, text);
        assert!(
            !parse.errors().is_empty(),
            "expected parse errors\n{}",
            parse.debug_dump()
        );
        parse
    }

    fn first_stmt(parse: &Parse) -> ast::Stmt {
        parse
            .tree()
            .block()
            .expect("block")
            .stmts()
            .next()
            .expect("statement")
    }

    /// The first expression of `return <text>`, for precedence tests.
    fn expr_of(text: &str) -> ast::Expr {
        let parse = ok(&format!("return {text}"));
        let ast::Stmt::Return(ret) = first_stmt(&parse) else {
            panic!("expected return statement");
        };
        ret.exprs().expect("exprs").exprs().next().expect("expr")
    }

    /// Render an expression as an S-expression keyed by operators.
    fn sexp(expr: &ast::Expr) -> String {
        match expr {
            ast::Expr::Name(_) | ast::Expr::Literal(_) | ast::Expr::Vararg(_) => {
                expr.syntax().text().to_string()
            }
            ast::Expr::Paren(paren) => {
                let inner = paren.inner().map(|e| sexp(&e)).unwrap_or_default();
                format!("(paren {inner})")
            }
            ast::Expr::Prefix(prefix) => {
                let op = prefix
                    .op_token()
                    .map(|t| t.text().to_string())
                    .unwrap_or_default();
                let operand = prefix.operand().map(|e| sexp(&e)).unwrap_or_default();
                format!("({op} {operand})")
            }
            ast::Expr::Bin(bin) => {
                let op = bin
                    .op_token()
                    .map(|t| t.text().to_string())
                    .unwrap_or_default();
                let lhs = bin.lhs().map(|e| sexp(&e)).unwrap_or_default();
                let rhs = bin.rhs().map(|e| sexp(&e)).unwrap_or_default();
                format!("({op} {lhs} {rhs})")
            }
            other => format!("<{:?}>", other.syntax().kind()),
        }
    }

    fn assert_sexp(text: &str, expected: &str) {
        assert_eq!(sexp(&expr_of(text)), expected, "for input {text:?}");
    }

    // === Tree shape ===

    #[test]
    fn dump_local_stmt() {
        let parse = ok("local x = 1");
        let expected = "\
SOURCE_FILE@0..11
  BLOCK@0..11
    LOCAL_STMT@0..11
      LOCAL_KW@0..5 \"local\"
      WHITESPACE@5..6 \" \"
      LOCAL_NAME@6..7
        IDENT@6..7 \"x\"
      WHITESPACE@7..8 \" \"
      EQ@8..9 \"=\"
      WHITESPACE@9..10 \" \"
      EXPR_LIST@10..11
        LITERAL_EXPR@10..11
          NUMBER@10..11 \"1\"
";
        assert_eq!(parse.debug_dump(), expected);
    }

    #[test]
    fn dump_call_stmt() {
        let parse = ok("f(1)");
        let expected = "\
SOURCE_FILE@0..4
  BLOCK@0..4
    CALL_STMT@0..4
      CALL_EXPR@0..4
        NAME_EXPR@0..1
          IDENT@0..1 \"f\"
        ARG_LIST@1..4
          L_PAREN@1..2 \"(\"
          EXPR_LIST@2..3
            LITERAL_EXPR@2..3
              NUMBER@2..3 \"1\"
          R_PAREN@3..4 \")\"
";
        assert_eq!(parse.debug_dump(), expected);
    }

    // === Statements ===

    #[test]
    fn local_with_attribs() {
        let parse = ok("local x <const>, y <close> = 1, 2");
        let ast::Stmt::Local(local) = first_stmt(&parse) else {
            panic!("expected local");
        };
        let names: Vec<_> = local.names().collect();
        assert_eq!(names.len(), 2);
        assert!(names[0].attrib().unwrap().is_const());
        assert!(names[1].attrib().unwrap().is_close());
        assert_eq!(local.values().unwrap().exprs().count(), 2);
    }

    #[test]
    fn local_without_values() {
        ok("local a, b, c");
    }

    #[test]
    fn multi_target_assignment() {
        let parse = ok("a, b.c, d[1] = 1, 2, 3");
        let ast::Stmt::Assign(assign) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
        let target_kinds: Vec<_> = assign
            .targets()
            .unwrap()
            .exprs()
            .map(|e| e.syntax().kind())
            .collect();
        assert_eq!(
            target_kinds,
            [
                SyntaxKind::NAME_EXPR,
                SyntaxKind::FIELD_EXPR,
                SyntaxKind::INDEX_EXPR
            ]
        );
        assert_eq!(assign.values().unwrap().exprs().count(), 3);
    }

    #[test]
    fn function_decl_with_method_path() {
        let parse = ok("function a.b.c:d(x, y, ...) return x end");
        let ast::Stmt::FunctionDecl(decl) = first_stmt(&parse) else {
            panic!("expected function decl");
        };
        let name = decl.name().unwrap();
        let segments: Vec<_> = name.segments().map(|t| t.text().to_string()).collect();
        assert_eq!(segments, ["a", "b", "c", "d"]);
        assert!(name.is_method());
        assert_eq!(name.method_name().unwrap().text(), "d");
        let params: Vec<_> = decl.param_list().unwrap().params().collect();
        assert_eq!(params.len(), 3);
        assert!(params[2].is_vararg());
    }

    #[test]
    fn local_function() {
        let parse = ok("local function f(a) return a end");
        let ast::Stmt::LocalFunction(decl) = first_stmt(&parse) else {
            panic!("expected local function");
        };
        assert_eq!(decl.name().unwrap().text(), "f");
        assert_eq!(decl.param_list().unwrap().params().count(), 1);
    }

    #[test]
    fn call_statement_forms() {
        ok("f()");
        ok("f 'lit'");
        ok("f [[long]]");
        ok("f { 1, 2 }");
        ok("t:m(1)");
        ok("t.a:b 'x'");
        ok("(f)()");
    }

    #[test]
    fn if_elseif_else() {
        let parse = ok("if a then x = 1 elseif b then x = 2 elseif c then x = 3 else x = 4 end");
        let ast::Stmt::If(if_stmt) = first_stmt(&parse) else {
            panic!("expected if");
        };
        assert!(if_stmt.condition().is_some());
        assert!(if_stmt.then_block().is_some());
        assert_eq!(if_stmt.elseif_clauses().count(), 2);
        assert!(if_stmt.else_clause().is_some());
    }

    #[test]
    fn while_and_repeat() {
        ok("while x < 10 do x = x + 1 end");
        let parse = ok("repeat f() until done");
        let ast::Stmt::Repeat(repeat) = first_stmt(&parse) else {
            panic!("expected repeat");
        };
        assert!(repeat.body().is_some());
        assert!(repeat.condition().is_some());
    }

    #[test]
    fn numeric_for() {
        let parse = ok("for i = 1, 10, 2 do print(i) end");
        let ast::Stmt::NumericFor(for_stmt) = first_stmt(&parse) else {
            panic!("expected numeric for");
        };
        assert_eq!(for_stmt.var().unwrap().text(), "i");
        assert!(for_stmt.start().is_some());
        assert!(for_stmt.end().is_some());
        assert!(for_stmt.step().is_some());
        ok("for i = 1, 10 do end");
    }

    #[test]
    fn generic_for() {
        let parse = ok("for k, v in pairs(t) do end");
        let ast::Stmt::GenericFor(for_stmt) = first_stmt(&parse) else {
            panic!("expected generic for");
        };
        let vars: Vec<_> = for_stmt.vars().map(|t| t.text().to_string()).collect();
        assert_eq!(vars, ["k", "v"]);
        assert_eq!(for_stmt.exprs().unwrap().exprs().count(), 1);
    }

    #[test]
    fn do_break_and_semicolons() {
        ok("do break end");
        ok(";;; f() ;; g() ;");
    }

    #[test]
    fn return_forms() {
        ok("return");
        ok("return 1, 2;");
        ok("return f()");
        ok("do return end f()");
    }

    #[test]
    fn goto_and_labels() {
        ok("::top:: goto top");
        let parse = parse("::top:: goto top", Dialect::LuaJit);
        assert_lossless(&parse, "::top:: goto top");
        assert_eq!(parse.errors(), &[]);
    }

    #[test]
    fn goto_is_an_identifier_in_51() {
        // 5.1 has no goto statement; `goto` is an ordinary variable name.
        let parse = parse("goto = 1", Dialect::Lua51);
        assert_lossless(&parse, "goto = 1");
        assert_eq!(parse.errors(), &[], "{}", parse.debug_dump());
        let ast::Stmt::Assign(_) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
    }

    #[test]
    fn table_constructor_fields() {
        let parse = ok("t = { [k] = v, name = 2, 3, 'positional'; f(), }");
        let ast::Stmt::Assign(assign) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
        let Some(ast::Expr::Table(table)) = assign.values().unwrap().exprs().next() else {
            panic!("expected table");
        };
        let fields: Vec<_> = table.fields().collect();
        assert_eq!(fields.len(), 5);
        assert!(matches!(fields[0], ast::TableField::Key(_)));
        assert!(matches!(fields[1], ast::TableField::Name(_)));
        assert!(matches!(fields[2], ast::TableField::Item(_)));
        assert!(matches!(fields[3], ast::TableField::Item(_)));
        assert!(matches!(fields[4], ast::TableField::Item(_)));
    }

    #[test]
    fn table_constructor_requires_separator_between_fields() {
        let parse = parse("t = {1 2}", Dialect::Lua54);
        assert_lossless(&parse, "t = {1 2}");
        assert_eq!(
            parse.errors(),
            &[ParseError {
                message: "expected ',' or ';' between table fields".to_string(),
                range: TextRange::new(TextSize::from(7), TextSize::from(8)),
            }],
            "{}",
            parse.debug_dump()
        );
        let ast::Stmt::Assign(assign) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
        let Some(ast::Expr::Table(table)) = assign.values().unwrap().exprs().next() else {
            panic!("expected table");
        };
        let fields: Vec<_> = table.fields().collect();
        assert_eq!(fields.len(), 2, "{}", parse.debug_dump());
        assert!(fields.iter().all(|f| matches!(f, ast::TableField::Item(_))));
    }

    #[test]
    fn table_constructor_separators_are_clean() {
        ok("t = {1, 2}");
        ok("t = {1; 2}");
        ok("t = {1, 2,}");
    }

    #[test]
    fn table_constructor_missing_separator_between_calls_errors() {
        err("t = {f() g()}");
    }

    #[test]
    fn nested_table_missing_separator_errors_once() {
        let parse = err("t = { {1 2} }");
        assert_eq!(parse.errors().len(), 1, "{}", parse.debug_dump());
    }

    #[test]
    fn table_constructor_missing_separator_recovers_when_unterminated() {
        // Unclosed on top of a missing separator: recovers without looping,
        // and both fields still make it into the tree.
        let parse = err("t = {1 2");
        assert_lossless(&parse, "t = {1 2");
        let ast::Stmt::Assign(assign) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
        let Some(ast::Expr::Table(table)) = assign.values().unwrap().exprs().next() else {
            panic!("expected table");
        };
        assert_eq!(table.fields().count(), 2, "{}", parse.debug_dump());
    }

    #[test]
    fn function_expr_and_varargs() {
        ok("local f = function(...) return ... end");
        ok("f(...)");
    }

    #[test]
    fn union_operators_parse_in_every_dialect() {
        // `//`, bitops, and attribs are 5.3/5.4 features but part of the
        // union grammar: they must parse under 5.1 too (validated later).
        let src = "x = a // b % c << d >> e & f | g ~ h  local y <const> = 1";
        for dialect in Dialect::ALL {
            let parse = parse(src, dialect);
            assert_lossless(&parse, src);
            assert_eq!(
                parse.errors(),
                &[],
                "dialect {dialect:?}\n{}",
                parse.debug_dump()
            );
        }
    }

    #[test]
    fn method_call_chain() {
        let parse = ok("a.b:c(1)(2)[3].d = 1");
        let ast::Stmt::Assign(assign) = first_stmt(&parse) else {
            panic!("expected assignment");
        };
        let target = assign.targets().unwrap().exprs().next().unwrap();
        // ((((a.b):c(1))(2))[3]).d
        let ast::Expr::Field(field) = target else {
            panic!("outermost should be field access");
        };
        let ast::Expr::Index(index) = field.base().unwrap() else {
            panic!("then index");
        };
        let ast::Expr::Call(call) = index.base().unwrap() else {
            panic!("then call");
        };
        let ast::Expr::MethodCall(method) = call.callee().unwrap() else {
            panic!("then method call");
        };
        assert_eq!(method.method_name().unwrap().text(), "c");
        let ast::Expr::Field(inner) = method.receiver().unwrap() else {
            panic!("innermost is a.b");
        };
        assert_eq!(inner.field_name().unwrap().text(), "b");
    }

    #[test]
    fn corpus_parses_cleanly() {
        for source in CORPUS {
            let parse = parse(source, Dialect::Lua54);
            assert_lossless(&parse, source);
            assert_eq!(
                parse.errors(),
                &[],
                "in program:\n{source}\n{}",
                parse.debug_dump()
            );
        }
    }

    // === Precedence & associativity ===

    #[test]
    fn precedence_mul_over_add() {
        assert_sexp("1+2*3", "(+ 1 (* 2 3))");
    }

    #[test]
    fn precedence_pow_right_assoc() {
        assert_sexp("2^3^2", "(^ 2 (^ 3 2))");
    }

    #[test]
    fn precedence_pow_binds_tighter_than_unary() {
        assert_sexp("-2^2", "(- (^ 2 2))");
        assert_sexp("2 ^ -3", "(^ 2 (- 3))");
    }

    #[test]
    fn precedence_not_vs_comparison() {
        assert_sexp("not a == b", "(== (not a) b)");
    }

    #[test]
    fn precedence_concat_right_assoc() {
        assert_sexp("a .. b .. c", "(.. a (.. b c))");
        assert_sexp("a + b .. c + d", "(.. (+ a b) (+ c d))");
    }

    #[test]
    fn precedence_and_or() {
        assert_sexp("1 < 2 and 2 < 3 or x", "(or (and (< 1 2) (< 2 3)) x)");
    }

    #[test]
    fn precedence_bitwise_ladder() {
        assert_sexp("a | b ~ c & d << e + f", "(| a (~ b (& c (<< d (+ e f)))))");
    }

    #[test]
    fn precedence_len_unary() {
        assert_sexp("#t + 1", "(+ (# t) 1)");
    }

    #[test]
    fn precedence_parens_override() {
        assert_sexp("(1 + 2) * 3", "(* (paren (+ 1 2)) 3)");
    }

    // === Error resilience ===

    #[test]
    fn broken_inputs_are_lossless_with_errors() {
        err("local = 5");
        err("if x then");
        err("f(");
        err("x = ");
        err("end");
        err("a b c");
        err("function f( end");
        err("t = {1, 2");
        err("x = 1 + )");
        err("else x = 1 end");
        err("until true");
        err("= = =");
        err("local x <");
    }

    #[test]
    fn statement_after_return_is_reported_but_parsed() {
        let parse = err("return 1 return 2");
        // Both returns are in the tree.
        let stmts: Vec<_> = parse.tree().block().unwrap().stmts().collect();
        assert_eq!(stmts.len(), 2);
        assert!(
            parse
                .errors()
                .iter()
                .any(|e| e.message.contains("after 'return'"))
        );
    }

    #[test]
    fn unterminated_string_recovers_on_next_line() {
        let text = "x = 'oops\ny = 1";
        let parse = err(text);
        // The second assignment still parses.
        let stmts: Vec<_> = parse.tree().block().unwrap().stmts().collect();
        assert!(
            stmts.iter().any(
                |s| matches!(s, ast::Stmt::Assign(a) if a.syntax().text().to_string().starts_with('y'))
            ),
            "{}",
            parse.debug_dump()
        );
    }

    #[test]
    fn recovery_produces_error_nodes_not_lost_tokens() {
        let parse = err("local x = 1 ??? local y = 2");
        assert!(
            parse
                .syntax()
                .descendants()
                .any(|n| n.kind() == SyntaxKind::ERROR_NODE),
            "{}",
            parse.debug_dump()
        );
        // Recovery resynchronizes: both locals are real statements.
        let locals = parse
            .tree()
            .block()
            .unwrap()
            .stmts()
            .filter(|s| matches!(s, ast::Stmt::Local(_)))
            .count();
        assert_eq!(locals, 2);
    }

    #[test]
    fn goto_in_51_source_recovers() {
        // `goto top` under 5.1 is two identifiers — an error, but lossless.
        let text = "goto top";
        let parse = parse(text, Dialect::Lua51);
        assert_lossless(&parse, text);
        assert!(!parse.errors().is_empty());
    }

    #[test]
    fn deep_paren_nesting_does_not_overflow() {
        let text = format!("x = {}1{}", "(".repeat(10_000), ")".repeat(10_000));
        let parse = parse(&text, Dialect::Lua54);
        assert_lossless(&parse, &text);
        assert!(!parse.errors().is_empty());
    }

    #[test]
    fn deep_block_nesting_does_not_overflow() {
        let text = "do ".repeat(10_000);
        let parse = parse(&text, Dialect::Lua54);
        assert_lossless(&parse, &text);
        assert!(!parse.errors().is_empty());
    }

    #[test]
    fn deep_right_assoc_chains_do_not_overflow() {
        for text in [
            format!("x = {}1", "1 .. ".repeat(10_000)),
            format!("x = {}1", "- ".repeat(10_000)),
        ] {
            let parse = parse(&text, Dialect::Lua54);
            assert_lossless(&parse, &text);
        }
    }

    #[test]
    fn long_flat_chains_stay_within_tree_height_budget() {
        // Valid programs: without the height budget these would build
        // 50k-deep trees (one level per operator/suffix) and overflow the
        // stack when the green tree drops.
        for text in [
            format!("x = 1{}", " + 1".repeat(50_000)),
            format!("x = t{}", ".f".repeat(50_000)),
            format!("x = f{}", "()".repeat(50_000)),
        ] {
            let parse = parse(&text, Dialect::Lua54);
            assert_lossless(&parse, &text);
            // The height budget reports (once) instead of crashing.
            assert_eq!(parse.errors().len(), 1, "{:?}", parse.errors());
            assert_eq!(parse.errors()[0].message, "expression too complex");
        }
    }

    // === Property tests ===

    const SNIPPETS: &[&str] = &[
        "end", "then", "(", ")", "==", "local", "..", "[[", "]]", "'", "\"", "0x", "...", "::",
        "<const>", "--", "\n", "goto", ";", ",", "{", "}", "=", "function", "^", "~",
    ];

    /// Splice/delete/truncate at a char boundary near `at`.
    fn mutate(source: &str, at: usize, action: u8, snippet: &str) -> String {
        let mut pos = at.min(source.len());
        while pos > 0 && !source.is_char_boundary(pos) {
            pos -= 1;
        }
        match action {
            0 => format!("{}{}{}", &source[..pos], snippet, &source[pos..]),
            1 => {
                let mut end = (pos + 8).min(source.len());
                while end < source.len() && !source.is_char_boundary(end) {
                    end += 1;
                }
                format!("{}{}", &source[..pos], &source[end..])
            }
            _ => source[..pos].to_string(),
        }
    }

    proptest! {
        #[test]
        fn arbitrary_input_never_panics_and_stays_lossless(text in any::<String>()) {
            for dialect in Dialect::ALL {
                let parse = parse(&text, dialect);
                prop_assert_eq!(parse.syntax().text().to_string(), text.as_str());
            }
        }

        #[test]
        fn corpus_mutations_never_panic_and_stay_lossless(
            index in 0..CORPUS.len(),
            at in any::<prop::sample::Index>(),
            action in 0u8..3,
            snippet in prop::sample::select(SNIPPETS.to_vec()),
        ) {
            let source = CORPUS[index];
            let mutated = mutate(source, at.index(source.len() + 1), action, snippet);
            for dialect in Dialect::ALL {
                let parse = parse(&mutated, dialect);
                prop_assert_eq!(parse.syntax().text().to_string(), mutated.as_str());
            }
        }
    }
}
