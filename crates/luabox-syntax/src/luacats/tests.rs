//! Tests for the LuaCATS annotation parser (SPEC.md §3). Exhaustive by
//! intent: this is a day-one compatibility surface, so every tag form and
//! type construct gets direct coverage, alongside error-tolerance, harvest
//! semantics, exact spans, and a no-panic property test.

use super::*;
use crate::lua::{Dialect, parse as lua_parse};
use proptest::prelude::*;

// === Helpers ===

fn block(text: &str) -> AnnotationBlock {
    parse_block(text, 0)
}

/// Parse a one-line block and return its single tag.
fn one_tag(line: &str) -> Tag {
    let b = block(line);
    assert_eq!(
        b.tags.len(),
        1,
        "expected exactly one tag in {line:?}, got {:?}",
        b.tags
    );
    assert!(
        b.errors.is_empty(),
        "unexpected errors in {line:?}: {:?}",
        b.errors
    );
    b.tags.into_iter().next().unwrap()
}

/// Parse a bare type expression, asserting it is error-free.
fn ty(s: &str) -> TypeExpr {
    let mut p = TypeParser::new(s, 0);
    let t = p.parse_type();
    let errs = p.take_errors();
    assert!(
        errs.is_empty(),
        "unexpected type errors for {s:?}: {errs:?}"
    );
    t
}

/// Parse a bare type expression, returning it together with its errors.
fn ty_err(s: &str) -> (TypeExpr, Vec<LuaCatsError>) {
    let mut p = TypeParser::new(s, 0);
    let t = p.parse_type();
    let errs = p.take_errors();
    (t, errs)
}

fn named_of(t: &TypeExpr) -> (&str, &[TypeExpr]) {
    match &t.kind {
        TypeExprKind::Named { name, args } => (name, args),
        other => panic!("expected Named, got {other:?}"),
    }
}

// === @class ===

#[test]
fn class_plain() {
    let Tag::Class(c) = one_tag("---@class Animal") else {
        panic!()
    };
    assert!(!c.exact);
    assert_eq!(c.name, "Animal");
    assert!(c.parents.is_empty());
}

#[test]
fn class_with_parents() {
    let Tag::Class(c) = one_tag("---@class Dog : Animal, Comparable") else {
        panic!()
    };
    assert_eq!(c.name, "Dog");
    let parents: Vec<_> = c.parents.iter().map(|p| named_of(p).0).collect();
    assert_eq!(parents, ["Animal", "Comparable"]);
}

#[test]
fn class_exact_and_dotted_and_generic_parent() {
    let Tag::Class(c) = one_tag("---@class (exact) mod.Cat : Base<string>") else {
        panic!()
    };
    assert!(c.exact);
    assert_eq!(c.name, "mod.Cat");
    let (name, args) = named_of(&c.parents[0]);
    assert_eq!(name, "Base");
    assert_eq!(args.len(), 1);
}

// === @field ===

#[test]
fn field_plain() {
    let Tag::Field(f) = one_tag("---@field name string the name") else {
        panic!()
    };
    assert!(f.scope.is_none());
    assert_eq!(f.key, FieldKey::Name("name".to_string()));
    assert!(!f.optional);
    assert_eq!(named_of(&f.ty).0, "string");
    assert_eq!(f.desc.as_deref(), Some("the name"));
}

#[test]
fn field_scopes_and_optional() {
    for (src, scope) in [
        ("---@field public a string", FieldScope::Public),
        ("---@field protected b string", FieldScope::Protected),
        ("---@field private c string", FieldScope::Private),
        ("---@field package d string", FieldScope::Package),
    ] {
        let Tag::Field(f) = one_tag(src) else {
            panic!()
        };
        assert_eq!(f.scope, Some(scope));
    }
    let Tag::Field(f) = one_tag("---@field count? integer") else {
        panic!()
    };
    assert!(f.optional);
}

#[test]
fn field_indexer_form() {
    let Tag::Field(f) = one_tag("---@field [string] integer") else {
        panic!()
    };
    let FieldKey::Indexer(key) = &f.key else {
        panic!("expected indexer key")
    };
    assert_eq!(named_of(key).0, "string");
    assert_eq!(named_of(&f.ty).0, "integer");
}

// === @param ===

