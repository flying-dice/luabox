//! LuaCATS annotation parsing (SPEC.md §3).
//!
//! A span-carrying, error-tolerant typed parser for the LuaLS/sumneko `---@`
//! comment dialect. Full LuaCATS compatibility is a non-negotiable spec pillar
//! (SPEC.md §1, §3): existing annotated codebases must check on day one.
//!
//! Layers:
//! - [`ty`] — the type-expression grammar ([`TypeExpr`]).
//! - [`Tag`] and its structs — every `---@` tag in structured form.
//! - [`parse_block`] — parse one doc-comment block into an [`AnnotationBlock`].
//! - [`harvest`] — walk a parsed Lua tree, group consecutive `---` comment
//!   trivia into blocks, and attach each to the statement it annotates.
//!
//! Spans are file-absolute byte offsets into the original source. Nothing here
//! panics on malformed input: bad types become [`TypeExprKind::Error`] nodes
//! with a recorded [`LuaCatsError`], and unknown tags round-trip as
//! [`Tag::Unknown`] (forward compatibility).

mod ty;

#[cfg(test)]
mod tests;

pub use ty::{FunParam, FunReturn, TableField, TypeExpr, TypeExprKind, TypeParser};

use crate::lua::{self, SyntaxKind};

/// A file-absolute byte range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    #[must_use]
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// An empty span at `at` (used for errors that point between tokens).
    #[must_use]
    pub fn empty(at: usize) -> Self {
        Span { start: at, end: at }
    }

    #[must_use]
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// A recoverable diagnostic from annotation parsing, anchored to a [`Span`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaCatsError {
    pub message: String,
    pub span: Span,
}

/// A plain description line (no tag), attaching as documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocLine {
    pub text: String,
    pub span: Span,
}

/// Visibility scope of an `@field`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldScope {
    Public,
    Protected,
    Private,
    Package,
}

/// The key of an `@field`: a plain name or a typed indexer (`[string]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKey {
    Name(String),
    Indexer(TypeExpr),
}

/// How an `@cast` operand mutates the variable's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastKind {
    /// `+T` — add `T` to the union.
    Add,
    /// `-T` — remove `T` from the union.
    Remove,
    /// `T` — replace the type outright.
    Replace,
}

/// `---@class [(exact)] Name[: Parent1, Parent2]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassTag {
    pub exact: bool,
    pub name: String,
    pub parents: Vec<TypeExpr>,
    pub span: Span,
}

/// `---@field [scope] name[?] Type [desc]` or `---@field [KeyType] Type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldTag {
    pub scope: Option<FieldScope>,
    pub key: FieldKey,
    pub optional: bool,
    pub ty: TypeExpr,
    pub desc: Option<String>,
    pub span: Span,
}

/// `---@param name[?] Type [desc]` (`name` may be `...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamTag {
    pub name: String,
    pub optional: bool,
    pub vararg: bool,
    pub ty: TypeExpr,
    pub desc: Option<String>,
    pub span: Span,
}

/// One value declared by a `@return` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnItem {
    pub ty: TypeExpr,
    pub name: Option<String>,
    pub vararg: bool,
}

/// `---@return Type [name] [, Type [name]]... [# desc]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReturnTag {
    pub items: Vec<ReturnItem>,
    pub desc: Option<String>,
    pub span: Span,
}

/// `---@type Type[, Type...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeTag {
    pub types: Vec<TypeExpr>,
    pub span: Span,
}

/// One `---| value [# desc]` member line of a multiline `@alias`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasMember {
    pub ty: TypeExpr,
    pub desc: Option<String>,
    pub span: Span,
}

/// `---@alias Name Type` (single-line) or `---@alias Name` followed by
/// `---| ...` member lines (multiline literal-union form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasTag {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub members: Vec<AliasMember>,
    pub span: Span,
}

/// One `@generic` parameter: `Name[: Constraint]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParam {
    pub name: String,
    pub constraint: Option<TypeExpr>,
    pub span: Span,
}

