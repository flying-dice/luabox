//! The canonical `.luab` formatter (SHAPES-V2.md): 4-space indent, one item
//! per declaration, a single blank line between items, trailing commas on
//! expanded member lists — and *no configuration*.
//!
//! Canonical shape choices (deterministic, no width heuristics):
//! - An object type with no members prints inline as `{}`.
//! - An object type with members always expands, one member per line, each
//!   terminated by a trailing comma. Nested objects expand at nested indent.
//! - Union/intersection members join inline (`A | B`, `Shape & { ... }`) —
//!   an expanded object opens its brace on the same line.
//! - Method parameter lists stay on the member line (they are short by
//!   construction).
//! - Doc comments and comments attach to the item/member they precede.
//!
//! Contract: a formatter must never destroy code. [`format`] returns the input
//! **unchanged** if it does not parse cleanly, and also if formatting would drop
//! any comment (a conservative safety net).

use super::ast::{
    AstNode, GenericArgs, GenericParams, Member, ObjectType, ParamList, ShapeFile, TypeDef, TypeRef,
};
use super::{ShapeSyntaxKind, ShapeSyntaxNode, parse};
use ShapeSyntaxKind::{COMMENT, DOC_COMMENT, FIELD, METHOD, WHITESPACE};
use rowan::NodeOrToken;

const INDENT: &str = "    ";

/// Format `.luab` source into its canonical form (SHAPES-V2.md).
///
/// Returns the input unchanged when it does not parse cleanly, or when
/// reformatting would drop a comment — a formatter never destroys code.
#[must_use]
pub fn format(text: &str) -> String {
    let parsed = parse(text);
    if !parsed.errors().is_empty() {
        return text.to_string();
    }
    let root = parsed.syntax();
    let Some(file) = ShapeFile::cast(root.clone()) else {
        return text.to_string();
    };

    let mut out = String::new();
    for (i, item) in file.items().enumerate() {
        if i > 0 {
            out.push('\n'); // single blank line between items
        }
        format_item(&mut out, &item);
    }

    if !all_comments_preserved(&root, &out) {
        return text.to_string();
    }
    out
}

// --- items --------------------------------------------------------------

fn format_item(out: &mut String, item: &TypeDef) {
    push_comments(out, &leading_comments(item.syntax()), "");
    if item.is_export() {
        out.push_str("export ");
    }
    out.push_str("type ");
    out.push_str(&item.name().unwrap_or_default());
    if let Some(g) = item.generic_params() {
        out.push_str(&format_generic_params(&g));
    }
    out.push_str(" = ");
    out.push_str(&format_type_opt(item.ty().as_ref(), 0));
    out.push('\n');
}

// --- types --------------------------------------------------------------

fn format_type_opt(ty: Option<&TypeRef>, depth: usize) -> String {
    ty.map_or_else(String::new, |t| format_type(t, depth))
}