#[test]
fn param_basic_optional_vararg() {
    let Tag::Param(p) = one_tag("---@param x integer the count") else {
        panic!()
    };
    assert_eq!(p.name, "x");
    assert!(!p.optional && !p.vararg);
    assert_eq!(p.desc.as_deref(), Some("the count"));

    let Tag::Param(p) = one_tag("---@param opt? string") else {
        panic!()
    };
    assert!(p.optional);

    let Tag::Param(p) = one_tag("---@param ... any extra args") else {
        panic!()
    };
    assert!(p.vararg);
    assert_eq!(p.name, "...");
    assert_eq!(p.desc.as_deref(), Some("extra args"));
}

// === @return ===

#[test]
fn return_forms() {
    let Tag::Return(r) = one_tag("---@return string") else {
        panic!()
    };
    assert_eq!(r.items.len(), 1);
    assert_eq!(named_of(&r.items[0].ty).0, "string");

    let Tag::Return(r) = one_tag("---@return string name # a description") else {
        panic!()
    };
    assert_eq!(r.items[0].name.as_deref(), Some("name"));
    assert_eq!(r.desc.as_deref(), Some("a description"));

    let Tag::Return(r) = one_tag("---@return string, number") else {
        panic!()
    };
    assert_eq!(r.items.len(), 2);

    let Tag::Return(r) = one_tag("---@return string ...") else {
        panic!()
    };
    assert!(r.items[0].vararg);
}

#[test]
fn multiple_return_lines() {
    let b = block("---@return string\n---@return integer");
    let returns = b
        .tags
        .iter()
        .filter(|t| matches!(t, Tag::Return(_)))
        .count();
    assert_eq!(returns, 2);
}

// === @type ===

#[test]
fn type_single_and_multi() {
    let Tag::Type(t) = one_tag("---@type string|nil") else {
        panic!()
    };
    assert_eq!(t.types.len(), 1);
    assert!(matches!(t.types[0].kind, TypeExprKind::Union(_)));

    let Tag::Type(t) = one_tag("---@type integer, string") else {
        panic!()
    };
    assert_eq!(t.types.len(), 2);
}

// === @alias ===

#[test]
fn alias_single_line() {
    let Tag::Alias(a) = one_tag("---@alias Id integer") else {
        panic!()
    };
    assert_eq!(a.name, "Id");
    assert_eq!(named_of(a.ty.as_ref().unwrap()).0, "integer");
    assert!(a.members.is_empty());
}

#[test]
fn alias_multiline_literal_union() {
    let b = block("---@alias Color\n---| '\"red\"' # the red one\n---| '\"green\"'");
    assert_eq!(b.tags.len(), 1);
    let Tag::Alias(a) = &b.tags[0] else { panic!() };
    assert_eq!(a.name, "Color");
    assert!(a.ty.is_none());
    assert_eq!(a.members.len(), 2);
    assert!(matches!(a.members[0].ty.kind, TypeExprKind::StringLit(_)));
    assert_eq!(a.members[0].desc.as_deref(), Some("the red one"));
    // The alias span extends across the continuation lines.
    assert!(a.span.end > a.span.start + 15);
}

#[test]
fn alias_multiline_backtick_members() {
    let b = block("---@alias Handler\n---| `EventA`\n---| `EventB`");
    let Tag::Alias(a) = &b.tags[0] else { panic!() };
    assert_eq!(a.members.len(), 2);
    let TypeExprKind::Backtick(t) = &a.members[0].ty.kind else {
        panic!("expected backtick member")
    };
    assert_eq!(t, "EventA");
}

// === @generic ===

#[test]
fn generic_forms() {
    let Tag::Generic(g) = one_tag("---@generic T") else {
        panic!()
    };
    assert_eq!(g.params.len(), 1);
    assert_eq!(g.params[0].name, "T");
    assert!(g.params[0].constraint.is_none());

    let Tag::Generic(g) = one_tag("---@generic T : string, U") else {
        panic!()
    };
    assert_eq!(g.params.len(), 2);
    assert_eq!(
        named_of(g.params[0].constraint.as_ref().unwrap()).0,
        "string"
    );
    assert_eq!(g.params[1].name, "U");

    // A constraint containing commas must not split the parameter list.
    let Tag::Generic(g) = one_tag("---@generic K : table<string, number>, V") else {
        panic!()
    };
    assert_eq!(g.params.len(), 2);
}

// === @overload ===

#[test]
fn overload_fun() {
    let Tag::Overload(o) = one_tag("---@overload fun(a: integer): string") else {
        panic!()
    };
    let TypeExprKind::Fun { params, returns } = &o.ty.kind else {
        panic!("expected fun type")
    };
    assert_eq!(params.len(), 1);
    assert_eq!(returns.len(), 1);
}

// === @cast ===