/// `---@generic T[: C][, U...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericTag {
    pub params: Vec<GenericParam>,
    pub span: Span,
}

/// `---@overload fun(...): ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverloadTag {
    pub ty: TypeExpr,
    pub span: Span,
}

/// One operand of an `@cast`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastOp {
    pub kind: CastKind,
    pub ty: TypeExpr,
}

/// `---@cast var [+|-]Type[, ...]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastTag {
    pub var: String,
    pub ops: Vec<CastOp>,
    pub span: Span,
}

/// `---@enum [(key)] Name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumTag {
    pub key: bool,
    pub name: String,
    pub span: Span,
}

/// `---@meta [name]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaTag {
    pub name: Option<String>,
    pub span: Span,
}

/// `---@operator name[(InputType)]: ResultType`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorTag {
    pub op: String,
    pub input: Option<TypeExpr>,
    pub result: TypeExpr,
    pub span: Span,
}

/// Legacy `---@vararg Type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarargTag {
    pub ty: TypeExpr,
    pub span: Span,
}

/// A tag parsed only for its free-text remainder: `@see`, `@deprecated`,
/// `@nodiscard`, `@async`, `@diagnostic`, `@version`, `@source`, `@package`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleTag {
    pub text: Option<String>,
    pub span: Span,
}

/// An unrecognised `@tag`, preserved verbatim for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTag {
    pub tag: String,
    pub text: Option<String>,
    pub span: Span,
}

/// luabox `---@use <module>` — import a `.luab` shape module (SHAPES.md §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseTag {
    /// The (possibly dotted) module path.
    pub module: String,
    pub span: Span,
}

/// luabox `---@struct <Struct>` — bind the following value to a `.luab`
/// struct (SHAPES.md §4). Generic use sites (`---@struct Pair<number>`)
/// keep their argument list as raw text; the shape checker parses it with
/// the shape type grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructTag {
    /// The struct name (without generic arguments).
    pub name: String,
    /// Raw text inside `<...>`, when the use site is generic.
    pub args: Option<String>,
    pub span: Span,
}

/// luabox `---@impl <Trait> for <Struct>` — conformance assertion
/// (SHAPES.md §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplTag {
    /// The trait being implemented.
    pub trait_name: String,
    /// The struct (or LuaCATS class) implementing it.
    pub struct_name: String,
    pub span: Span,
}

/// A single parsed `---@` tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tag {
    Class(ClassTag),
    Field(FieldTag),
    Param(ParamTag),
    Return(ReturnTag),
    Type(TypeTag),
    Alias(AliasTag),
    Generic(GenericTag),
    Overload(OverloadTag),
    Cast(CastTag),
    Enum(EnumTag),
    Meta(MetaTag),
    See(SimpleTag),
    Deprecated(SimpleTag),
    Nodiscard(SimpleTag),
    Async(SimpleTag),
    Diagnostic(SimpleTag),
    Version(SimpleTag),
    Source(SimpleTag),
    Package(SimpleTag),
    Operator(OperatorTag),
    Vararg(VarargTag),
    /// luabox shape import (`---@use`).
    Use(UseTag),
    /// luabox struct binding (`---@struct`).
    Struct(StructTag),
    /// luabox trait conformance (`---@impl`).
    Impl(ImplTag),
    Unknown(UnknownTag),
}

