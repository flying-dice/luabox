//! The LuaCATS type-expression grammar — the heart of the annotation dialect.
//!
//! Types are rich: dotted names, optionals (`T?`), unions (`A|B`), arrays
//! (`T[]`), tuples (`[T, U]`), table/dictionary literals (`{ f: T, [K]: V }`),
//! generic application (`table<K, V>`, `Name<T>`), function types
//! (`fun(a: T, ...: V): R1, R2`), literal types (`"s"`, `1`, `true`,
//! `` `T` ``), and parentheses. Postfix (`?`, `[]`) binds tighter than `|`.
//!
//! The parser is error-tolerant: malformed syntax yields a
//! [`TypeExprKind::Error`] node and a recorded [`LuaCatsError`]; it never
//! panics and never aborts the surrounding block.

use super::{LuaCatsError, Span};

/// A parsed type expression with a file-absolute byte [`Span`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeExpr {
    pub kind: TypeExprKind,
    pub span: Span,
}

/// The shape of a [`TypeExpr`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExprKind {
    /// A (possibly dotted) name with optional generic arguments: `a.b.C`,
    /// `Name<T, U>`, `table<K, V>`.
    Named { name: String, args: Vec<TypeExpr> },
    /// `T?` — an optional/nilable type.
    Optional(Box<TypeExpr>),
    /// `T[]` — an array type.
    Array(Box<TypeExpr>),
    /// `A | B | ...` — a union.
    Union(Vec<TypeExpr>),
    /// `[T, U, ...]` — a tuple.
    Tuple(Vec<TypeExpr>),
    /// `{ field: T, [K]: V, ... }` — a table literal / dictionary type.
    Table(Vec<TableField>),
    /// `fun(a: T, ...: V): R1, R2` — a function type.
    Fun {
        params: Vec<FunParam>,
        returns: Vec<FunReturn>,
    },
    /// A string literal type: `"lit"` or `'lit'`. Stores the raw text
    /// including quotes.
    StringLit(String),
    /// A number literal type: `123`, `0x1F`, `-1`.
    NumberLit(String),
    /// A boolean literal type: `true` / `false`.
    BoolLit(bool),
    /// A backtick-quoted generic capture: `` `T` ``. Stores the inner text.
    Backtick(String),
    /// `( T )` — a parenthesised type.
    Paren(Box<TypeExpr>),
    /// A malformed type; a [`LuaCatsError`] was recorded for it.
    Error,
}

/// One field of a table literal type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableField {
    /// `name: T` or `name?: T`.
    Named {
        name: String,
        optional: bool,
        ty: TypeExpr,
    },
    /// `[K]: V` — an indexer whose key is itself a type.
    Indexer { key: TypeExpr, value: TypeExpr },
}

/// One parameter of a `fun(...)` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunParam {
    /// The parameter name, or `...` for the variadic parameter.
    pub name: String,
    pub optional: bool,
    pub vararg: bool,
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

/// One return value of a `fun(...): ...` type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunReturn {
    pub name: Option<String>,
    pub ty: TypeExpr,
    pub vararg: bool,
}

// === Tokeniser ===

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(String),
    Backtick(String),
    Question,
    Pipe,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Lt,
    Gt,
    Comma,
    Semicolon,
    Colon,
    Dot,
    Ellipsis,
    Plus,
    Minus,
    Hash,
    Other,
}

#[derive(Debug, Clone)]
struct SpanTok {
    tok: Tok,
    start: usize,
    end: usize,
}

/// Tokenise `text`, producing tokens whose spans are file-absolute (offset by
/// `base`). Whitespace is skipped; unrecognised bytes become [`Tok::Other`].
fn tokenize(text: &str, base: usize) -> Vec<SpanTok> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let tok = scan_token(text, bytes, &mut i);
        out.push(SpanTok {
            tok,
            start: base + start,
            end: base + i,
        });
    }
    out
}