#[test]
fn cast_ops() {
    let Tag::Cast(c) = one_tag("---@cast x +integer, -string, boolean") else {
        panic!()
    };
    assert_eq!(c.var, "x");
    assert_eq!(c.ops.len(), 3);
    assert_eq!(c.ops[0].kind, CastKind::Add);
    assert_eq!(c.ops[1].kind, CastKind::Remove);
    assert_eq!(c.ops[2].kind, CastKind::Replace);
}

// === @enum / @meta ===

#[test]
fn enum_and_meta() {
    let Tag::Enum(e) = one_tag("---@enum Direction") else {
        panic!()
    };
    assert!(!e.key);
    assert_eq!(e.name, "Direction");

    let Tag::Enum(e) = one_tag("---@enum (key) Flags") else {
        panic!()
    };
    assert!(e.key);
    assert_eq!(e.name, "Flags");

    let Tag::Meta(m) = one_tag("---@meta") else {
        panic!()
    };
    assert!(m.name.is_none());

    let Tag::Meta(m) = one_tag("---@meta love") else {
        panic!()
    };
    assert_eq!(m.name.as_deref(), Some("love"));
}

// === @operator / @vararg ===

#[test]
fn operator_and_vararg() {
    let Tag::Operator(o) = one_tag("---@operator add(number): Vec") else {
        panic!()
    };
    assert_eq!(o.op, "add");
    assert_eq!(named_of(o.input.as_ref().unwrap()).0, "number");
    assert_eq!(named_of(&o.result).0, "Vec");

    let Tag::Operator(o) = one_tag("---@operator len: integer") else {
        panic!()
    };
    assert!(o.input.is_none());
    assert_eq!(named_of(&o.result).0, "integer");

    let Tag::Vararg(v) = one_tag("---@vararg string") else {
        panic!()
    };
    assert_eq!(named_of(&v.ty).0, "string");
}

// === Common extras + unknown ===

#[test]
fn simple_tags() {
    assert!(matches!(one_tag("---@deprecated"), Tag::Deprecated(_)));
    assert!(matches!(one_tag("---@nodiscard"), Tag::Nodiscard(_)));
    assert!(matches!(one_tag("---@async"), Tag::Async(_)));
    assert!(matches!(one_tag("---@package"), Tag::Package(_)));
    assert!(matches!(one_tag("---@private"), Tag::Private(_)));
    assert!(matches!(one_tag("---@protected"), Tag::Protected(_)));
    let Tag::See(s) = one_tag("---@see foo.bar") else {
        panic!()
    };
    assert_eq!(s.text.as_deref(), Some("foo.bar"));
    let Tag::Diagnostic(d) = one_tag("---@diagnostic disable: undefined-global") else {
        panic!()
    };
    assert_eq!(d.text.as_deref(), Some("disable: undefined-global"));
    let Tag::Version(v) = one_tag("---@version 5.4") else {
        panic!()
    };
    assert_eq!(v.text.as_deref(), Some("5.4"));
    assert!(matches!(one_tag("---@source foo.lua:1"), Tag::Source(_)));
}

#[test]
fn unrecognised_tags_parse_as_unknown() {
    // Tags outside the recognised LuaCATS vocabulary parse as Unknown —
    // surfaced to the user, never silently accepted.
    for src in [
        "---@use geometry",
        "---@struct Point",
        "---@impl Widget for Circle",
    ] {
        let Tag::Unknown(u) = one_tag(src) else {
            panic!("retired tag must parse as Unknown: {src}")
        };
        assert!(!u.tag.is_empty());
    }
}

#[test]
fn unknown_tag_roundtrips() {
    let Tag::Unknown(u) = one_tag("---@futuristic some payload here") else {
        panic!("unknown tags must never be an error")
    };
    assert_eq!(u.tag, "futuristic");
    assert_eq!(u.text.as_deref(), Some("some payload here"));
}

#[test]
fn description_lines_attach_as_docs() {
    let b = block("--- A summary line.\n---@param x integer\n--- Trailing note.");
    assert_eq!(b.docs.len(), 2);
    assert_eq!(b.docs[0].text, "A summary line.");
    assert_eq!(b.docs[1].text, "Trailing note.");
    assert_eq!(b.tags.len(), 1);
}

// === Type expressions: one per construct ===

#[test]
fn type_named_and_dotted() {
    assert_eq!(named_of(&ty("string")).0, "string");
    assert_eq!(named_of(&ty("a.b.C")).0, "a.b.C");
}

#[test]
fn type_optional() {
    let t = ty("string?");
    let TypeExprKind::Optional(inner) = &t.kind else {
        panic!()
    };
    assert_eq!(named_of(inner).0, "string");
}

