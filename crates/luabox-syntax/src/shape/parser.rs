//! Recursive-descent, error-resilient parser for the `.luab` shape grammar
//! (SHAPES-V2.md), producing a lossless rowan green tree.
//!
//! Design notes:
//! - **Lossless.** Every lexed token — trivia included — is emitted into the
//!   tree exactly once, so `parse(text).syntax().to_string() == text` for any
//!   input. This is asserted by the property test at the bottom of the file.
//! - **Leading trivia nests inward.** Item/member nodes are opened *before*
//!   their first token is bumped, so preceding doc comments and comments land
//!   inside the node they document (the formatter relies on this).
//! - **Never panics.** Unexpected tokens become [`ShapeSyntaxKind::ERROR_NODE`]
//!   spans and parsing resynchronises at the next item keyword. Bodies (a `{`
//!   after a method signature) are rejected with `LB2010` and skipped over
//!   balanced braces.
//! - **No terminators.** Items are self-delimiting: an item ends where its
//!   type expression ends, and the next `export`/`type` keyword (or EOF)
//!   starts the next one.

use rowan::{GreenNode, GreenNodeBuilder, TextRange, TextSize};

use super::{ShapeLanguage, ShapeSyntaxKind, ShapeSyntaxNode, lex};
use ShapeSyntaxKind::{
    AMP, COLON, COMMA, DOT, EQ, ERROR_NODE, EXPORT_KW, FAT_ARROW, FIELD, FN_TYPE, GENERIC_ARGS,
    GENERIC_PARAM, GENERIC_PARAMS, IDENT, INTERSECTION_TYPE, L_ANGLE, L_BRACE, L_PAREN, METHOD,
    OBJECT_TYPE, OPTIONAL_TYPE, PARAM, PARAM_LIST, PAREN_TYPE, PIPE, QUESTION, R_ANGLE, R_BRACE,
    R_PAREN, SELF_KW, SHAPE_FILE, TYPE_DEF, TYPE_KW, TYPE_REF, UNION_TYPE,
};

/// The exact `LB2010` message (SHAPES-V2.md: `.luab` stays analyser-only).
pub const LB2010_MESSAGE: &str = "implementations live in .lua - .luab declares types only";

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