/// Scan a single token starting at `*i`, advancing `*i` past it.
fn scan_token(text: &str, bytes: &[u8], i: &mut usize) -> Tok {
    let c = bytes[*i];
    match c {
        b'"' | b'\'' => scan_quoted(text, bytes, i, c),
        b'`' => scan_backtick(text, bytes, i),
        b'.' => scan_dots(bytes, i),
        b'?' => single(i, Tok::Question),
        b'|' => single(i, Tok::Pipe),
        b'[' => single(i, Tok::LBracket),
        b']' => single(i, Tok::RBracket),
        b'{' => single(i, Tok::LBrace),
        b'}' => single(i, Tok::RBrace),
        b'(' => single(i, Tok::LParen),
        b')' => single(i, Tok::RParen),
        b'<' => single(i, Tok::Lt),
        b'>' => single(i, Tok::Gt),
        b',' => single(i, Tok::Comma),
        b';' => single(i, Tok::Semicolon),
        b':' => single(i, Tok::Colon),
        b'+' => single(i, Tok::Plus),
        b'-' => single(i, Tok::Minus),
        b'#' => single(i, Tok::Hash),
        _ if c.is_ascii_digit() => scan_number(text, bytes, i),
        _ if c == b'_' || c.is_ascii_alphabetic() || c >= 0x80 => scan_ident(text, bytes, i),
        _ => single(i, Tok::Other),
    }
}

fn single(i: &mut usize, tok: Tok) -> Tok {
    *i += 1;
    tok
}

fn scan_dots(bytes: &[u8], i: &mut usize) -> Tok {
    if bytes[*i..].starts_with(b"...") {
        *i += 3;
        Tok::Ellipsis
    } else {
        *i += 1;
        Tok::Dot
    }
}

fn scan_quoted(text: &str, bytes: &[u8], i: &mut usize, quote: u8) -> Tok {
    let start = *i;
    *i += 1;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'\\' {
            *i = (*i + 2).min(bytes.len());
            continue;
        }
        *i += 1;
        if b == quote {
            break;
        }
    }
    Tok::Str(text[start..*i].to_string())
}

fn scan_backtick(text: &str, bytes: &[u8], i: &mut usize) -> Tok {
    let start = *i + 1;
    *i += 1;
    while *i < bytes.len() && bytes[*i] != b'`' {
        *i += 1;
    }
    let inner = text[start..*i].to_string();
    if *i < bytes.len() {
        *i += 1; // closing backtick
    }
    Tok::Backtick(inner)
}

fn scan_ident(text: &str, bytes: &[u8], i: &mut usize) -> Tok {
    let start = *i;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80 {
            *i += 1;
        } else {
            break;
        }
    }
    Tok::Ident(text[start..*i].to_string())
}

fn scan_number(text: &str, bytes: &[u8], i: &mut usize) -> Tok {
    let start = *i;
    while *i < bytes.len() {
        let b = bytes[*i];
        if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' {
            *i += 1;
        } else {
            break;
        }
    }
    Tok::Num(text[start..*i].to_string())
}

// === Parser ===

/// A cursor over tokenised type syntax, shared by the type grammar and by the
/// tag parsers (which drive it for `@generic`, `@cast`, return lists, etc.).
pub struct TypeParser {
    toks: Vec<SpanTok>,
    pos: usize,
    end: usize,
    errors: Vec<LuaCatsError>,
}

impl TypeParser {
    /// Build a parser over `text`, whose tokens carry file-absolute spans
    /// offset by `base`.
    pub fn new(text: &str, base: usize) -> Self {
        let toks = tokenize(text, base);
        TypeParser {
            toks,
            pos: 0,
            end: base + text.len(),
            errors: Vec::new(),
        }
    }

    /// The recorded errors, consumed.
    pub fn take_errors(&mut self) -> Vec<LuaCatsError> {
        std::mem::take(&mut self.errors)
    }