#[test]
fn type_union() {
    let TypeExprKind::Union(m) = ty("a|b|c").kind else {
        panic!()
    };
    assert_eq!(m.len(), 3);
}

#[test]
fn type_array() {
    let TypeExprKind::Array(inner) = ty("integer[]").kind else {
        panic!()
    };
    assert_eq!(named_of(&inner).0, "integer");
}

#[test]
fn type_tuple() {
    let TypeExprKind::Tuple(items) = ty("[integer, string]").kind else {
        panic!()
    };
    assert_eq!(items.len(), 2);
}

#[test]
fn type_dictionary_indexer() {
    let TypeExprKind::Table(fields) = ty("{ [string]: integer }").kind else {
        panic!()
    };
    assert!(matches!(fields[0], TableField::Indexer { .. }));
}

#[test]
fn type_table_literal_fields() {
    let TypeExprKind::Table(fields) = ty("{ x: integer, y?: string }").kind else {
        panic!()
    };
    assert_eq!(fields.len(), 2);
    let TableField::Named { name, optional, .. } = &fields[1] else {
        panic!()
    };
    assert_eq!(name, "y");
    assert!(optional);
}

#[test]
fn type_generic_application() {
    let t = ty("table<string, number>");
    let (name, args) = named_of(&t);
    assert_eq!(name, "table");
    assert_eq!(args.len(), 2);
}

#[test]
fn type_fun_full() {
    let t = ty("fun(a: integer, opt?: string, ...: any): boolean, string");
    let TypeExprKind::Fun { params, returns } = &t.kind else {
        panic!()
    };
    assert_eq!(params.len(), 3);
    assert!(params[1].optional);
    assert!(params[2].vararg);
    assert_eq!(returns.len(), 2);
}

#[test]
fn type_fun_named_and_paren_returns() {
    let t = ty("fun(): ok: boolean");
    let TypeExprKind::Fun { returns, .. } = &t.kind else {
        panic!()
    };
    assert_eq!(returns[0].name.as_deref(), Some("ok"));

    let t = ty("fun(): (string, number)");
    let TypeExprKind::Fun { returns, .. } = &t.kind else {
        panic!()
    };
    assert_eq!(returns.len(), 2);
}

#[test]
fn type_literals() {
    assert!(matches!(ty("\"lit\"").kind, TypeExprKind::StringLit(_)));
    assert!(matches!(ty("'lit'").kind, TypeExprKind::StringLit(_)));
    assert_eq!(ty("123").kind, TypeExprKind::NumberLit("123".to_string()));
    assert_eq!(ty("-1").kind, TypeExprKind::NumberLit("-1".to_string()));
    assert_eq!(ty("true").kind, TypeExprKind::BoolLit(true));
    assert_eq!(ty("false").kind, TypeExprKind::BoolLit(false));
    let TypeExprKind::Backtick(s) = ty("`T`").kind else {
        panic!()
    };
    assert_eq!(s, "T");
}

#[test]
fn type_paren() {
    let TypeExprKind::Paren(inner) = ty("(string)").kind else {
        panic!()
    };
    assert_eq!(named_of(&inner).0, "string");
}

#[test]
fn type_table_fields_may_be_separated_by_semicolons() {
    let TypeExprKind::Table(fields) = ty("{ a: string; b: number }").kind else {
        panic!()
    };
    assert_eq!(fields.len(), 2);
    let TableField::Named { name, .. } = &fields[1] else {
        panic!("second field should be named")
    };
    assert_eq!(name, "b");
}