impl Tag {
    /// The file-absolute span of this tag (covering continuation lines for a
    /// multiline `@alias`).
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Tag::Class(t) => t.span,
            Tag::Field(t) => t.span,
            Tag::Param(t) => t.span,
            Tag::Return(t) => t.span,
            Tag::Type(t) => t.span,
            Tag::Alias(t) => t.span,
            Tag::Generic(t) => t.span,
            Tag::Overload(t) => t.span,
            Tag::Cast(t) => t.span,
            Tag::Enum(t) => t.span,
            Tag::Meta(t) => t.span,
            Tag::Operator(t) => t.span,
            Tag::Vararg(t) => t.span,
            Tag::See(t)
            | Tag::Deprecated(t)
            | Tag::Nodiscard(t)
            | Tag::Async(t)
            | Tag::Diagnostic(t)
            | Tag::Version(t)
            | Tag::Source(t)
            | Tag::Package(t) => t.span,
            Tag::Use(t) => t.span,
            Tag::Struct(t) => t.span,
            Tag::Impl(t) => t.span,
            Tag::Unknown(t) => t.span,
        }
    }
}

/// A fully parsed doc-comment block: its tags, description lines, recovered
/// errors, and the span it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationBlock {
    pub tags: Vec<Tag>,
    pub docs: Vec<DocLine>,
    pub errors: Vec<LuaCatsError>,
    pub span: Span,
}

/// A harvested block together with the source range of the statement it
/// annotates (`None` for a trailing or detached block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedItem {
    pub block: AnnotationBlock,
    pub target: Option<Span>,
}

// === Block parsing ===

/// Parse one doc-comment block. `text` is the raw source slice of the block
/// (its `---` lines, including interior newlines); `block_offset` is that
/// slice's absolute start, so all resulting spans are file-absolute.
#[must_use]
pub fn parse_block(text: &str, block_offset: usize) -> AnnotationBlock {
    let mut tags: Vec<Tag> = Vec::new();
    let mut docs: Vec<DocLine> = Vec::new();
    let mut errors: Vec<LuaCatsError> = Vec::new();

    for line in doc_lines(text, block_offset) {
        let (content, base) = lstrip(line.content, line.content_start);
        let content = content.trim_end();
        if let Some(rest) = content.strip_prefix('@') {
            let (word, body, body_base) = split_tag_word(rest, base + 1);
            let span = Span::new(base, base + content.len());
            let tag = parse_tag(word, body, body_base, span, &mut errors);
            tags.push(tag);
        } else if let Some(rest) = content.strip_prefix('|') {
            parse_alias_member(rest, base + 1, &mut tags, &mut errors);
        } else if !content.is_empty() {
            docs.push(DocLine {
                text: content.to_string(),
                span: Span::new(base, base + content.len()),
            });
        }
    }

    AnnotationBlock {
        tags,
        docs,
        errors,
        span: Span::new(block_offset, block_offset + text.len()),
    }
}

struct PhysLine<'a> {
    content: &'a str,
    content_start: usize,
}

/// Split a block slice into physical `---` lines, yielding the content after
/// the leading `---` with its absolute offset.
fn doc_lines(text: &str, block_offset: usize) -> Vec<PhysLine<'_>> {
    let mut out = Vec::new();
    for (line, off) in split_keep_offsets(text) {
        let (trimmed, base) = lstrip(line, block_offset + off);
        if let Some(after) = trimmed.strip_prefix("---") {
            out.push(PhysLine {
                content: after,
                content_start: base + 3,
            });
        }
    }
    out
}

/// Iterate lines of `text` (split on `\n`, trailing `\r` left in the slice),
/// yielding each line and its byte offset within `text`.
fn split_keep_offsets(text: &str) -> Vec<(&str, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            let mut end = i;
            if end > start && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
            out.push((&text[start..end], start));
            start = i + 1;
        }
    }
    if start <= text.len() {
        out.push((&text[start..], start));
    }
    out
}

/// Trim leading ASCII whitespace, returning the slice and its new base offset.
fn lstrip(s: &str, base: usize) -> (&str, usize) {
    let trimmed = s.trim_start();
    (trimmed, base + (s.len() - trimmed.len()))
}

/// Split `@word body` (the slice after `@`): the tag word, the body, and the
/// body's absolute offset.
fn split_tag_word(rest: &str, base: usize) -> (&str, &str, usize) {
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (&rest[..end], &rest[end..], base + end)
}