    /// Whether every token has been consumed.
    pub fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }

    /// The file-absolute offset of the next unconsumed token (or end of
    /// input). Tag parsers use this to slice trailing description text.
    pub fn cursor_offset(&self) -> usize {
        self.toks.get(self.pos).map_or(self.end, |t| t.start)
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn nth(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n).map(|t| &t.tok)
    }

    fn at(&self, tok: &Tok) -> bool {
        self.peek() == Some(tok)
    }

    fn bump(&mut self) -> Option<SpanTok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, tok: &Tok) -> bool {
        if self.at(tok) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Consume and return the next comma if present.
    pub fn eat_comma(&mut self) -> bool {
        self.eat(&Tok::Comma)
    }

    /// Consume a `(` if present.
    pub fn eat_lparen(&mut self) -> bool {
        self.eat(&Tok::LParen)
    }

    /// Consume a `)` if present.
    pub fn eat_rparen(&mut self) -> bool {
        self.eat(&Tok::RParen)
    }

    /// Consume a `:` if present.
    pub fn eat_colon(&mut self) -> bool {
        self.eat(&Tok::Colon)
    }

    /// Consume a `...` if present.
    pub fn eat_ellipsis(&mut self) -> bool {
        self.eat(&Tok::Ellipsis)
    }

    /// Consume a leading `+` or `-` sign (used by `@cast`), returning it.
    pub fn eat_sign(&mut self) -> Option<char> {
        if self.eat(&Tok::Plus) {
            Some('+')
        } else if self.eat(&Tok::Minus) {
            Some('-')
        } else {
            None
        }
    }

    /// Consume the next token if it is an identifier, returning its text and
    /// span.
    pub fn take_ident(&mut self) -> Option<(String, Span)> {
        if !matches!(self.peek(), Some(Tok::Ident(_))) {
            return None;
        }
        match self.bump() {
            Some(SpanTok {
                tok: Tok::Ident(name),
                start,
                end,
            }) => Some((name, Span::new(start, end))),
            _ => None,
        }
    }

    /// Consume a trailing return name: an identifier that is immediately
    /// followed by end-of-input, a comma, or a `#` description marker. This
    /// distinguishes `@return string name` from `@return string some prose`.
    pub fn take_return_name(&mut self) -> Option<String> {
        let is_name = matches!(self.peek(), Some(Tok::Ident(_)))
            && matches!(self.nth(1), None | Some(Tok::Comma | Tok::Hash));
        if !is_name {
            return None;
        }
        match self.bump() {
            Some(SpanTok {
                tok: Tok::Ident(n), ..
            }) => Some(n),
            _ => None,
        }
    }

    /// The span of the previously consumed token, or an empty span at input
    /// end when nothing has been consumed.
    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            self.toks.first().map_or(self.end, |t| t.start)
        } else {
            self.toks[self.pos - 1].end
        }
    }

    fn error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(LuaCatsError {
            message: message.into(),
            span,
        });
    }

    fn here(&self) -> usize {
        self.cursor_offset()
    }

    /// Parse a single (maximal) type expression.
    pub fn parse_type(&mut self) -> TypeExpr {
        self.parse_union()
    }

    fn parse_union(&mut self) -> TypeExpr {
        self.eat(&Tok::Pipe); // tolerate a leading `|`
        let first = self.parse_postfix();
        let mut members = vec![first];
        while self.eat(&Tok::Pipe) {
            members.push(self.parse_postfix());
        }
        if members.len() == 1 {
            members.pop().unwrap_or(TypeExpr {
                kind: TypeExprKind::Error,
                span: Span::empty(self.here()),
            })
        } else {
            let span = Span::new(members[0].span.start, members[members.len() - 1].span.end);
            TypeExpr {
                kind: TypeExprKind::Union(members),
                span,
            }
        }
    }

    fn parse_postfix(&mut self) -> TypeExpr {
        let mut ty = self.parse_primary();
        loop {
            if self.at(&Tok::Question) {
                let end = self.bump().map_or(ty.span.end, |t| t.end);
                let span = Span::new(ty.span.start, end);
                ty = TypeExpr {
                    kind: TypeExprKind::Optional(Box::new(ty)),
                    span,
                };
            } else if self.at(&Tok::LBracket) && self.nth(1) == Some(&Tok::RBracket) {
                self.bump();
                let end = self.bump().map_or(ty.span.end, |t| t.end);
                let span = Span::new(ty.span.start, end);
                ty = TypeExpr {
                    kind: TypeExprKind::Array(Box::new(ty)),
                    span,
                };
            } else {
                break;
            }
        }
        ty
    }

    fn parse_primary(&mut self) -> TypeExpr {
        let Some(tok) = self.peek().cloned() else {
            let at = self.here();
            self.error("expected a type", Span::empty(at));
            return TypeExpr {
                kind: TypeExprKind::Error,
                span: Span::empty(at),
            };
        };
        match tok {
            Tok::LParen => self.parse_paren(),
            Tok::LBrace => self.parse_table(),
            Tok::LBracket => self.parse_tuple(),
            Tok::Str(s) => self.leaf(TypeExprKind::StringLit(s)),
            Tok::Num(n) => self.leaf(TypeExprKind::NumberLit(n)),
            Tok::Backtick(s) => self.leaf(TypeExprKind::Backtick(s)),
            Tok::Minus => self.parse_negative_number(),
            Tok::Ident(name) => self.parse_named_or_fun(name),
            _ => {
                let bad = self.bump().expect("peek succeeded");
                let span = Span::new(bad.start, bad.end);
                self.error("unexpected token in type", span);
                TypeExpr {
                    kind: TypeExprKind::Error,
                    span,
                }
            }
        }
    }

    /// Consume one token and wrap its span around `kind`.
    fn leaf(&mut self, kind: TypeExprKind) -> TypeExpr {
        let t = self.bump().expect("leaf called with a token present");
        TypeExpr {
            kind,
            span: Span::new(t.start, t.end),
        }
    }

    fn parse_negative_number(&mut self) -> TypeExpr {
        let minus = self.bump().expect("minus present");
        if let Some(Tok::Num(n)) = self.peek().cloned() {
            let num = self.bump().expect("num present");
            return TypeExpr {
                kind: TypeExprKind::NumberLit(format!("-{n}")),
                span: Span::new(minus.start, num.end),
            };
        }
        let span = Span::new(minus.start, minus.end);
        self.error("unexpected '-' in type", span);
        TypeExpr {
            kind: TypeExprKind::Error,
            span,
        }
    }

    fn parse_paren(&mut self) -> TypeExpr {
        let open = self.bump().expect("'(' present");
        let inner = self.parse_type();
        let end = if self.at(&Tok::RParen) {
            self.bump().map_or(inner.span.end, |t| t.end)
        } else {
            self.error("expected ')'", Span::empty(self.here()));
            inner.span.end
        };
        TypeExpr {
            kind: TypeExprKind::Paren(Box::new(inner)),
            span: Span::new(open.start, end),
        }
    }

    fn parse_tuple(&mut self) -> TypeExpr {
        let open = self.bump().expect("'[' present");
        let mut items = Vec::new();
        while !self.at(&Tok::RBracket) && !self.at_end() {
            items.push(self.parse_type());
            if !self.eat_comma() {
                break;
            }
        }
        let end = if self.at(&Tok::RBracket) {
            self.bump().map_or(self.prev_end(), |t| t.end)
        } else {
            self.error("expected ']' to close tuple", Span::empty(self.here()));
            self.prev_end()
        };
        TypeExpr {
            kind: TypeExprKind::Tuple(items),
            span: Span::new(open.start, end),
        }
    }

    fn parse_table(&mut self) -> TypeExpr {
        let open = self.bump().expect("'{' present");
        let mut fields = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at_end() {
            if let Some(field) = self.parse_table_field() {
                fields.push(field);
            }
            if !self.eat_comma() && !self.eat(&Tok::Semicolon) {
                break;
            }
        }
        let end = if self.at(&Tok::RBrace) {
            self.bump().map_or(self.prev_end(), |t| t.end)
        } else {
            self.error("expected '}' to close table type", Span::empty(self.here()));
            self.prev_end()
        };
        TypeExpr {
            kind: TypeExprKind::Table(fields),
            span: Span::new(open.start, end),
        }
    }

    fn parse_table_field(&mut self) -> Option<TableField> {
        if self.at(&Tok::LBracket) {
            self.bump();
            let key = self.parse_type();
            if !self.eat(&Tok::RBracket) {
                self.error("expected ']' in table indexer", Span::empty(self.here()));
            }
            self.expect_colon("table indexer");
            let value = self.parse_type();
            return Some(TableField::Indexer { key, value });
        }
        let Some(Tok::Ident(name)) = self.peek().cloned() else {
            // Unrecognised field start: consume one token to make progress.
            if let Some(bad) = self.bump() {
                self.error(
                    "expected a field name in table type",
                    Span::new(bad.start, bad.end),
                );
            }
            return None;
        };
        self.bump();
        let optional = self.eat(&Tok::Question);
        self.expect_colon("table field");
        let ty = self.parse_type();
        Some(TableField::Named { name, optional, ty })
    }

    fn expect_colon(&mut self, ctx: &str) {
        if !self.eat(&Tok::Colon) {
            self.error(format!("expected ':' in {ctx}"), Span::empty(self.here()));
        }
    }

    fn parse_named_or_fun(&mut self, first: String) -> TypeExpr {
        if first == "fun" && self.nth(1) == Some(&Tok::LParen) {
            return self.parse_fun();
        }
        if first == "true" || first == "false" {
            return self.leaf(TypeExprKind::BoolLit(first == "true"));
        }
        self.parse_named(first)
    }

    fn parse_named(&mut self, first: String) -> TypeExpr {
        let head = self.bump().expect("ident present");
        let mut name = first;
        let mut end = head.end;
        // Dotted path: `a.b.C`.
        while self.at(&Tok::Dot) {
            if let Some(Tok::Ident(seg)) = self.nth(1).cloned() {
                self.bump(); // '.'
                let seg_tok = self.bump().expect("segment present");
                name.push('.');
                name.push_str(&seg);
                end = seg_tok.end;
            } else {
                break;
            }
        }
        // Generic application: `Name<T, U>`.
        let mut args = Vec::new();
        if self.at(&Tok::Lt) {
            self.bump();
            while !self.at(&Tok::Gt) && !self.at_end() {
                args.push(self.parse_type());
                if !self.eat_comma() {
                    break;
                }
            }
            if self.at(&Tok::Gt) {
                end = self.bump().map_or(end, |t| t.end);
            } else {
                self.error("expected '>' to close generics", Span::empty(self.here()));
                end = self.prev_end();
            }
        }
        TypeExpr {
            kind: TypeExprKind::Named { name, args },
            span: Span::new(head.start, end),
        }
    }

    fn parse_fun(&mut self) -> TypeExpr {
        let fun = self.bump().expect("'fun' present");
        self.bump(); // '('
        let mut params = Vec::new();
        while !self.at(&Tok::RParen) && !self.at_end() {
            params.push(self.parse_fun_param());
            if !self.eat_comma() {
                break;
            }
        }
        let mut end = if self.at(&Tok::RParen) {
            self.bump().map_or(self.prev_end(), |t| t.end)
        } else {
            self.error("expected ')' in fun type", Span::empty(self.here()));
            self.prev_end()
        };
        let mut returns = Vec::new();
        if self.eat(&Tok::Colon) {
            returns = self.parse_fun_returns();
            if let Some(last) = returns.last() {
                end = last.ty.span.end;
            }
        }
        TypeExpr {
            kind: TypeExprKind::Fun { params, returns },
            span: Span::new(fun.start, end),
        }
    }

    fn parse_fun_param(&mut self) -> FunParam {
        let start = self.here();
        if self.at(&Tok::Ellipsis) {
            let dots = self.bump().expect("'...' present");
            let mut end = dots.end;
            let mut ty = None;
            if self.eat(&Tok::Colon) {
                let t = self.parse_type();
                end = t.span.end;
                ty = Some(t);
            }
            return FunParam {
                name: "...".to_string(),
                optional: false,
                vararg: true,
                ty,
                span: Span::new(dots.start, end),
            };
        }
        let Some(Tok::Ident(name)) = self.peek().cloned() else {
            // Unnamed parameter: parse it as a bare type.
            let t = self.parse_type();
            let span = t.span;
            return FunParam {
                name: String::new(),
                optional: false,
                vararg: false,
                ty: Some(t),
                span,
            };
        };
        self.bump();
        let optional = self.eat(&Tok::Question);
        let mut end = self.prev_end();
        let mut ty = None;
        if self.eat(&Tok::Colon) {
            let t = self.parse_type();
            end = t.span.end;
            ty = Some(t);
        }
        FunParam {
            name,
            optional,
            vararg: false,
            ty,
            span: Span::new(start, end),
        }
    }

    fn parse_fun_returns(&mut self) -> Vec<FunReturn> {
        // Optional parenthesised return list: `fun(): (a, b)`.
        let paren = self.eat(&Tok::LParen);
        let mut returns = Vec::new();
        loop {
            if self.at_end() || self.at(&Tok::RParen) {
                break;
            }
            returns.push(self.parse_fun_return());
            if !self.eat_comma() {
                break;
            }
        }
        if paren && !self.eat(&Tok::RParen) {
            self.error("expected ')' in return list", Span::empty(self.here()));
        }
        returns
    }

    fn parse_fun_return(&mut self) -> FunReturn {
        if self.at(&Tok::Ellipsis) {
            let dots = self.bump().expect("'...' present");
            return FunReturn {
                name: None,
                ty: TypeExpr {
                    kind: TypeExprKind::Named {
                        name: "any".to_string(),
                        args: Vec::new(),
                    },
                    span: Span::new(dots.start, dots.end),
                },
                vararg: true,
            };
        }
        // Named return: `name: Type`.
        if let (Some(Tok::Ident(n)), Some(Tok::Colon)) = (self.peek().cloned(), self.nth(1)) {
            self.bump(); // name
            self.bump(); // ':'
            let ty = self.parse_type();
            return FunReturn {
                name: Some(n),
                ty,
                vararg: false,
            };
        }
        FunReturn {
            name: None,
            ty: self.parse_type(),
            vararg: false,
        }
    }
}