#[test]
fn type_string_literal_keeps_escaped_quotes_verbatim() {
    // The scanner skips `\"` rather than ending the literal there.
    assert_eq!(
        ty(r#""a\"b""#).kind,
        TypeExprKind::StringLit(r#""a\"b""#.to_string())
    );
    assert_eq!(
        ty(r"'a\'b'").kind,
        TypeExprKind::StringLit(r"'a\'b'".to_string())
    );
}

#[test]
fn type_number_literals_end_at_the_next_delimiter() {
    let TypeExprKind::Union(m) = ty("1|2").kind else {
        panic!()
    };
    assert_eq!(m[0].kind, TypeExprKind::NumberLit("1".to_string()));
    assert_eq!(m[1].kind, TypeExprKind::NumberLit("2".to_string()));
    assert_eq!(m[0].span, Span::new(0, 1));
}

#[test]
fn type_fun_params_may_be_unnamed_bare_types() {
    // A parameter that cannot start with a name is parsed as a bare type.
    let TypeExprKind::Fun { params, .. } = ty("fun({ a: string }, [integer]): boolean").kind else {
        panic!()
    };
    assert_eq!(params.len(), 2);
    assert!(params.iter().all(|p| p.name.is_empty() && !p.vararg));
    assert!(matches!(
        params[0].ty.as_ref().unwrap().kind,
        TypeExprKind::Table(_)
    ));
    assert!(matches!(
        params[1].ty.as_ref().unwrap().kind,
        TypeExprKind::Tuple(_)
    ));
}

#[test]
fn type_fun_vararg_return_is_an_any_marked_vararg() {
    let TypeExprKind::Fun { returns, .. } = ty("fun(): ...").kind else {
        panic!()
    };
    assert_eq!(returns.len(), 1);
    assert!(returns[0].vararg);
    assert!(returns[0].name.is_none());
    assert_eq!(named_of(&returns[0].ty).0, "any");
}

#[test]
fn type_fun_may_have_no_returns_at_all() {
    // A bare `:` and an empty parenthesised list both yield zero returns.
    for src in ["fun():", "fun(): ()"] {
        let TypeExprKind::Fun { returns, .. } = ty(src).kind else {
            panic!("{src}")
        };
        assert!(returns.is_empty(), "{src}");
    }
}

#[test]
fn type_dotted_path_stops_at_a_trailing_dot() {
    // `a.` leaves the `.` unconsumed rather than inventing a segment.
    let mut p = TypeParser::new("a.", 0);
    let t = p.parse_type();
    assert_eq!(named_of(&t).0, "a");
    assert_eq!(t.span, Span::new(0, 1));
    assert!(p.take_errors().is_empty());
    assert!(!p.at_end(), "the trailing '.' is still pending");
}

#[test]
fn take_ident_declines_non_identifier_tokens() {
    let mut p = TypeParser::new("123", 0);
    assert_eq!(p.take_ident(), None);
    assert!(!p.at_end(), "declining must not consume the token");
    let mut p = TypeParser::new("name rest", 0);
    assert_eq!(p.take_ident(), Some(("name".to_string(), Span::new(0, 4))));
}

// === Type expressions: precedence & nesting ===

#[test]
fn precedence_postfix_binds_tighter_than_union() {
    // `a|b?` parses as `a | (b?)`, not `(a|b)?`.
    let TypeExprKind::Union(m) = ty("a|b?").kind else {
        panic!()
    };
    assert_eq!(m.len(), 2);
    assert!(matches!(m[0].kind, TypeExprKind::Named { .. }));
    assert!(matches!(m[1].kind, TypeExprKind::Optional(_)));
}

#[test]
fn nesting_table_of_fun() {
    let t = ty("table<string, fun(x: integer?): string|nil>");
    let (_, args) = named_of(&t);
    let TypeExprKind::Fun { params, returns } = &args[1].kind else {
        panic!("second arg should be a fun type")
    };
    assert!(matches!(
        params[0].ty.as_ref().unwrap().kind,
        TypeExprKind::Optional(_)
    ));
    assert!(matches!(returns[0].ty.kind, TypeExprKind::Union(_)));
}

#[test]
fn nesting_array_of_dictionary() {
    let TypeExprKind::Array(inner) = ty("{ [string]: integer[] }[]").kind else {
        panic!()
    };
    let TypeExprKind::Table(fields) = &inner.kind else {
        panic!()
    };
    let TableField::Indexer { value, .. } = &fields[0] else {
        panic!()
    };
    assert!(matches!(value.kind, TypeExprKind::Array(_)));
}

#[test]
fn nesting_tuple_in_union() {
    let TypeExprKind::Union(m) = ty("[integer, string] | nil").kind else {
        panic!()
    };
    assert!(matches!(m[0].kind, TypeExprKind::Tuple(_)));
    assert_eq!(named_of(&m[1]).0, "nil");
}

// === Error tolerance ===

#[test]
fn malformed_type_yields_error_node_with_span() {
    let (t, errs) = ty_err(")");
    assert_eq!(t.kind, TypeExprKind::Error);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].span, Span::new(0, 1));
}

#[test]
fn unterminated_table_reports_but_recovers() {
    let (t, errs) = ty_err("{ x: integer");
    assert!(matches!(t.kind, TypeExprKind::Table(_)));
    assert!(!errs.is_empty());
}

