//! The canonical `.lb` formatter (SHAPES.md §8): 4-space indent, one item per
//! line, a single blank line between items, trailing commas on multi-line field
//! lists — and *no configuration*.
//!
//! Canonical shape choices (deterministic, no width heuristics):
//! - A braced body with no members prints inline as `{}`.
//! - A struct with one or more fields (or a `..` marker) always expands, one
//!   field per line, each terminated by a trailing comma.
//! - Trait-fn parameter lists stay on the signature line (they are short by
//!   construction), so the "trailing comma on multi-line param lists" rule is
//!   satisfied vacuously.
//! - Doc comments and comments attach to the item/field they precede.
//!
//! Contract: a formatter must never destroy code. [`format`] returns the input
//! **unchanged** if it does not parse cleanly, and also if formatting would drop
//! any comment (a conservative safety net).

use super::ast::{
    AstNode, Field, FnType, GenericArgs, GenericParams, Item, ParamList, ShapeFile, TraitFn,
    TypeRef,
};
use super::{ShapeSyntaxKind, ShapeSyntaxNode, parse};
use ShapeSyntaxKind::{COMMENT, DOC_COMMENT, FIELD, TRAIT_FN, WHITESPACE};
use rowan::NodeOrToken;

const INDENT: &str = "    ";

/// Format `.lb` source into its canonical form (SHAPES.md §8).
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

fn format_item(out: &mut String, item: &Item) {
    push_comments(out, &leading_comments(item.syntax()), "");
    match item {
        Item::Struct(s) => {
            let mut header = format!("struct {}", s.name().unwrap_or_default());
            if let Some(g) = s.generic_params() {
                header.push_str(&format_generic_params(&g));
            }
            let fields: Vec<Field> = s.fields().collect();
            let is_open = s.is_open();
            if fields.is_empty() && !is_open {
                out.push_str(&header);
                out.push_str(" {}\n");
                return;
            }
            out.push_str(&header);
            out.push_str(" {\n");
            for f in &fields {
                push_comments(out, &leading_comments(f.syntax()), INDENT);
                out.push_str(INDENT);
                out.push_str(&f.name().unwrap_or_default());
                out.push_str(": ");
                out.push_str(&format_type_opt(f.ty().as_ref()));
                out.push_str(",\n");
            }
            if is_open {
                out.push_str(INDENT);
                out.push_str("..\n");
            }
            push_comments(out, &trailing_comments(s.syntax(), FIELD), INDENT);
            out.push_str("}\n");
        }
        Item::Trait(t) => {
            let mut header = format!("trait {}", t.name().unwrap_or_default());
            if let Some(g) = t.generic_params() {
                header.push_str(&format_generic_params(&g));
            }
            let supers = t.supertraits();
            if !supers.is_empty() {
                header.push_str(": ");
                header.push_str(&supers.join(" + "));
            }
            let fns: Vec<TraitFn> = t.fns().collect();
            if fns.is_empty() {
                out.push_str(&header);
                out.push_str(" {}\n");
                return;
            }
            out.push_str(&header);
            out.push_str(" {\n");
            for f in &fns {
                push_comments(out, &leading_comments(f.syntax()), INDENT);
                out.push_str(INDENT);
                out.push_str(&format_trait_fn(f));
                out.push_str(";\n");
            }
            push_comments(out, &trailing_comments(t.syntax(), TRAIT_FN), INDENT);
            out.push_str("}\n");
        }
        Item::Impl(im) => {
            out.push_str("impl ");
            out.push_str(&im.trait_name().unwrap_or_default());
            if let Some(g) = im.generic_params() {
                out.push_str(&format_generic_params(&g));
            }
            out.push_str(" for ");
            out.push_str(&im.struct_name().unwrap_or_default());
            out.push_str(";\n");
        }
        Item::Alias(a) => {
            out.push_str("type ");
            out.push_str(&a.name().unwrap_or_default());
            if let Some(g) = a.generic_params() {
                out.push_str(&format_generic_params(&g));
            }
            out.push_str(" = ");
            out.push_str(&format_type_opt(a.ty().as_ref()));
            out.push_str(";\n");
        }
        Item::Use(u) => {
            out.push_str("use ");
            out.push_str(&u.path());
            out.push_str(";\n");
        }
    }
}