#[allow(clippy::too_many_lines)]
fn parse_tag(
    word: &str,
    body: &str,
    base: usize,
    span: Span,
    errors: &mut Vec<LuaCatsError>,
) -> Tag {
    match word {
        "class" => Tag::Class(parse_class(body, base, span, errors)),
        "field" => Tag::Field(parse_field(body, base, span, errors)),
        "param" => Tag::Param(parse_param(body, base, span, errors)),
        "return" => Tag::Return(parse_return(body, base, span, errors)),
        "type" => Tag::Type(parse_type_tag(body, base, span, errors)),
        "alias" => Tag::Alias(parse_alias(body, base, span, errors)),
        "generic" => Tag::Generic(parse_generic(body, base, span, errors)),
        "overload" => Tag::Overload(parse_overload(body, base, span, errors)),
        "cast" => Tag::Cast(parse_cast(body, base, span, errors)),
        "enum" => Tag::Enum(parse_enum(body, base, span)),
        "meta" => Tag::Meta(MetaTag {
            name: text_or_none(body),
            span,
        }),
        "operator" => Tag::Operator(parse_operator(body, base, span, errors)),
        "vararg" => Tag::Vararg(VarargTag {
            ty: parse_one_type(body, base, errors),
            span,
        }),
        "use" => Tag::Use(parse_use(body, base, span)),
        "struct" => Tag::Struct(parse_struct(body, base, span)),
        "impl" => Tag::Impl(parse_impl(body, base, span)),
        "see" => Tag::See(simple(body, span)),
        "deprecated" => Tag::Deprecated(simple(body, span)),
        "nodiscard" => Tag::Nodiscard(simple(body, span)),
        "async" => Tag::Async(simple(body, span)),
        "diagnostic" => Tag::Diagnostic(simple(body, span)),
        "version" => Tag::Version(simple(body, span)),
        "source" => Tag::Source(simple(body, span)),
        "package" => Tag::Package(simple(body, span)),
        _ => Tag::Unknown(UnknownTag {
            tag: word.to_string(),
            text: text_or_none(body),
            span,
        }),
    }
}

fn simple(body: &str, span: Span) -> SimpleTag {
    SimpleTag {
        text: text_or_none(body),
        span,
    }
}

fn text_or_none(body: &str) -> Option<String> {
    let t = body.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Drain a type parser's errors into the block-level error list.
fn drain(errors: &mut Vec<LuaCatsError>, parser: &mut TypeParser) {
    errors.append(&mut parser.take_errors());
}

/// The trailing description of `body` after `parser` stopped, with a leading
/// `#` marker stripped.
fn trailing_desc(body: &str, base: usize, parser: &TypeParser) -> Option<String> {
    let off = parser.cursor_offset().saturating_sub(base);
    let rest = body.get(off..).unwrap_or("").trim();
    let rest = rest.strip_prefix('#').map_or(rest, str::trim);
    text_or_none(rest)
}

/// Read a leading name: `...`, or a run of identifier/`.` characters. Returns
/// the name, the remaining slice, and its base offset.
fn take_name(s: &str, base: usize) -> Option<(String, &str, usize)> {
    let (s, base) = lstrip(s, base);
    if let Some(rest) = s.strip_prefix("...") {
        return Some(("...".to_string(), rest, base + 3));
    }
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || !c.is_ascii()))
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    Some((s[..end].to_string(), &s[end..], base + end))
}