#[test]
fn empty_and_trailing_pipe_types_report_a_missing_type() {
    let (t, errs) = ty_err("");
    assert_eq!(t.kind, TypeExprKind::Error);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].message, "expected a type");
    assert_eq!(errs[0].span, Span::empty(0));

    // A trailing `|` still yields a union whose last member is the error.
    let (t, errs) = ty_err("string|");
    let TypeExprKind::Union(m) = t.kind else {
        panic!("expected a union")
    };
    assert_eq!(named_of(&m[0]).0, "string");
    assert_eq!(m[1].kind, TypeExprKind::Error);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].message, "expected a type");
    assert_eq!(errs[0].span, Span::empty(7));
}

#[test]
fn leading_pipe_is_tolerated_without_an_error() {
    let TypeExprKind::Union(m) = ty("|string|nil").kind else {
        panic!("expected a union")
    };
    assert_eq!(m.len(), 2);
}

#[test]
fn lone_minus_reports_and_yields_an_error_node() {
    let (t, errs) = ty_err("-x");
    assert_eq!(t.kind, TypeExprKind::Error);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].message, "unexpected '-' in type");
    assert_eq!(errs[0].span, Span::new(0, 1));
}

#[test]
fn unclosed_bracketing_reports_one_error_per_construct() {
    for (src, message, ok) in [
        (
            "(string",
            "expected ')'",
            &(|t: &TypeExpr| matches!(t.kind, TypeExprKind::Paren(_)))
                as &dyn Fn(&TypeExpr) -> bool,
        ),
        (
            "[string, number",
            "expected ']' to close tuple",
            &|t| matches!(&t.kind, TypeExprKind::Tuple(items) if items.len() == 2),
        ),
        (
            "table<string, number",
            "expected '>' to close generics",
            &|t| matches!(&t.kind, TypeExprKind::Named { name, args } if name == "table" && args.len() == 2),
        ),
        (
            "fun(a: string",
            "expected ')' in fun type",
            &|t| matches!(&t.kind, TypeExprKind::Fun { params, .. } if params.len() == 1),
        ),
        (
            "fun(): (string",
            "expected ')' in return list",
            &|t| matches!(&t.kind, TypeExprKind::Fun { returns, .. } if returns.len() == 1),
        ),
    ] {
        let (t, errs) = ty_err(src);
        assert_eq!(
            errs.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(),
            [message],
            "for {src:?}"
        );
        assert!(ok(&t), "unexpected recovered shape for {src:?}: {t:?}");
        // The whole construct is still spanned from its opening token.
        assert_eq!(t.span.start, 0, "for {src:?}");
    }
}

#[test]
fn malformed_table_fields_report_and_keep_parsing() {
    // Indexer missing its `]`.
    let (t, errs) = ty_err("{[string: number}");
    let TypeExprKind::Table(fields) = &t.kind else {
        panic!("expected a table type")
    };
    assert!(matches!(fields[0], TableField::Indexer { .. }));
    assert_eq!(
        errs.iter().map(|e| e.message.as_str()).collect::<Vec<_>>(),
        ["expected ']' in table indexer"]
    );

    // A field that does not start with a name: the token is consumed so the
    // loop makes progress, and the following field still parses.
    let (t, errs) = ty_err("{ 1, ok: string }");
    let TypeExprKind::Table(fields) = &t.kind else {
        panic!("expected a table type")
    };
    assert_eq!(fields.len(), 1, "{fields:?}");
    let TableField::Named { name, .. } = &fields[0] else {
        panic!("expected a named field")
    };
    assert_eq!(name, "ok");
    assert_eq!(errs[0].message, "expected a field name in table type");
    assert_eq!(errs[0].span, Span::new(2, 3));

    // Missing `:` between field name and type.
    let (_, errs) = ty_err("{ foo string }");
    assert_eq!(errs[0].message, "expected ':' in table field");
}

#[test]
fn malformed_type_does_not_abort_the_block() {
    // First tag has a broken type; the second must still parse cleanly.
    let b = block("---@type )\n---@param good string ok");
    assert_eq!(b.tags.len(), 2);
    assert!(!b.errors.is_empty(), "the broken type must record an error");
    let Tag::Param(p) = &b.tags[1] else {
        panic!("second tag must still be a param")
    };
    assert_eq!(p.name, "good");
    assert_eq!(named_of(&p.ty).0, "string");
}

#[test]
fn tags_without_a_name_recover_with_an_empty_name() {
    let Tag::Class(c) = one_tag("---@class") else {
        panic!()
    };
    assert_eq!(c.name, "");
    assert!(c.parents.is_empty());

    // `:` is not a name character, so the name is empty and the parent list
    // still parses.
    let Tag::Class(c) = one_tag("---@class : Animal") else {
        panic!()
    };
    assert_eq!(c.name, "");
    assert_eq!(named_of(&c.parents[0]).0, "Animal");
}