/// Render a type expression. `depth` is the indent level the expression
/// starts at; expanded object members print at `depth + 1`.
fn format_type(ty: &TypeRef, depth: usize) -> String {
    match ty {
        TypeRef::Named(n) => {
            let mut s = n.path();
            if let Some(args) = n.args() {
                s.push_str(&format_generic_args(&args, depth));
            }
            s
        }
        TypeRef::Object(o) => format_object(o, depth),
        TypeRef::Optional(o) => match o.inner() {
            Some(inner) => format!("{}?", format_type(&inner, depth)),
            None => "?".to_string(),
        },
        TypeRef::Union(u) => u
            .members()
            .map(|m| format_type(&m, depth))
            .collect::<Vec<_>>()
            .join(" | "),
        TypeRef::Intersection(i) => i
            .members()
            .map(|m| format_type(&m, depth))
            .collect::<Vec<_>>()
            .join(" & "),
        TypeRef::Fn(f) => {
            let mut s = format!("({})", format_params(f.params().as_ref(), depth));
            s.push_str(" => ");
            s.push_str(&format_type_opt(f.ret().as_ref(), depth));
            s
        }
        TypeRef::Paren(p) => {
            let inners: Vec<TypeRef> = p.inners().collect();
            format!(
                "({})",
                inners
                    .iter()
                    .map(|t| format_type(t, depth))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

fn format_object(obj: &ObjectType, depth: usize) -> String {
    let members: Vec<Member> = obj.members().collect();
    if members.is_empty() {
        return "{}".to_string();
    }
    let inner_indent = INDENT.repeat(depth + 1);
    let close_indent = INDENT.repeat(depth);
    let mut s = String::from("{\n");
    for m in &members {
        for c in leading_comments(m.syntax()) {
            s.push_str(&inner_indent);
            s.push_str(c.trim_end());
            s.push('\n');
        }
        s.push_str(&inner_indent);
        match m {
            Member::Field(f) => {
                s.push_str(&f.name().unwrap_or_default());
                if f.optional() {
                    s.push('?');
                }
                s.push_str(": ");
                s.push_str(&format_type_opt(f.ty().as_ref(), depth + 1));
            }
            Member::Method(f) => {
                s.push_str(&f.name().unwrap_or_default());
                s.push('(');
                s.push_str(&format_params(f.params().as_ref(), depth + 1));
                s.push(')');
                if let Some(ret) = f.ret() {
                    s.push_str(": ");
                    s.push_str(&format_type(&ret, depth + 1));
                }
            }
        }
        s.push_str(",\n");
    }
    for c in trailing_member_comments(obj.syntax()) {
        s.push_str(&inner_indent);
        s.push_str(c.trim_end());
        s.push('\n');
    }
    s.push_str(&close_indent);
    s.push('}');
    s
}

fn format_params(params: Option<&ParamList>, depth: usize) -> String {
    let Some(params) = params else {
        return String::new();
    };
    params
        .params()
        .map(|p| {
            if p.is_self() {
                "self".to_string()
            } else {
                format!(
                    "{}{}: {}",
                    p.name().unwrap_or_default(),
                    if p.optional() { "?" } else { "" },
                    format_type_opt(p.ty().as_ref(), depth)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_generic_args(args: &GenericArgs, depth: usize) -> String {
    let parts: Vec<String> = args.args().map(|t| format_type(&t, depth)).collect();
    format!("<{}>", parts.join(", "))
}

fn format_generic_params(params: &GenericParams) -> String {
    let parts: Vec<String> = params.params().filter_map(|p| p.name()).collect();
    format!("<{}>", parts.join(", "))
}

// --- comments -----------------------------------------------------------

fn push_comments(out: &mut String, comments: &[String], indent: &str) {
    for c in comments {
        out.push_str(indent);
        out.push_str(c.trim_end());
        out.push('\n');
    }
}

/// Comment tokens preceding the first significant token of `node`.
fn leading_comments(node: &ShapeSyntaxNode) -> Vec<String> {
    let mut out = Vec::new();
    for el in node.children_with_tokens() {
        match el {
            NodeOrToken::Token(t) => match t.kind() {
                COMMENT | DOC_COMMENT => out.push(t.text().to_string()),
                WHITESPACE => {}
                _ => break,
            },
            NodeOrToken::Node(_) => break,
        }
    }
    out
}

/// Comment tokens that are direct children of an object node and appear
/// *after* its last member (i.e. dangling before the closing brace).
fn trailing_member_comments(node: &ShapeSyntaxNode) -> Vec<String> {
    let last_end = node
        .children()
        .filter(|c| matches!(c.kind(), FIELD | METHOD))
        .map(|c| c.text_range().end())
        .max();
    let Some(last_end) = last_end else {
        return Vec::new();
    };
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| matches!(t.kind(), COMMENT | DOC_COMMENT) && t.text_range().start() >= last_end)
        .map(|t| t.text().to_string())
        .collect()
}

/// Conservative safety net: every comment in the tree must survive into `out`.
fn all_comments_preserved(root: &ShapeSyntaxNode, out: &str) -> bool {
    root.descendants_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| matches!(t.kind(), COMMENT | DOC_COMMENT))
        .all(|t| out.contains(t.text().trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_messy_object() {
        let src = "type   Point={x:number,y:number,label?:string}";
        let expected = "\
type Point = {
    x: number,
    y: number,
    label?: string,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn empty_object_stays_inline() {
        assert_eq!(format("type  Empty  =  {  }"), "type Empty = {}\n");
    }

    #[test]
    fn formats_methods_and_intersection() {
        let src = "export type Drawable=Shape&{draw(self,surface:Surface),id(self):number}";
        let expected = "\
export type Drawable = Shape & {
    draw(self, surface: Surface),
    id(self): number,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn formats_generics_aliases_and_fn_types() {
        let src = "type Handler=(a:A,b:B)=>R\ntype M=Map<string,Point?>\ntype Pair<T,U>={a:T,b:U}";
        let expected = "\
type Handler = (a: A, b: B) => R

type M = Map<string, Point?>

type Pair<T, U> = {
    a: T,
    b: U,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn qualified_reexport_on_one_line() {
        assert_eq!(
            format("export   type Canvas=love.graphics.Canvas"),
            "export type Canvas = love.graphics.Canvas\n"
        );
    }

    #[test]
    fn nested_object_expands_at_nested_indent() {
        let src = "type A = { inner: { x: number } }";
        let expected = "\
type A = {
    inner: {
        x: number,
    },
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn single_blank_line_between_items() {
        let src = "type A = number\n\n\n\ntype B = string";
        assert_eq!(format(src), "type A = number\n\ntype B = string\n");
    }

    #[test]
    fn doc_comment_stays_with_item() {
        let src = "--- A point.\ntype Point={x:number}";
        let expected = "\
--- A point.
type Point = {
    x: number,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn member_doc_comment_preserved() {
        let src = "type P={\n--- the x coord\nx:number,\ny:number}";
        let out = format(src);
        assert!(out.contains("--- the x coord"), "got: {out}");
        assert!(out.contains("    x: number,"), "got: {out}");
    }

    #[test]
    fn invalid_input_returned_unchanged() {
        let src = "type = { oops";
        assert_eq!(format(src), src);
    }

    #[test]
    fn idempotent_on_spec_example() {
        let src = include_str!("test_data/spec_example.luab");
        let once = format(src);
        assert_eq!(format(&once), once, "format must be idempotent");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::shape::parse;
    use proptest::prelude::*;

    fn ident() -> impl Strategy<Value = String> {
        prop::sample::select(vec![
            "A", "B", "C", "Foo", "Bar", "Point", "number", "string", "boolean", "T", "U", "V",
        ])
        .prop_map(String::from)
    }

    fn qualified() -> impl Strategy<Value = String> {
        prop::collection::vec(ident(), 1..3).prop_map(|p| p.join("."))
    }

    /// A bounded, always-parseable type expression.
    fn ty() -> impl Strategy<Value = String> {
        qualified().prop_recursive(3, 12, 3, |inner| {
            prop_oneof![
                (ident(), prop::collection::vec(inner.clone(), 1..3))
                    .prop_map(|(n, args)| format!("{n}<{}>", args.join(", "))),
                inner.clone().prop_map(|t| format!("{t}?")),
                prop::collection::vec(inner.clone(), 2..3).prop_map(|m| m.join(" | ")),
                prop::collection::vec(inner, 2..3).prop_map(|m| m.join(" & ")),
            ]
        })
    }

    fn generic_params() -> impl Strategy<Value = String> {
        prop::collection::vec(ident(), 0..3).prop_map(|ps| {
            if ps.is_empty() {
                String::new()
            } else {
                format!("<{}>", ps.join(", "))
            }
        })
    }

    fn field() -> impl Strategy<Value = String> {
        (ident(), any::<bool>(), ty())
            .prop_map(|(n, opt, t)| format!("{n}{}: {t}", if opt { "?" } else { "" }))
    }

    fn param() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("self".to_string()),
            (ident(), ty()).prop_map(|(n, t)| format!("{n}: {t}")),
        ]
    }

    fn method() -> impl Strategy<Value = String> {
        (
            ident(),
            prop::collection::vec(param(), 0..3),
            prop::option::of(ty()),
        )
            .prop_map(|(n, params, ret)| {
                let ret = ret.map(|r| format!(": {r}")).unwrap_or_default();
                format!("{n}({}){ret}", params.join(", "))
            })
    }

    fn object() -> impl Strategy<Value = String> {
        prop::collection::vec(prop_oneof![field(), method()], 0..4)
            .prop_map(|ms| format!("{{ {} }}", ms.join(", ")))
    }

    fn item() -> impl Strategy<Value = String> {
        (
            any::<bool>(),
            ident(),
            generic_params(),
            prop_oneof![
                ty(),
                object(),
                (ty(), object()).prop_map(|(t, o)| format!("{t} & {o}"))
            ],
        )
            .prop_map(|(exp, n, g, rhs)| {
                format!("{}type {n}{g} = {rhs}", if exp { "export " } else { "" })
            })
    }

    fn shape_file() -> impl Strategy<Value = String> {
        prop::collection::vec(item(), 1..6).prop_map(|items| items.join("\n\n"))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(400))]

        /// Every generated file is valid, and formatting it is idempotent and
        /// itself re-parses cleanly (`format(format(x)) == format(x)`).
        #[test]
        fn fmt_is_idempotent(src in shape_file()) {
            prop_assert!(
                parse(&src).errors().is_empty(),
                "generator produced invalid source: {:?} -> {:?}",
                src,
                parse(&src).errors()
            );
            let once = format(&src);
            prop_assert!(parse(&once).errors().is_empty(), "formatted output must re-parse: {once:?}");
            prop_assert_eq!(format(&once), once);
        }
    }
}