/// The first whitespace-delimited word of `s`.
fn peek_word(s: &str) -> &str {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

fn parse_one_type(body: &str, base: usize, errors: &mut Vec<LuaCatsError>) -> TypeExpr {
    let mut p = TypeParser::new(body, base);
    let ty = p.parse_type();
    drain(errors, &mut p);
    ty
}

fn parse_class(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> ClassTag {
    let (mut s, mut base) = lstrip(body, base);
    let mut exact = false;
    if let Some(rest) = s.strip_prefix("(exact)") {
        exact = true;
        let stripped = rest.trim_start();
        base += s.len() - stripped.len();
        s = stripped;
    }
    let (name, rest, rest_base) = take_name(s, base).unwrap_or_else(|| (String::new(), s, base));
    let mut parents = Vec::new();
    let (rest, rest_base) = lstrip(rest, rest_base);
    if let Some(after) = rest.strip_prefix(':') {
        let mut p = TypeParser::new(after, rest_base + 1);
        loop {
            parents.push(p.parse_type());
            if !p.eat_comma() {
                break;
            }
        }
        drain(errors, &mut p);
    }
    ClassTag {
        exact,
        name,
        parents,
        span,
    }
}

fn parse_field(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> FieldTag {
    let (s, base) = lstrip(body, base);
    let mut scope = None;
    let (s, base) = match scope_of(peek_word(s)) {
        Some(sc) => {
            scope = Some(sc);
            let word = peek_word(s);
            let idx = s.find(word).unwrap_or(0) + word.len();
            lstrip(&s[idx..], base + idx)
        }
        None => (s, base),
    };

    // Indexer form: `[KeyType] ValueType`.
    if let Some((inner, inner_base, rest, rest_base)) = split_bracket(s, base) {
        let key = parse_one_type(inner, inner_base, errors);
        let mut p = TypeParser::new(rest, rest_base);
        let ty = p.parse_type();
        let desc = trailing_desc(rest, rest_base, &p);
        drain(errors, &mut p);
        return FieldTag {
            scope,
            key: FieldKey::Indexer(key),
            optional: false,
            ty,
            desc,
            span,
        };
    }

    let (name, rest, rest_base) = take_name(s, base).unwrap_or_else(|| (String::new(), s, base));
    let (optional, rest, rest_base) = eat_q(rest, rest_base);
    let mut p = TypeParser::new(rest, rest_base);
    let ty = p.parse_type();
    let desc = trailing_desc(rest, rest_base, &p);
    drain(errors, &mut p);
    FieldTag {
        scope,
        key: FieldKey::Name(name),
        optional,
        ty,
        desc,
        span,
    }
}

fn scope_of(word: &str) -> Option<FieldScope> {
    match word {
        "public" => Some(FieldScope::Public),
        "protected" => Some(FieldScope::Protected),
        "private" => Some(FieldScope::Private),
        "package" => Some(FieldScope::Package),
        _ => None,
    }
}

/// Match a leading `[ ... ]` (balanced), returning the inner slice, the rest,
/// and their base offsets.
fn split_bracket(s: &str, base: usize) -> Option<(&str, usize, &str, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let inner = &s[1..i];
                    let rest = &s[i + 1..];
                    return Some((inner, base + 1, rest, base + i + 1));
                }
            }
            _ => {}
        }
    }
    None
}

fn eat_q(s: &str, base: usize) -> (bool, &str, usize) {
    if let Some(rest) = s.strip_prefix('?') {
        (true, rest, base + 1)
    } else {
        (false, s, base)
    }
}