#[test]
fn class_type_params_need_a_closing_angle_bracket() {
    // Unterminated `<`: the parameter list is dropped rather than guessed.
    let Tag::Class(c) = one_tag("---@class Foo<T") else {
        panic!()
    };
    assert_eq!(c.name, "Foo");
    assert!(c.params.is_empty());

    let Tag::Class(c) = one_tag("---@class Foo<T: Base, U>") else {
        panic!()
    };
    assert_eq!(c.params, ["T", "U"]);
}

#[test]
fn field_indexer_brackets_nest_and_must_balance() {
    // Nested `[` inside the key: the *matching* `]` closes the indexer.
    let Tag::Field(f) = one_tag("---@field [string[]] number") else {
        panic!()
    };
    let FieldKey::Indexer(key) = &f.key else {
        panic!("expected an indexer key")
    };
    assert!(matches!(key.kind, TypeExprKind::Array(_)));
    assert_eq!(named_of(&f.ty).0, "number");

    // Unbalanced: not an indexer at all, so it falls back to the name form.
    let b = block("---@field [string number");
    let Tag::Field(f) = &b.tags[0] else { panic!() };
    assert_eq!(f.key, FieldKey::Name(String::new()));
}

#[test]
fn crlf_line_endings_do_not_leak_into_tag_text() {
    let text = "---@class Windows\r\n---@field path string a path\r\n";
    let b = parse_block(text, 0);
    let Tag::Class(c) = &b.tags[0] else { panic!() };
    assert_eq!(c.name, "Windows");
    let Tag::Field(f) = &b.tags[1] else { panic!() };
    assert_eq!(f.desc.as_deref(), Some("a path"));
    // The tag span stops before the `\r`.
    assert_eq!(&text[c.span.start..c.span.end], "@class Windows");
}

// === Span correctness ===

#[test]
fn span_len_and_is_empty_track_the_byte_range() {
    let s = Span::new(3, 10);
    assert_eq!(s.len(), 7);
    assert!(!s.is_empty());
    assert_eq!(Span::empty(4).len(), 0);
    assert!(Span::empty(4).is_empty());
    // Degenerate (inverted) spans report zero length rather than underflow.
    let inverted = Span::new(9, 4);
    assert_eq!(inverted.len(), 0);
    assert!(inverted.is_empty());
}

#[test]
fn every_tag_kind_reports_the_span_of_its_own_line() {
    let lines = [
        "@class C",
        "@field f string",
        "@param p string",
        "@return string",
        "@type string",
        "@alias A string",
        "@generic T",
        "@overload fun()",
        "@cast x string",
        "@enum E",
        "@meta",
        "@operator add(C): C",
        "@vararg string",
        "@see other",
        "@deprecated",
        "@nodiscard",
        "@async",
        "@diagnostic disable",
        "@version 5.4",
        "@source a.lua",
        "@package",
        "@private",
        "@protected",
        "@madeup thing",
    ];
    let text = lines
        .iter()
        .map(|l| format!("---{l}"))
        .collect::<Vec<_>>()
        .join("\n");
    let b = parse_block(&text, 0);
    assert_eq!(b.tags.len(), lines.len(), "{:?}", b.tags);
    for (tag, line) in b.tags.iter().zip(lines) {
        let span = tag.span();
        assert_eq!(&text[span.start..span.end], line, "wrong span for {tag:?}");
    }
    // The last line is the forward-compatibility fallback.
    assert!(matches!(b.tags[lines.len() - 1], Tag::Unknown(_)));
}

#[test]
fn param_spans_are_file_absolute() {
    // Offsets within "---@param n string":
    //   '@' at 3, tag body ends at 18, type "string" at 12..18.
    let Tag::Param(p) = one_tag("---@param n string") else {
        panic!()
    };
    assert_eq!(p.span, Span::new(3, 18));
    assert_eq!(p.ty.span, Span::new(12, 18));
}

#[test]
fn block_offset_makes_spans_absolute() {
    let text = "  ---@type integer";
    // The block slice starts at the first '-', which is offset 2 here.
    let b = parse_block(&text[2..], 2);
    let Tag::Type(t) = &b.tags[0] else { panic!() };
    // "integer" sits at 2 + len("---@type ") = 2 + 9 = 11.
    assert_eq!(t.types[0].span, Span::new(11, 18));
}

// === Harvest ===

const FIXTURE: &str = "\
---@class Animal
---@field name string
local Animal = {}