fn format_trait_fn(f: &TraitFn) -> String {
    let mut s = format!("fn {}(", f.name().unwrap_or_default());
    s.push_str(&format_params(f.params().as_ref()));
    s.push(')');
    let rets = f.returns();
    if !rets.is_empty() {
        s.push_str(" -> ");
        s.push_str(&join_types(&rets));
    }
    s
}

// --- types --------------------------------------------------------------

fn format_type_opt(ty: Option<&TypeRef>) -> String {
    ty.map_or_else(String::new, format_type)
}

fn format_type(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named(n) => {
            let mut s = n.name().unwrap_or_default();
            if let Some(args) = n.args() {
                s.push_str(&format_generic_args(&args));
            }
            s
        }
        TypeRef::Optional(o) => match o.inner() {
            Some(inner) => format!("{}?", format_type(&inner)),
            None => "?".to_string(),
        },
        TypeRef::Union(u) => {
            let members: Vec<TypeRef> = u.members().collect();
            members
                .iter()
                .map(format_type)
                .collect::<Vec<_>>()
                .join(" | ")
        }
        TypeRef::Fn(f) => format_fn_type(f),
        TypeRef::Paren(p) => match p.inner() {
            Some(inner) => format!("({})", format_type(&inner)),
            None => "()".to_string(),
        },
    }
}

fn format_fn_type(f: &FnType) -> String {
    let mut s = String::from("fn(");
    s.push_str(&format_params(f.params().as_ref()));
    s.push(')');
    let rets = f.returns();
    if !rets.is_empty() {
        s.push_str(" -> ");
        s.push_str(&join_types(&rets));
    }
    s
}

