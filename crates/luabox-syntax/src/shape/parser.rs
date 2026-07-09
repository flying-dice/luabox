//! Recursive-descent, error-resilient parser for the `.lb` shape grammar
//! (SHAPES.md §3), producing a lossless rowan green tree.
//!
//! Design notes:
//! - **Lossless.** Every lexed token — trivia included — is emitted into the
//!   tree exactly once, so `parse(text).syntax().to_string() == text` for any
//!   input. This is asserted by the property test at the bottom of the file.
//! - **Leading trivia nests inward.** Item/field/fn nodes are opened *before*
//!   their first token is bumped, so preceding doc comments and comments land
//!   inside the node they document (the formatter relies on this).
//! - **Never panics.** Unexpected tokens become [`ShapeSyntaxKind::ERROR_NODE`]
//!   spans and parsing resynchronises at the next item keyword. Bodies (a `{`
//!   where a trait-fn signature expects `;`) are rejected with `LB2010` and
//!   skipped over balanced braces.

use rowan::{GreenNode, GreenNodeBuilder, TextRange, TextSize};

use super::{ShapeLanguage, ShapeSyntaxKind, ShapeSyntaxNode, lex};
use ShapeSyntaxKind::{
    ARROW, COLON, COMMA, DOT, DOT_DOT, EQ, ERROR_NODE, FIELD, FN_KW, FN_TYPE, FOR_KW, GENERIC_ARGS,
    GENERIC_PARAM, GENERIC_PARAMS, IDENT, IMPL_DEF, IMPL_KW, L_ANGLE, L_BRACE, L_PAREN,
    OPEN_MARKER, OPTIONAL_TYPE, PARAM, PARAM_LIST, PAREN_TYPE, PIPE, PLUS, QUESTION, R_ANGLE,
    R_BRACE, R_PAREN, RET_TYPE, SELF_KW, SEMICOLON, SHAPE_FILE, STRUCT_DEF, STRUCT_KW, SUPERTRAITS,
    TRAIT_DEF, TRAIT_FN, TRAIT_KW, TYPE_ALIAS, TYPE_KW, TYPE_REF, UNION_TYPE, USE_DECL, USE_KW,
};

/// The exact `LB2010` message required by SHAPES.md §3 / §5.
pub const LB2010_MESSAGE: &str = "implementations live in .lua - bind with ---@impl";

/// A single parse diagnostic. `code` is the `LB2xxx` string when one applies
/// (only `LB2010` is raised at parse time); syntax errors carry `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Diagnostic code (e.g. `"LB2010"`), or `None` for a plain syntax error.
    pub code: Option<&'static str>,
    /// Human-readable message.
    pub message: String,
    /// Byte range in the source the error covers.
    pub range: TextRange,
}

/// The result of [`parse`]: a lossless green tree plus any diagnostics.
#[derive(Debug, Clone)]
pub struct ShapeParse {
    green: GreenNode,
    errors: Vec<ParseError>,
}

impl ShapeParse {
    /// The typed root syntax node (a `SHAPE_FILE`).
    #[must_use]
    pub fn syntax(&self) -> ShapeSyntaxNode {
        ShapeSyntaxNode::new_root(self.green.clone())
    }

    /// The parse diagnostics, in source order.
    #[must_use]
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }

    /// The raw green node (for callers that build their own tree views).
    #[must_use]
    pub fn green(&self) -> &GreenNode {
        &self.green
    }
}

/// Parse `.lb` source into a lossless green tree. Never fails, never panics:
/// malformed input yields a tree with `ERROR_NODE` spans and diagnostics.
#[must_use]
pub fn parse(text: &str) -> ShapeParse {
    let raw = lex(text);
    let mut tokens = Vec::with_capacity(raw.len());
    let mut off = 0usize;
    for t in raw {
        let len = t.len as usize;
        tokens.push(PToken {
            kind: t.kind,
            text: &text[off..off + len],
            start: off,
        });
        off += len;
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        text_len: text.len(),
        consumed_end: 0,
        builder: GreenNodeBuilder::new(),
        errors: Vec::new(),
    };
    parser.file();
    let (green, errors) = parser.finish_parse();
    ShapeParse { green, errors }
}

#[derive(Clone, Copy)]
struct PToken<'a> {
    kind: ShapeSyntaxKind,
    text: &'a str,
    start: usize,
}