---@param sound string the noise
---@return string
function Animal:speak(sound)
  return sound
end

local x = 1 ---@type integer

---@alias Color
---| '\"red\"'
---| '\"green\"'

-- a plain comment

---@type number
";

#[test]
fn harvest_attaches_blocks_to_statements() {
    let parse = lua_parse(FIXTURE, Dialect::Lua54);
    let items = harvest(&parse);
    assert_eq!(items.len(), 5, "expected five doc blocks");

    let target_text = |item: &AnnotatedItem| {
        item.target
            .map(|s| FIXTURE[s.start..s.end].to_string())
            .unwrap_or_default()
    };

    // Block 0: class + field -> the `local Animal = {}` statement.
    assert!(target_text(&items[0]).starts_with("local Animal"));
    assert_eq!(items[0].block.tags.len(), 2);

    // Block 1: param + return -> the function declaration.
    assert!(target_text(&items[1]).starts_with("function Animal:speak"));

    // Block 2: same-line trailing `---@type integer` -> its own line's local.
    assert_eq!(target_text(&items[2]), "local x = 1");
    assert!(matches!(items[2].block.tags[0], Tag::Type(_)));

    // Block 3: the multiline alias is detached (blank line + plain comment).
    assert!(items[3].target.is_none());
    let Tag::Alias(a) = &items[3].block.tags[0] else {
        panic!()
    };
    assert_eq!(a.members.len(), 2);

    // Block 4: trailing `---@type number` at EOF -> no target.
    assert!(items[4].target.is_none());
}

#[test]
fn harvest_empty_when_no_doc_comments() {
    let parse = lua_parse("local x = 1\n-- plain\nreturn x\n", Dialect::Lua54);
    assert!(harvest(&parse).is_empty());
}

#[test]
fn harvest_block_before_nested_statement() {
    let src = "function f()\n  ---@type integer\n  local y = 2\nend\n";
    let parse = lua_parse(src, Dialect::Lua54);
    let items = harvest(&parse);
    assert_eq!(items.len(), 1);
    let target = items[0].target.expect("nested local target");
    assert_eq!(&src[target.start..target.end], "local y = 2");
}

// === Inline `--[[@as T]]` casts ===

#[test]
fn harvest_inline_as_finds_long_bracket_casts() {
    let src =
        "local u = SOME_GLOBAL --[[@as number]]\nlocal v = other() --[==[@as string|nil]==]\n";
    let parse = lua_parse(src, Dialect::Lua54);
    let casts = harvest_inline_as(&parse);
    assert_eq!(casts.len(), 2);
    let (name, _) = named_of(&casts[0].ty);
    assert_eq!(name, "number");
    assert_eq!(
        &src[casts[0].span.start..casts[0].span.end],
        "--[[@as number]]"
    );
    assert!(matches!(casts[1].ty.kind, TypeExprKind::Union(_)));
}

#[test]
fn harvest_inline_as_ignores_other_comments() {
    let src = "\
local a = 1 -- @as number is a line comment, not a cast
--[[ plain block comment ]]
---@as doc-comment form is not the inline syntax
--[[@asymmetric]] local b = 2
local c = 3 --[[@as]]
";
    let parse = lua_parse(src, Dialect::Lua54);
    assert!(harvest_inline_as(&parse).is_empty());
}

// === Property test: no panic over arbitrary `---@` line soup ===

proptest! {
    #[test]
    fn parse_block_never_panics_and_spans_stay_in_bounds(
        lines in prop::collection::vec("---@?[ -~]{0,24}", 0..8),
    ) {
        let text = lines.join("\n");
        let b = parse_block(&text, 0);
        prop_assert_eq!(b.span, Span::new(0, text.len()));
        for tag in &b.tags {
            prop_assert!(tag.span().end <= text.len());
        }
        for err in &b.errors {
            prop_assert!(err.span.end <= text.len());
        }
    }

    #[test]
    fn harvest_never_panics_on_arbitrary_annotated_source(
        lines in prop::collection::vec("(---@?[ -~]{0,24}|local x = 1|function f\\(\\) end)", 0..10),
    ) {
        let src = format!("{}\n", lines.join("\n"));
        let parse = lua_parse(&src, Dialect::Lua54);
        let items = harvest(&parse);
        for item in &items {
            if let Some(t) = item.target {
                prop_assert!(t.end <= src.len());
            }
        }
    }

    #[test]
    fn arbitrary_string_never_panics_the_parser(s in ".*") {
        let _ = parse_block(&s, 0);
    }
}