fn format_params(params: Option<&ParamList>) -> String {
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
                    "{}: {}",
                    p.name().unwrap_or_default(),
                    format_type_opt(p.ty().as_ref())
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_types(types: &[TypeRef]) -> String {
    types.iter().map(format_type).collect::<Vec<_>>().join(", ")
}

fn format_generic_args(args: &GenericArgs) -> String {
    let parts: Vec<TypeRef> = args.args().collect();
    format!("<{}>", join_types(&parts))
}

fn format_generic_params(params: &GenericParams) -> String {
    let parts: Vec<String> = params
        .params()
        .map(|p| {
            let mut s = p.name().unwrap_or_default();
            let bounds = p.bounds();
            if !bounds.is_empty() {
                s.push_str(": ");
                s.push_str(&bounds.join(" + "));
            }
            s
        })
        .collect();
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

/// Comment tokens that are direct children of `node` and appear *after* the last
/// member of `member_kind` (i.e. dangling before the closing brace).
fn trailing_comments(node: &ShapeSyntaxNode, member_kind: ShapeSyntaxKind) -> Vec<String> {
    let last_end = node
        .children()
        .filter(|c| c.kind() == member_kind)
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
    fn formats_messy_struct() {
        let src = "struct   Point{x:number,y:number,label:string?}";
        let expected = "\
struct Point {
    x: number,
    y: number,
    label: string?,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn empty_struct_stays_inline() {
        assert_eq!(format("struct  Empty  {  }"), "struct Empty {}\n");
    }

    #[test]
    fn open_marker_on_own_line() {
        let src = "struct Bag{n:number,..}";
        let expected = "\
struct Bag {
    n: number,
    ..
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn formats_trait_and_impl() {
        let src = "trait Drawable:Shape+Sized{fn draw(self,surface:Surface);fn id(self)->number;}\nimpl Shape for Circle;";
        let expected = "\
trait Drawable: Shape + Sized {
    fn draw(self, surface: Surface);
    fn id(self) -> number;
}

impl Shape for Circle;
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn formats_generics_and_types() {
        let src = "type Handler=fn(a:A,b:B)->R,E;\ntype M=HashMap<string,Point?>;\nstruct Pair<T:Hash+Eq,U>{a:T,b:U}";
        let expected = "\
type Handler = fn(a: A, b: B) -> R, E;

type M = HashMap<string, Point?>;

struct Pair<T: Hash + Eq, U> {
    a: T,
    b: U,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn single_blank_line_between_items() {
        let src = "use a;\n\n\n\nuse b.c;";
        assert_eq!(format(src), "use a;\n\nuse b.c;\n");
    }

    #[test]
    fn doc_comment_stays_with_item() {
        let src = "/// A point.\nstruct Point{x:number}";
        let expected = "\
/// A point.
struct Point {
    x: number,
}
";
        assert_eq!(format(src), expected);
    }

    #[test]
    fn field_doc_comment_preserved() {
        let src = "struct P{\n/// the x coord\nx:number,\ny:number}";
        let out = format(src);
        assert!(out.contains("/// the x coord"), "got: {out}");
        assert!(out.contains("    x: number,"), "got: {out}");
    }

    #[test]
    fn invalid_input_returned_unchanged() {
        let src = "struct { oops";
        assert_eq!(format(src), src);
    }

    #[test]
    fn idempotent_on_spec_example() {
        let src = include_str!("test_data/spec_example.lb");
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

    /// A bounded, always-parseable type expression.
    fn ty() -> impl Strategy<Value = String> {
        ident().prop_recursive(3, 12, 3, |inner| {
            prop_oneof![
                (ident(), prop::collection::vec(inner.clone(), 1..3))
                    .prop_map(|(n, args)| format!("{n}<{}>", args.join(", "))),
                inner.clone().prop_map(|t| format!("{t}?")),
                prop::collection::vec(inner, 2..3).prop_map(|m| m.join(" | ")),
            ]
        })
    }

    fn generic_params() -> impl Strategy<Value = String> {
        prop::collection::vec(
            (ident(), prop::collection::vec(ident(), 0..2)).prop_map(|(n, bounds)| {
                if bounds.is_empty() {
                    n
                } else {
                    format!("{n}: {}", bounds.join(" + "))
                }
            }),
            0..3,
        )
        .prop_map(|ps| {
            if ps.is_empty() {
                String::new()
            } else {
                format!("<{}>", ps.join(", "))
            }
        })
    }

    fn field() -> impl Strategy<Value = String> {
        (ident(), ty()).prop_map(|(n, t)| format!("{n}: {t}"))
    }

    fn struct_item() -> impl Strategy<Value = String> {
        (
            ident(),
            generic_params(),
            prop::collection::vec(field(), 0..4),
            any::<bool>(),
        )
            .prop_map(|(n, g, fields, open)| {
                let mut body = fields.join(", ");
                if open {
                    if body.is_empty() {
                        body.push_str("..");
                    } else {
                        body.push_str(", ..");
                    }
                }
                format!("struct {n}{g} {{ {body} }}")
            })
    }

    fn param() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("self".to_string()),
            (ident(), ty()).prop_map(|(n, t)| format!("{n}: {t}")),
        ]
    }

    fn trait_fn() -> impl Strategy<Value = String> {
        (
            ident(),
            prop::collection::vec(param(), 0..3),
            prop::collection::vec(ty(), 0..3),
        )
            .prop_map(|(n, params, rets)| {
                let ret = if rets.is_empty() {
                    String::new()
                } else {
                    format!(" -> {}", rets.join(", "))
                };
                format!("fn {n}({}){ret};", params.join(", "))
            })
    }

    fn trait_item() -> impl Strategy<Value = String> {
        (
            ident(),
            prop::collection::vec(ident(), 0..2),
            prop::collection::vec(trait_fn(), 0..3),
        )
            .prop_map(|(n, supers, fns)| {
                let sup = if supers.is_empty() {
                    String::new()
                } else {
                    format!(": {}", supers.join(" + "))
                };
                format!("trait {n}{sup} {{ {} }}", fns.join(" "))
            })
    }

    fn item() -> impl Strategy<Value = String> {
        prop_oneof![
            struct_item(),
            trait_item(),
            (ident(), ident()).prop_map(|(a, b)| format!("impl {a} for {b};")),
            (ident(), generic_params(), ty()).prop_map(|(n, g, t)| format!("type {n}{g} = {t};")),
            prop::collection::vec(ident(), 1..3).prop_map(|p| format!("use {};", p.join("."))),
        ]
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