struct Parser<'a> {
    tokens: Vec<PToken<'a>>,
    pos: usize,
    text_len: usize,
    consumed_end: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
}

fn raw(k: ShapeSyntaxKind) -> rowan::SyntaxKind {
    <ShapeLanguage as rowan::Language>::kind_to_raw(k)
}

fn is_item_start(k: ShapeSyntaxKind) -> bool {
    matches!(k, STRUCT_KW | TRAIT_KW | IMPL_KW | TYPE_KW | USE_KW)
}

fn describe(k: ShapeSyntaxKind) -> &'static str {
    match k {
        IDENT => "an identifier",
        L_BRACE => "`{`",
        R_BRACE => "`}`",
        L_PAREN => "`(`",
        R_PAREN => "`)`",
        R_ANGLE => "`>`",
        COLON => "`:`",
        SEMICOLON => "`;`",
        EQ => "`=`",
        FOR_KW => "`for`",
        _ => "a token",
    }
}

impl Parser<'_> {
    // --- green-tree plumbing --------------------------------------------

    fn start(&mut self, k: ShapeSyntaxKind) {
        self.builder.start_node(raw(k));
    }

    fn start_at(&mut self, cp: rowan::Checkpoint, k: ShapeSyntaxKind) {
        self.builder.start_node_at(cp, raw(k));
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }

    fn checkpoint(&self) -> rowan::Checkpoint {
        self.builder.checkpoint()
    }

    fn finish_parse(self) -> (GreenNode, Vec<ParseError>) {
        (self.builder.finish(), self.errors)
    }

    // --- token cursor ---------------------------------------------------

    /// Emit pending trivia (whitespace/comments) into the current node.
    fn eat_trivia(&mut self) {
        while let Some(t) = self.tokens.get(self.pos).copied() {
            if t.kind.is_trivia() {
                self.builder.token(raw(t.kind), t.text);
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Emit any pending trivia, then the next significant token.
    fn bump(&mut self) {
        self.eat_trivia();
        if let Some(t) = self.tokens.get(self.pos).copied() {
            self.builder.token(raw(t.kind), t.text);
            self.consumed_end = t.start + t.text.len();
            self.pos += 1;
        }
    }

    /// Kind of the `n`-th significant token ahead (trivia skipped).
    fn nth(&self, n: usize) -> Option<ShapeSyntaxKind> {
        let mut seen = 0;
        for t in &self.tokens[self.pos..] {
            if !t.kind.is_trivia() {
                if seen == n {
                    return Some(t.kind);
                }
                seen += 1;
            }
        }
        None
    }

    fn current(&self) -> Option<ShapeSyntaxKind> {
        self.nth(0)
    }

    fn at(&self, k: ShapeSyntaxKind) -> bool {
        self.current() == Some(k)
    }

    /// Byte offset of the next significant token (or EOF).
    fn current_start(&self) -> usize {
        self.tokens[self.pos..]
            .iter()
            .find(|t| !t.kind.is_trivia())
            .map_or(self.text_len, |t| t.start)
    }

    fn eat(&mut self, k: ShapeSyntaxKind) -> bool {
        if self.at(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, k: ShapeSyntaxKind) {
        if !self.eat(k) {
            let at = self.current_start();
            self.error(None, format!("expected {}", describe(k)), at, at);
        }
    }

    fn error(&mut self, code: Option<&'static str>, message: String, start: usize, end: usize) {
        let end = end.max(start);
        let range = TextRange::new(
            TextSize::from(u32::try_from(start).unwrap_or(u32::MAX)),
            TextSize::from(u32::try_from(end).unwrap_or(u32::MAX)),
        );
        self.errors.push(ParseError {
            code,
            message,
            range,
        });
    }

    /// Wrap a single unexpected significant token in an `ERROR_NODE` and record
    /// a diagnostic. Guarantees forward progress; callers must ensure the
    /// cursor is not at EOF.
    fn error_bump_one(&mut self, message: &str) {
        let start = self.current_start();
        self.start(ERROR_NODE);
        self.bump();
        self.finish();
        let end = self.consumed_end;
        self.error(None, message.to_string(), start, end);
    }

    // --- grammar --------------------------------------------------------

    fn file(&mut self) {
        self.start(SHAPE_FILE);
        while let Some(k) = self.current() {
            match k {
                STRUCT_KW => self.struct_def(),
                TRAIT_KW => self.trait_def(),
                IMPL_KW => self.impl_def(),
                TYPE_KW => self.type_alias(),
                USE_KW => self.use_decl(),
                _ => self.error_item(),
            }
        }
        // Flush trailing trivia so the tree tiles the input to EOF.
        self.eat_trivia();
        self.finish();
    }

    /// Top-level junk: consume up to the next item keyword as one error span.
    fn error_item(&mut self) {
        let start = self.current_start();
        self.start(ERROR_NODE);
        while let Some(k) = self.current() {
            if is_item_start(k) {
                break;
            }
            self.bump();
        }
        self.finish();
        let end = self.consumed_end;
        self.error(None, "unexpected token".to_string(), start, end);
    }

    fn struct_def(&mut self) {
        self.start(STRUCT_DEF);
        self.bump(); // `struct`
        self.expect(IDENT);
        if self.at(L_ANGLE) {
            self.generic_params();
        }
        if self.at(L_BRACE) {
            self.bump();
            let mut seen_open = false;
            loop {
                match self.current() {
                    None | Some(R_BRACE) => break,
                    Some(DOT_DOT) => {
                        if seen_open {
                            let at = self.current_start();
                            self.error(None, "duplicate `..` open marker".to_string(), at, at);
                        }
                        self.open_marker();
                        seen_open = true;
                    }
                    Some(IDENT) => {
                        if seen_open {
                            let at = self.current_start();
                            self.error(
                                None,
                                "`..` must be the last item in a struct body".to_string(),
                                at,
                                at,
                            );
                        }
                        self.field();
                    }
                    Some(_) => self.error_bump_one("expected a field or `}`"),
                }
            }
            self.expect(R_BRACE);
        } else {
            self.expect(L_BRACE);
        }
        self.finish();
    }

    fn open_marker(&mut self) {
        self.start(OPEN_MARKER);
        self.bump(); // `..`
        self.finish();
    }

    fn field(&mut self) {
        self.start(FIELD);
        self.bump(); // IDENT (leading doc comments nest inside FIELD)
        self.expect(COLON);
        self.type_expr();
        self.eat(COMMA); // trailing comma optional
        self.finish();
    }

    fn trait_def(&mut self) {
        self.start(TRAIT_DEF);
        self.bump(); // `trait`
        self.expect(IDENT);
        if self.at(L_ANGLE) {
            self.generic_params();
        }
        if self.at(COLON) {
            self.supertraits();
        }
        if self.at(L_BRACE) {
            self.bump();
            loop {
                match self.current() {
                    None | Some(R_BRACE) => break,
                    Some(FN_KW) => self.trait_fn(),
                    Some(_) => self.error_bump_one("expected `fn` or `}`"),
                }
            }
            self.expect(R_BRACE);
        } else {
            self.expect(L_BRACE);
        }
        self.finish();
    }

    fn supertraits(&mut self) {
        self.start(SUPERTRAITS);
        self.bump(); // `:`
        self.expect(IDENT);
        while self.eat(PLUS) {
            self.expect(IDENT);
        }
        self.finish();
    }

    fn trait_fn(&mut self) {
        self.start(TRAIT_FN);
        self.bump(); // `fn` (leading doc comments nest inside TRAIT_FN)
        self.expect(IDENT);
        if self.at(L_PAREN) {
            self.param_list();
        } else {
            self.expect(L_PAREN);
        }
        if self.at(ARROW) {
            self.ret();
        }
        if self.at(L_BRACE) {
            // A body where a `;` is expected: reject with LB2010, skip it.
            let start = self.current_start();
            self.skip_balanced_braces();
            let end = self.consumed_end;
            self.error(Some("LB2010"), LB2010_MESSAGE.to_string(), start, end);
            self.eat(SEMICOLON); // tolerate a stray `;` after the body
        } else {
            self.expect(SEMICOLON);
        }
        self.finish();
    }

    /// Consume a balanced `{ ... }` region into an `ERROR_NODE`. Cursor must be
    /// at the opening `{`.
    fn skip_balanced_braces(&mut self) {
        self.start(ERROR_NODE);
        let mut depth = 0usize;
        loop {
            match self.current() {
                None => break,
                Some(L_BRACE) => {
                    depth += 1;
                    self.bump();
                }
                Some(R_BRACE) => {
                    depth -= 1;
                    self.bump();
                    if depth == 0 {
                        break;
                    }
                }
                Some(_) => self.bump(),
            }
        }
        self.finish();
    }

    fn param_list(&mut self) {
        self.start(PARAM_LIST);
        self.bump(); // `(`
        loop {
            match self.current() {
                None | Some(R_PAREN) => break,
                Some(SELF_KW | IDENT) => {
                    self.param();
                    if !self.eat(COMMA) {
                        break;
                    }
                }
                Some(_) => self.error_bump_one("expected a parameter or `)`"),
            }
        }
        self.expect(R_PAREN);
        self.finish();
    }

    fn param(&mut self) {
        self.start(PARAM);
        if self.at(SELF_KW) {
            self.bump();
        } else {
            self.bump(); // IDENT
            self.expect(COLON);
            self.type_expr();
        }
        self.finish();
    }

    fn ret(&mut self) {
        self.start(RET_TYPE);
        self.bump(); // `->`
        self.type_expr();
        while self.eat(COMMA) {
            let before = self.pos;
            self.type_expr();
            if self.pos == before {
                break;
            }
        }
        self.finish();
    }

    fn impl_def(&mut self) {
        self.start(IMPL_DEF);
        self.bump(); // `impl`
        self.expect(IDENT); // trait name
        if self.at(L_ANGLE) {
            self.generic_params();
        }
        // SHAPES.md §12.3: `impl A + B for C;` sugar is rejected (not in the
        // grammar). Recover by consuming the extra `+ Trait` clauses.
        if self.at(PLUS) {
            let start = self.current_start();
            while self.eat(PLUS) {
                self.expect(IDENT);
                if self.at(L_ANGLE) {
                    self.generic_params();
                }
            }
            let end = self.consumed_end;
            self.error(
                None,
                "multiple traits in one `impl` are not supported; write a separate \
                 `impl Trait for Struct;` line for each"
                    .to_string(),
                start,
                end,
            );
        }
        self.expect(FOR_KW);
        self.expect(IDENT); // struct name
        self.expect(SEMICOLON);
        self.finish();
    }

    fn type_alias(&mut self) {
        self.start(TYPE_ALIAS);
        self.bump(); // `type`
        self.expect(IDENT);
        if self.at(L_ANGLE) {
            self.generic_params();
        }
        self.expect(EQ);
        self.type_expr();
        self.expect(SEMICOLON);
        self.finish();
    }

    fn use_decl(&mut self) {
        self.start(USE_DECL);
        self.bump(); // `use`
        if self.at(IDENT) {
            self.bump();
            while self.at(DOT) {
                self.bump();
                self.expect(IDENT);
            }
        } else {
            self.expect(IDENT);
        }
        self.expect(SEMICOLON);
        self.finish();
    }

    // --- generics -------------------------------------------------------

    fn generic_params(&mut self) {
        self.start(GENERIC_PARAMS);
        self.bump(); // `<`
        loop {
            match self.current() {
                None | Some(R_ANGLE) => break,
                Some(IDENT) => {
                    self.generic_param();
                    if !self.eat(COMMA) {
                        break;
                    }
                }
                Some(_) => self.error_bump_one("expected a generic parameter or `>`"),
            }
        }
        self.expect(R_ANGLE);
        self.finish();
    }

    fn generic_param(&mut self) {
        self.start(GENERIC_PARAM);
        self.bump(); // IDENT
        if self.eat(COLON) {
            self.expect(IDENT);
            while self.eat(PLUS) {
                self.expect(IDENT);
            }
        }
        self.finish();
    }

    fn generic_args(&mut self) {
        self.start(GENERIC_ARGS);
        self.bump(); // `<`
        self.type_list(R_ANGLE);
        self.expect(R_ANGLE);
        self.finish();
    }

    /// A comma-separated type list ending before `end`. Always terminates:
    /// each iteration consumes at least one token or breaks.
    fn type_list(&mut self, end: ShapeSyntaxKind) {
        if self.at(end) || self.current().is_none() {
            return;
        }
        loop {
            let before = self.pos;
            self.type_expr();
            if self.pos == before {
                if self.at(end) || self.current().is_none() {
                    break;
                }
                self.error_bump_one("expected a type");
                continue;
            }
            if self.eat(COMMA) {
                if self.at(end) || self.current().is_none() {
                    break; // trailing comma
                }
                continue;
            }
            break;
        }
    }

    // --- type expressions (precedence: `?` tightest, `|` loosest) --------

    fn type_expr(&mut self) {
        let cp = self.checkpoint();
        self.type_postfix();
        if self.at(PIPE) {
            self.start_at(cp, UNION_TYPE);
            while self.eat(PIPE) {
                self.type_postfix();
            }
            self.finish();
        }
    }

    fn type_postfix(&mut self) {
        let cp = self.checkpoint();
        self.type_primary();
        while self.at(QUESTION) {
            self.start_at(cp, OPTIONAL_TYPE);
            self.bump(); // `?`
            self.finish();
        }
    }

    fn type_primary(&mut self) {
        match self.current() {
            Some(IDENT) => {
                self.start(TYPE_REF);
                self.bump();
                if self.at(L_ANGLE) {
                    self.generic_args();
                }
                self.finish();
            }
            Some(FN_KW) => {
                self.start(FN_TYPE);
                self.bump(); // `fn`
                if self.at(L_PAREN) {
                    self.param_list();
                } else {
                    self.expect(L_PAREN);
                }
                if self.at(ARROW) {
                    self.ret();
                }
                self.finish();
            }
            Some(L_PAREN) => {
                self.start(PAREN_TYPE);
                self.bump(); // `(`
                self.type_expr();
                self.expect(R_PAREN);
                self.finish();
            }
            _ => {
                let at = self.current_start();
                self.error(None, "expected a type".to_string(), at, at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::ast::AstNode;

    fn parse_ok(src: &str) -> ShapeParse {
        let p = parse(src);
        assert!(
            p.errors().is_empty(),
            "unexpected errors for {src:?}: {:?}",
            p.errors()
        );
        p
    }

    #[test]
    fn lossless_on_spec_example() {
        let src = include_str!("test_data/spec_example.lb");
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src, "tree must tile the input");
        assert!(p.errors().is_empty(), "errors: {:?}", p.errors());
    }

    #[test]
    fn all_constructs_parse_clean() {
        parse_ok("struct P { x: number, y: number, label: string? }");
        parse_ok("struct Bag { n: number, .. }");
        parse_ok("struct Empty {}");
        parse_ok("struct Pair<T> { first: T, second: T }");
        parse_ok("struct Kv<K: Hash + Eq, V> { m: HashMap<K, V> }");
        parse_ok("trait Shape { fn area(self) -> number; }");
        parse_ok("trait Drawable: Shape + Sized { fn draw(self, s: Surface); }");
        parse_ok("trait Multi { fn split(self) -> A, B; }");
        parse_ok("impl Shape for Circle;");
        parse_ok("type Points = Vec<Point>;");
        parse_ok("type Handler = fn(a: A, b: B) -> R;");
        parse_ok("type MaybeNum = number?;");
        parse_ok("type Either = A | B | C;");
        parse_ok("type Nested = Vec<HashMap<string, Point?>>;");
        parse_ok("use geometry;");
        parse_ok("use pkg.geometry.core;");
    }

    #[test]
    fn lb2010_body_rejected_with_recovery() {
        let src = "trait T { fn a(self) { return 1; } fn b(self) -> number; }";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        let lb = p
            .errors()
            .iter()
            .find(|e| e.code == Some("LB2010"))
            .expect("LB2010 expected");
        assert_eq!(lb.message, LB2010_MESSAGE);
        // Recovery: `fn b` after the body still parses; the only error is LB2010.
        assert_eq!(p.errors().len(), 1, "errors: {:?}", p.errors());
        let root = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let t = root.items().next().unwrap();
        let crate::shape::ast::Item::Trait(t) = t else {
            panic!("expected trait")
        };
        let fns: Vec<_> = t.fns().collect();
        assert_eq!(fns.len(), 2);
        assert_eq!(fns[1].name().as_deref(), Some("b"));
    }

    #[test]
    fn nested_body_braces_are_balanced() {
        let src = "trait T { fn a(self) { if x { y } else { z } } fn b(self); }";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        assert_eq!(
            p.errors()
                .iter()
                .filter(|e| e.code == Some("LB2010"))
                .count(),
            1
        );
    }

    #[test]
    fn garbage_between_items_recovers() {
        let src = "struct A { x: number } @@@ !! 42 struct B { y: number }";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        assert!(!p.errors().is_empty());
        let root = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let names: Vec<_> = root
            .items()
            .filter_map(|i| match i {
                crate::shape::ast::Item::Struct(s) => s.name(),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn impl_multi_trait_sugar_rejected() {
        let src = "impl Shape + Drawable for Circle;";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        let e = &p.errors()[0];
        assert!(e.code.is_none());
        assert!(e.message.contains("separate"), "message: {}", e.message);
    }

    #[test]
    fn never_panics_on_truncated_input() {
        for src in [
            "struct",
            "struct A {",
            "trait T { fn",
            "type X =",
            "use",
            "impl A for",
            "struct A { x: }",
            "< > | ? -> ,",
            "fn(",
        ] {
            let p = parse(src);
            assert_eq!(p.syntax().to_string(), src, "lossless for {src:?}");
        }
    }

    #[test]
    fn spec_example_ast_assertions() {
        use crate::shape::ast::{Item, TypeRef};
        let src = include_str!("test_data/spec_example.lb");
        let p = parse(src);
        assert!(p.errors().is_empty(), "{:?}", p.errors());
        let file = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let items: Vec<Item> = file.items().collect();
        assert_eq!(items.len(), 6);

        // struct Point { x, y, label: string? }
        let Item::Struct(point) = &items[0] else {
            panic!()
        };
        assert_eq!(point.name().as_deref(), Some("Point"));
        let fields: Vec<_> = point.fields().collect();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[2].name().as_deref(), Some("label"));
        assert!(fields[2].optional(), "label is string?");
        assert!(!fields[0].optional());
        assert!(!point.is_open());

        // trait Shape { fn area(self) -> number; fn perimeter(self) -> number; }
        let Item::Trait(shape) = &items[1] else {
            panic!()
        };
        assert_eq!(shape.name().as_deref(), Some("Shape"));
        assert!(shape.supertraits().is_empty());
        let sfns: Vec<_> = shape.fns().collect();
        assert_eq!(sfns.len(), 2);
        assert_eq!(sfns[0].name().as_deref(), Some("area"));
        assert!(sfns[0].has_self());
        assert_eq!(sfns[0].returns().len(), 1);

        // trait Drawable: Shape { fn draw(self, surface: Surface); }
        let Item::Trait(drawable) = &items[2] else {
            panic!()
        };
        assert_eq!(drawable.supertraits(), vec!["Shape".to_string()]);
        let dfn = drawable.fns().next().unwrap();
        assert_eq!(dfn.name().as_deref(), Some("draw"));
        let params: Vec<_> = dfn.params().unwrap().params().collect();
        assert_eq!(params.len(), 2);
        assert!(params[0].is_self());
        assert_eq!(params[1].name().as_deref(), Some("surface"));
        assert!(dfn.returns().is_empty());

        // struct Circle; impl Shape for Circle;
        let Item::Struct(circle) = &items[3] else {
            panic!()
        };
        assert_eq!(circle.name().as_deref(), Some("Circle"));
        let Item::Impl(imp) = &items[4] else { panic!() };
        assert_eq!(imp.trait_name().as_deref(), Some("Shape"));
        assert_eq!(imp.struct_name().as_deref(), Some("Circle"));

        // struct Pair<T> { first: T, second: T }
        let Item::Struct(pair) = &items[5] else {
            panic!()
        };
        let gp: Vec<_> = pair.generic_params().unwrap().params().collect();
        assert_eq!(gp.len(), 1);
        assert_eq!(gp[0].name().as_deref(), Some("T"));
        let f0 = pair.fields().next().unwrap();
        assert!(matches!(f0.ty(), Some(TypeRef::Named(_))));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Fragments drawn from the `.lb` alphabet — joining these stresses the
    /// lexer boundaries and every parser recovery path.
    fn shape_token_soup() -> impl Strategy<Value = String> {
        let frag = prop::sample::select(vec![
            "struct", "trait", "impl", "for", "fn", "self", "type", "use", "Foo", "x", "number",
            "{", "}", "(", ")", "<", ">", ":", ";", ",", "?", "|", "+", "=", "->", "..", ".", " ",
            "\n", "// c\n", "/// d\n", "/* b */", "@", "5",
        ]);
        prop::collection::vec(frag, 0..40).prop_map(|v| v.join(""))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// Parsing any input never panics and always tiles it byte-for-byte.
        #[test]
        fn parse_never_panics_and_is_lossless(s in ".*") {
            let p = parse(&s);
            prop_assert_eq!(p.syntax().to_string(), s);
        }

        #[test]
        fn parse_lossless_on_shape_soup(s in shape_token_soup()) {
            let p = parse(&s);
            prop_assert_eq!(p.syntax().to_string(), s);
        }
    }
}