fn parse_param(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> ParamTag {
    let (name, rest, rest_base) =
        take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let vararg = name == "...";
    let (optional, rest, rest_base) = eat_q(rest, rest_base);
    let mut p = TypeParser::new(rest, rest_base);
    let ty = p.parse_type();
    let desc = trailing_desc(rest, rest_base, &p);
    drain(errors, &mut p);
    ParamTag {
        name,
        optional,
        vararg,
        ty,
        desc,
        span,
    }
}

fn parse_return(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> ReturnTag {
    let mut p = TypeParser::new(body, base);
    let mut items = Vec::new();
    while !p.at_end() {
        let ty = p.parse_type();
        let mut name = None;
        let mut vararg = false;
        if let Some(n) = p.take_return_name() {
            name = Some(n);
        } else if p.eat_ellipsis() {
            vararg = true;
        }
        items.push(ReturnItem { ty, name, vararg });
        if !p.eat_comma() {
            break;
        }
    }
    let desc = trailing_desc(body, base, &p);
    drain(errors, &mut p);
    ReturnTag { items, desc, span }
}

fn parse_type_tag(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> TypeTag {
    let mut p = TypeParser::new(body, base);
    let mut types = Vec::new();
    loop {
        types.push(p.parse_type());
        if !p.eat_comma() {
            break;
        }
    }
    drain(errors, &mut p);
    TypeTag { types, span }
}

fn parse_alias(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> AliasTag {
    let (name, rest, rest_base) =
        take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let (rest, rest_base) = lstrip(rest, rest_base);
    let ty = if rest.is_empty() {
        None
    } else {
        let mut p = TypeParser::new(rest, rest_base);
        let t = p.parse_type();
        drain(errors, &mut p);
        Some(t)
    };
    AliasTag {
        name,
        ty,
        members: Vec::new(),
        span,
    }
}

/// Attach a `---| value [# desc]` member line to the preceding `@alias`.
fn parse_alias_member(rest: &str, base: usize, tags: &mut [Tag], errors: &mut Vec<LuaCatsError>) {
    let (body, body_base) = lstrip(rest, base);
    let body = body.trim_end();
    let mut p = TypeParser::new(body, body_base);
    let ty = p.parse_type();
    let desc = trailing_desc(body, body_base, &p);
    drain(errors, &mut p);
    let member = AliasMember {
        ty,
        desc,
        span: Span::new(base, base + rest.trim_end().len()),
    };
    if let Some(Tag::Alias(alias)) = tags.last_mut() {
        alias.span.end = member.span.end;
        alias.members.push(member);
    }
}

fn parse_generic(
    body: &str,
    base: usize,
    span: Span,
    errors: &mut Vec<LuaCatsError>,
) -> GenericTag {
    let mut p = TypeParser::new(body, base);
    let mut params = Vec::new();
    while let Some((name, name_span)) = p.take_ident() {
        let mut constraint = None;
        let mut end = name_span.end;
        if p.eat_colon() {
            let c = p.parse_type();
            end = c.span.end;
            constraint = Some(c);
        }
        params.push(GenericParam {
            name,
            constraint,
            span: Span::new(name_span.start, end),
        });
        if !p.eat_comma() {
            break;
        }
    }
    drain(errors, &mut p);
    GenericTag { params, span }
}

fn parse_overload(
    body: &str,
    base: usize,
    span: Span,
    errors: &mut Vec<LuaCatsError>,
) -> OverloadTag {
    OverloadTag {
        ty: parse_one_type(body, base, errors),
        span,
    }
}

fn parse_cast(body: &str, base: usize, span: Span, errors: &mut Vec<LuaCatsError>) -> CastTag {
    let (var, rest, rest_base) =
        take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let mut p = TypeParser::new(rest, rest_base);
    let mut ops = Vec::new();
    while !p.at_end() {
        let kind = match p.eat_sign() {
            Some('+') => CastKind::Add,
            Some('-') => CastKind::Remove,
            _ => CastKind::Replace,
        };
        ops.push(CastOp {
            kind,
            ty: p.parse_type(),
        });
        if !p.eat_comma() {
            break;
        }
    }
    drain(errors, &mut p);
    CastTag { var, ops, span }
}

fn parse_enum(body: &str, base: usize, span: Span) -> EnumTag {
    let (s, base) = lstrip(body, base);
    let mut key = false;
    let s = if let Some(rest) = s.strip_prefix("(key)") {
        key = true;
        rest.trim_start()
    } else {
        s
    };
    let name = take_name(s, base).map_or_else(String::new, |(n, ..)| n);
    EnumTag { key, name, span }
}

/// `---@use <module>` — the module path is the first name-like word.
fn parse_use(body: &str, base: usize, span: Span) -> UseTag {
    let module = take_name(body, base).map_or_else(String::new, |(n, ..)| n);
    UseTag { module, span }
}

/// `---@struct <Name>[<Args>]` — the name plus the raw generic-argument
/// text (the shape checker parses arguments with the shape grammar).
fn parse_struct(body: &str, base: usize, span: Span) -> StructTag {
    let (name, rest, _) = take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let rest = rest.trim_start();
    let args = rest.strip_prefix('<').and_then(|inner| {
        // Take up to the matching top-level `>` (nested generics balance).
        let mut depth = 1usize;
        for (i, ch) in inner.char_indices() {
            match ch {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(inner[..i].trim().to_string());
                    }
                }
                _ => {}
            }
        }
        None
    });
    StructTag { name, args, span }
}

/// `---@impl <Trait> for <Struct>`.
fn parse_impl(body: &str, base: usize, span: Span) -> ImplTag {
    let (trait_name, rest, rest_base) =
        take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let (rest, rest_base) = lstrip(rest, rest_base);
    let struct_name = rest
        .strip_prefix("for")
        .and_then(|after| take_name(after, rest_base + 3))
        .map_or_else(String::new, |(n, ..)| n);
    ImplTag {
        trait_name,
        struct_name,
        span,
    }
}

fn parse_operator(
    body: &str,
    base: usize,
    span: Span,
    errors: &mut Vec<LuaCatsError>,
) -> OperatorTag {
    let (op, rest, rest_base) =
        take_name(body, base).unwrap_or_else(|| (String::new(), body, base));
    let mut p = TypeParser::new(rest, rest_base);
    let mut input = None;
    if p.eat_lparen() {
        input = Some(p.parse_type());
        p.eat_rparen();
    }
    p.eat_colon();
    let result = p.parse_type();
    drain(errors, &mut p);
    OperatorTag {
        op,
        input,
        result,
        span,
    }
}

// === Harvesting ===

/// One collected token with the facts `harvest` needs (offsets, triviality,
/// newline count).
struct Tk {
    start: usize,
    end: usize,
    is_ws: bool,
    is_comment: bool,
    is_doc: bool,
    newlines: usize,
}

/// Whether a `--` comment's text is a LuaCATS doc comment (`---`, but not a
/// `----` decoration rule or a `--[[` long comment).
fn is_doc_comment(text: &str) -> bool {
    text.starts_with("---") && text.as_bytes().get(3) != Some(&b'-')
}

/// Walk a parsed Lua tree, group consecutive `---` comment trivia into blocks,
/// and associate each block with the range of the statement it precedes (or
/// `None` for a trailing or detached block). Same-line trailing `---@type`
/// comments attach to their own line's statement.
#[must_use]
pub fn harvest(parse: &lua::Parse) -> Vec<AnnotatedItem> {
    let root = parse.syntax();
    let text = root.text().to_string();
    let stmts = collect_statements(&root);
    let tks = collect_tokens(&root);
    let blocks = group_blocks(&tks);

    blocks
        .into_iter()
        .map(|(first, last)| {
            let block_start = tks[first].start;
            let block_end = tks[last].end;
            let target = resolve_target(&tks, &stmts, first, last);
            let slice = text.get(block_start..block_end).unwrap_or("");
            AnnotatedItem {
                block: parse_block(slice, block_start),
                target,
            }
        })
        .collect()
}

fn is_statement(kind: SyntaxKind) -> bool {
    use SyntaxKind::{
        ASSIGN_STMT, BREAK_STMT, CALL_STMT, DO_STMT, FUNCTION_DECL_STMT, GENERIC_FOR_STMT,
        GOTO_STMT, IF_STMT, LABEL_STMT, LOCAL_FUNCTION_STMT, LOCAL_STMT, NUMERIC_FOR_STMT,
        REPEAT_STMT, RETURN_STMT, WHILE_STMT,
    };
    matches!(
        kind,
        LOCAL_STMT
            | ASSIGN_STMT
            | CALL_STMT
            | DO_STMT
            | WHILE_STMT
            | REPEAT_STMT
            | IF_STMT
            | NUMERIC_FOR_STMT
            | GENERIC_FOR_STMT
            | FUNCTION_DECL_STMT
            | LOCAL_FUNCTION_STMT
            | RETURN_STMT
            | BREAK_STMT
            | GOTO_STMT
            | LABEL_STMT
    )
}

fn collect_statements(root: &lua::SyntaxNode) -> Vec<(usize, usize)> {
    root.descendants()
        .filter(|n| is_statement(n.kind()))
        .map(|n| {
            let r = n.text_range();
            (usize::from(r.start()), usize::from(r.end()))
        })
        .collect()
}

fn collect_tokens(root: &lua::SyntaxNode) -> Vec<Tk> {
    root.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .map(|token| {
            let r = token.text_range();
            let kind = token.kind();
            let is_comment = kind == SyntaxKind::COMMENT;
            let is_ws = kind == SyntaxKind::WHITESPACE;
            let newlines = if is_ws {
                token.text().bytes().filter(|&b| b == b'\n').count()
            } else {
                0
            };
            Tk {
                start: usize::from(r.start()),
                end: usize::from(r.end()),
                is_ws,
                is_comment,
                is_doc: is_comment && is_doc_comment(token.text()),
                newlines,
            }
        })
        .collect()
}

/// Group doc-comment tokens into blocks of `(first_index, last_index)`. A
/// block continues while successive doc comments are separated only by
/// whitespace containing exactly one newline (i.e. adjacent lines).
fn group_blocks(tks: &[Tk]) -> Vec<(usize, usize)> {
    let mut blocks = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    for (i, t) in tks.iter().enumerate() {
        if t.is_doc {
            match run {
                None => run = Some((i, i)),
                Some((first, last)) => {
                    if adjacent(tks, last, i) {
                        run = Some((first, i));
                    } else {
                        blocks.push((first, last));
                        run = Some((i, i));
                    }
                }
            }
        } else if !t.is_ws
            && let Some(block) = run.take()
        {
            blocks.push(block);
        }
    }
    if let Some(block) = run.take() {
        blocks.push(block);
    }
    blocks
}

/// Whether doc comments at `last` and `next` are on adjacent lines (only
/// whitespace between them, totalling exactly one newline).
fn adjacent(tks: &[Tk], last: usize, next: usize) -> bool {
    let mut newlines = 0;
    for t in &tks[last + 1..next] {
        if !t.is_ws {
            return false;
        }
        newlines += t.newlines;
    }
    newlines == 1
}

fn resolve_target(tks: &[Tk], stmts: &[(usize, usize)], first: usize, last: usize) -> Option<Span> {
    // Scan back over whitespace for the preceding real token.
    let mut nl_before = 0;
    let mut prev = None;
    let mut j = first;
    while j > 0 {
        j -= 1;
        if tks[j].is_ws {
            nl_before += tks[j].newlines;
        } else {
            prev = Some(j);
            break;
        }
    }
    if let Some(pj) = prev
        && !tks[pj].is_comment
        && nl_before == 0
    {
        // Same-line trailing comment: attach to the preceding statement.
        return innermost(stmts, tks[pj].start);
    }

    // Leading block: scan forward over whitespace for the next real token.
    let mut nl_after = 0;
    let mut next = None;
    let mut k = last + 1;
    while k < tks.len() {
        if tks[k].is_ws {
            nl_after += tks[k].newlines;
            k += 1;
        } else {
            next = Some(k);
            break;
        }
    }
    match next {
        Some(nk) if !tks[nk].is_comment && nl_after <= 1 => innermost(stmts, tks[nk].start),
        _ => None,
    }
}

/// The innermost (smallest) statement range containing `offset`.
fn innermost(stmts: &[(usize, usize)], offset: usize) -> Option<Span> {
    stmts
        .iter()
        .filter(|&&(s, e)| s <= offset && offset < e)
        .min_by_key(|&&(s, e)| e - s)
        .map(|&(s, e)| Span::new(s, e))
}