/// Parse `.luab` source into a lossless green tree. Never fails, never panics:
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
    matches!(k, TYPE_KW | EXPORT_KW)
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
        EQ => "`=`",
        FAT_ARROW => "`=>`",
        TYPE_KW => "`type`",
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
            if is_item_start(k) {
                self.type_def();
            } else {
                self.error_item();
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

    /// `export? type Name<T, ...> = type_expr` — the only item form.
    fn type_def(&mut self) {
        self.start(TYPE_DEF);
        self.eat(EXPORT_KW);
        self.expect(TYPE_KW);
        self.expect(IDENT);
        if self.at(L_ANGLE) {
            self.generic_params();
        }
        self.expect(EQ);
        self.type_expr();
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
        self.bump(); // IDENT (v2 generics carry no bounds)
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

    // --- type expressions -------------------------------------------------
    // Precedence, tightest first: `?` postfix, `&`, `|`.

    fn type_expr(&mut self) {
        let cp = self.checkpoint();
        self.type_intersection();
        if self.at(PIPE) {
            self.start_at(cp, UNION_TYPE);
            while self.eat(PIPE) {
                self.type_intersection();
            }
            self.finish();
        }
    }

    fn type_intersection(&mut self) {
        let cp = self.checkpoint();
        self.type_postfix();
        if self.at(AMP) {
            self.start_at(cp, INTERSECTION_TYPE);
            while self.eat(AMP) {
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
                while self.at(DOT) {
                    self.bump();
                    self.expect(IDENT);
                }
                if self.at(L_ANGLE) {
                    self.generic_args();
                }
                self.finish();
            }
            Some(L_BRACE) => self.object_type(),
            Some(L_PAREN) => self.paren_or_fn_type(),
            _ => {
                let at = self.current_start();
                self.error(None, "expected a type".to_string(), at, at);
            }
        }
    }

    /// Disambiguate `(a: A) => R` (function type) from `(T)` / `(A, B)`
    /// (parenthesised group / multi-return list) by looking past the `(`:
    /// `)` or `self` or `name [?] :` means function-type parameters.
    fn paren_or_fn_type(&mut self) {
        let is_fn = match self.nth(1) {
            Some(R_PAREN | SELF_KW) => true,
            Some(IDENT) => {
                matches!(self.nth(2), Some(COLON))
                    || (self.nth(2) == Some(QUESTION) && self.nth(3) == Some(COLON))
            }
            _ => false,
        };
        if is_fn {
            self.start(FN_TYPE);
            self.param_list();
            self.expect(FAT_ARROW);
            self.type_expr();
            self.finish();
            return;
        }
        let cp = self.checkpoint();
        self.start(PAREN_TYPE);
        self.bump(); // `(`
        self.type_list(R_PAREN);
        self.expect(R_PAREN);
        self.finish();
        // `(A) => R`: function-type params must be named. Recover by wrapping
        // the group as the FN_TYPE's parameter position and say so.
        if self.at(FAT_ARROW) {
            let start = self.current_start();
            self.start_at(cp, FN_TYPE);
            self.bump(); // `=>`
            self.type_expr();
            self.finish();
            let end = self.consumed_end;
            self.error(
                None,
                "function-type parameters must be named: `(x: T) => R`".to_string(),
                start,
                end,
            );
        }
    }

    // --- object types & members -------------------------------------------

    fn object_type(&mut self) {
        self.start(OBJECT_TYPE);
        self.bump(); // `{`
        loop {
            match self.current() {
                None | Some(R_BRACE) => break,
                Some(IDENT) => self.member(),
                Some(_) => self.error_bump_one("expected a member or `}`"),
            }
        }
        self.expect(R_BRACE);
        self.finish();
    }

    /// One object member: `name?: type` (field) or
    /// `name(params...) (":" ret)?` (method).
    fn member(&mut self) {
        if self.nth(1) == Some(L_PAREN) {
            self.method();
        } else {
            self.field();
        }
    }

    fn field(&mut self) {
        self.start(FIELD);
        self.bump(); // IDENT (leading doc comments nest inside FIELD)
        self.eat(QUESTION);
        self.expect(COLON);
        self.type_expr();
        self.eat(COMMA); // trailing comma optional
        self.finish();
    }

    fn method(&mut self) {
        self.start(METHOD);
        self.bump(); // IDENT (leading doc comments nest inside METHOD)
        self.param_list();
        if self.eat(COLON) {
            self.type_expr();
        }
        if self.at(L_BRACE) {
            // A body where a member separator is expected: reject with
            // LB2010, skip it balanced.
            let start = self.current_start();
            self.skip_balanced_braces();
            let end = self.consumed_end;
            self.error(Some("LB2010"), LB2010_MESSAGE.to_string(), start, end);
        }
        self.eat(COMMA);
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
        self.expect(L_PAREN);
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
            self.eat(QUESTION);
            self.expect(COLON);
            self.type_expr();
        }
        self.finish();
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
        let src = include_str!("test_data/spec_example.luab");
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src, "tree must tile the input");
        assert!(p.errors().is_empty(), "errors: {:?}", p.errors());
    }

    #[test]
    fn all_constructs_parse_clean() {
        parse_ok("type P = { x: number, y: number, label?: string }");
        parse_ok("type Empty = {}");
        parse_ok("type Pair<T> = { first: T, second: T }");
        parse_ok("type Kv<K, V> = { m: Map<K, V> }");
        parse_ok("export type Shape = { area(self): number }");
        parse_ok("type D = Shape & { draw(self, s: Surface) }");
        parse_ok("type Multi = { split(self): (A, B) }");
        parse_ok("type Alias = geometry.Point");
        parse_ok("export type Canvas = love.graphics.Canvas");
        parse_ok("type Handler = (a: A, b: B) => R");
        parse_ok("type Thunk = () => number");
        parse_ok("type MaybeNum = number?");
        parse_ok("type Either = A | B | C");
        parse_ok("type Both = A & B & C");
        parse_ok("type Mixed = A & B | C?");
        parse_ok("type Nested = Vec<Map<string, Point?>>");
        parse_ok("type Opt = { f(x?: number): string }");
    }

    #[test]
    fn items_are_self_delimiting() {
        let src = "type A = number\ntype B = { x: A }\nexport type C = A | B";
        let p = parse_ok(src);
        let file = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let items: Vec<_> = file.items().collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name().as_deref(), Some("A"));
        assert!(!items[0].is_export());
        assert!(items[2].is_export());
    }

    #[test]
    fn lb2010_body_rejected_with_recovery() {
        let src = "type T = { a(self) { return 1 }, b(self): number }";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        let lb = p
            .errors()
            .iter()
            .find(|e| e.code == Some("LB2010"))
            .expect("LB2010 expected");
        assert_eq!(lb.message, LB2010_MESSAGE);
        // Recovery: member `b` after the body still parses; the only error
        // is LB2010.
        assert_eq!(p.errors().len(), 1, "errors: {:?}", p.errors());
        let root = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let t = root.items().next().unwrap();
        let Some(crate::shape::ast::TypeRef::Object(obj)) = t.ty() else {
            panic!("expected object type")
        };
        let members: Vec<_> = obj.members().collect();
        assert_eq!(members.len(), 2);
        let crate::shape::ast::Member::Method(b) = &members[1] else {
            panic!("expected method")
        };
        assert_eq!(b.name().as_deref(), Some("b"));
    }

    #[test]
    fn nested_body_braces_are_balanced() {
        let src = "type T = { a(self) { if x { y } else { z } }, b(self): number }";
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
        let src = "type A = { x: number } @@@ !! 42 type B = { y: number }";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        assert!(!p.errors().is_empty());
        let root = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let names: Vec<_> = root.items().filter_map(|i| i.name()).collect();
        assert_eq!(names, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn unnamed_fn_params_recover_with_hint() {
        let src = "type F = (A) => B";
        let p = parse(src);
        assert_eq!(p.syntax().to_string(), src);
        let e = &p.errors()[0];
        assert!(e.message.contains("named"), "message: {}", e.message);
    }

    #[test]
    fn never_panics_on_truncated_input() {
        for src in [
            "type",
            "export",
            "export type",
            "type A =",
            "type A = {",
            "type A = { x:",
            "type A = { f(",
            "type A = ( ",
            "type A = B &",
            "type A = B |",
            "< > | ? & => ,",
            "( ) =>",
        ] {
            let p = parse(src);
            assert_eq!(p.syntax().to_string(), src, "lossless for {src:?}");
        }
    }

    #[test]
    fn spec_example_ast_assertions() {
        use crate::shape::ast::{Member, TypeRef};
        let src = include_str!("test_data/spec_example.luab");
        let p = parse(src);
        assert!(p.errors().is_empty(), "{:?}", p.errors());
        let file = crate::shape::ast::ShapeFile::cast(p.syntax()).unwrap();
        let items: Vec<_> = file.items().collect();
        assert_eq!(items.len(), 6);

        // type Point = { x, y, label?: string }
        let point = &items[0];
        assert_eq!(point.name().as_deref(), Some("Point"));
        assert!(!point.is_export());
        let Some(TypeRef::Object(obj)) = point.ty() else {
            panic!("Point is an object type")
        };
        let members: Vec<_> = obj.members().collect();
        assert_eq!(members.len(), 3);
        let Member::Field(label) = &members[2] else {
            panic!()
        };
        assert_eq!(label.name().as_deref(), Some("label"));
        assert!(label.optional());
        let Member::Field(x) = &members[0] else {
            panic!()
        };
        assert!(!x.optional());

        // type Pair<T> = { first: T, second: T }
        let pair = &items[2];
        let gp: Vec<_> = pair.generic_params().unwrap().params().collect();
        assert_eq!(gp.len(), 1);
        assert_eq!(gp[0].name().as_deref(), Some("T"));

        // export type Shape = { area(self): number, perimeter(self): number }
        let shape = &items[3];
        assert!(shape.is_export());
        let Some(TypeRef::Object(shape_obj)) = shape.ty() else {
            panic!()
        };
        let smembers: Vec<_> = shape_obj.members().collect();
        assert_eq!(smembers.len(), 2);
        let Member::Method(area) = &smembers[0] else {
            panic!()
        };
        assert_eq!(area.name().as_deref(), Some("area"));
        assert!(area.has_self());
        assert!(area.ret().is_some());

        // export type Drawable = Shape & { draw(self, surface: Surface) }
        let drawable = &items[4];
        let Some(TypeRef::Intersection(both)) = drawable.ty() else {
            panic!("Drawable is an intersection")
        };
        let parts: Vec<_> = both.members().collect();
        assert_eq!(parts.len(), 2);
        let TypeRef::Named(base) = &parts[0] else {
            panic!()
        };
        assert_eq!(base.path(), "Shape");
        let TypeRef::Object(ext) = &parts[1] else {
            panic!()
        };
        let Member::Method(draw) = ext.members().next().unwrap() else {
            panic!()
        };
        assert_eq!(draw.name().as_deref(), Some("draw"));
        let params: Vec<_> = draw.params().unwrap().params().collect();
        assert_eq!(params.len(), 2);
        assert!(params[0].is_self());
        assert_eq!(params[1].name().as_deref(), Some("surface"));
        assert!(draw.ret().is_none());

        // export type Canvas = love.graphics.Canvas (re-export of a dep type)
        let canvas = &items[5];
        assert!(canvas.is_export());
        let Some(TypeRef::Named(target)) = canvas.ty() else {
            panic!()
        };
        assert_eq!(target.path(), "love.graphics.Canvas");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Fragments drawn from the `.luab` alphabet — joining these stresses the
    /// lexer boundaries and every parser recovery path. Retired v1 tokens
    /// (`->`, `..`, `struct`, …) stay in the soup as junk.
    fn shape_token_soup() -> impl Strategy<Value = String> {
        let frag = prop::sample::select(vec![
            "type",
            "export",
            "self",
            "Foo",
            "x",
            "number",
            "{",
            "}",
            "(",
            ")",
            "<",
            ">",
            ":",
            ",",
            "?",
            "|",
            "&",
            "=",
            "=>",
            ".",
            " ",
            "\n",
            "-- c\n",
            "--- d\n",
            "--[[ b ]]",
            "struct",
            "impl",
            "->",
            "..",
            ";",
            "@",
            "5",
            "-",
            "--[=[",
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
