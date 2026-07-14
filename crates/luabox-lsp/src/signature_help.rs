//! Signature help: while the cursor sits inside a call's argument list, the
//! resolved signature(s) of the callee — parameter names/types/docs and the
//! active parameter index — reusing the same callee resolution as
//! `hover`/`completion` (`functions()` for bare/dotted names,
//! `class_of_name`/`class_fields` for `recv:method()`/`recv.field()`).
//!
//! The innermost call whose argument list contains the cursor wins (so a
//! nested `f(g(x, |))` shows `g`'s signature, not `f`'s). If that call's
//! callee cannot be resolved to a signature, the result is `None` rather
//! than falling back to an enclosing call's signature.

use lsp_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, SignatureHelp,
    SignatureInformation,
};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::SyntaxToken;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::luacats::FieldKey;
use rowan::TextRange;

use crate::sema::{self, FileSema, SigParam};

/// One resolved, renderable signature: the callable's display name (used as
/// the label's prefix) plus its parameters, return types, and overall doc.
struct Signature {
    name: String,
    params: Vec<SigParam>,
    returns: Vec<luabox_syntax::luacats::TypeExpr>,
    doc: String,
}

/// The enclosing call at a cursor offset: its resolved signature(s) and the
/// raw (unclamped) active-parameter index.
struct CallSite {
    signatures: Vec<Signature>,
    active_param: usize,
}

/// Compute signature help at a byte `offset`.
#[must_use]
pub fn signature_help(sema: &FileSema, offset: usize) -> Option<SignatureHelp> {
    let call = enclosing_call(sema, offset)?;
    if call.signatures.is_empty() {
        return None;
    }
    let raw = call.active_param;
    // Best-fit overload: the first one with enough parameter slots for the
    // args typed so far (a trailing vararg always has room); otherwise the
    // last (most-parameters) overload.
    let active_signature = call
        .signatures
        .iter()
        .position(|s| raw < s.params.len() || s.params.last().is_some_and(|p| p.vararg))
        .unwrap_or(call.signatures.len() - 1);

    let signatures: Vec<SignatureInformation> = call
        .signatures
        .iter()
        .map(|s| render_signature(s, raw))
        .collect();
    let active_parameter = signatures[active_signature].active_parameter;
    Some(SignatureHelp {
        signatures,
        active_signature: Some(u32::try_from(active_signature).unwrap_or(0)),
        active_parameter,
    })
}

/// A callee reference deferred until we know its call is the innermost
/// match, so an outer/unresolvable candidate never pays for resolution.
enum Callee {
    /// The callee expression of a `f(...)` call (a bare name or `M.field`).
    Call(ast::Expr),
    /// The receiver and method name of a `recv:m(...)` call.
    Method(Option<ast::Expr>, SyntaxToken),
}

/// The innermost call whose argument list contains `offset`, resolved to its
/// signature(s). Walks every call in the file rather than the ancestor chain
/// (mirrors [`sema::FileSema::item_covering`]'s "innermost by narrowest
/// range" approach), which is simpler than reasoning about trivia/ancestor
/// boundaries and is cheap at file scale.
fn enclosing_call(sema: &FileSema, offset: usize) -> Option<CallSite> {
    let mut best: Option<(TextRange, ast::ArgList, Callee)> = None;
    for node in sema.root.descendants() {
        let (args, callee) = match node.kind() {
            SyntaxKind::CALL_EXPR => {
                let Some(call) = ast::CallExpr::cast(node) else {
                    continue;
                };
                let Some(args) = call.args() else { continue };
                if !active_arg_list(&args, offset) {
                    continue;
                }
                let Some(callee) = call.callee() else {
                    continue;
                };
                (args, Callee::Call(callee))
            }
            SyntaxKind::METHOD_CALL_EXPR => {
                let Some(call) = ast::MethodCallExpr::cast(node) else {
                    continue;
                };
                let Some(args) = call.args() else { continue };
                if !active_arg_list(&args, offset) {
                    continue;
                }
                let Some(member) = call.method_name() else {
                    continue;
                };
                (args, Callee::Method(call.receiver(), member))
            }
            _ => continue,
        };
        let range = args.syntax().text_range();
        if best
            .as_ref()
            .is_none_or(|(best_range, ..)| range.len() < best_range.len())
        {
            best = Some((range, args, callee));
        }
    }
    let (_, args, callee) = best?;
    let signatures = match callee {
        Callee::Call(callee) => resolve_call_signatures(sema, &callee)?,
        Callee::Method(receiver, member) => resolve_method_signatures(sema, receiver, &member)?,
    };
    Some(CallSite {
        signatures,
        active_param: active_param_index(&args, offset),
    })
}

/// Whether `offset` sits inside `list`'s parenthesized arguments: only
/// `f(...)` calls have positional structure worth showing (`f{...}` table-arg
/// and `f "s"` string-arg calls do not, so they never match). While the
/// closing `)` hasn't been parsed yet (the user is mid-call), any offset at
/// or past the node's own end still counts, since typing continues there.
fn active_arg_list(list: &ast::ArgList, offset: usize) -> bool {
    let tokens: Vec<_> = list
        .syntax()
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| !t.kind().is_trivia())
        .collect();
    if tokens
        .first()
        .is_none_or(|t| t.kind() != SyntaxKind::L_PAREN)
    {
        return false;
    }
    let closed = tokens
        .last()
        .is_some_and(|t| t.kind() == SyntaxKind::R_PAREN);
    let range = list.syntax().text_range();
    let (start, end) = (usize::from(range.start()), usize::from(range.end()));
    if closed {
        start < offset && offset < end
    } else {
        start < offset && offset <= end
    }
}

/// The raw (unclamped) active-parameter index: the count of top-level commas
/// in the argument list before `offset` — commas belonging to a nested call,
/// table, or parenthesised expression live under a deeper node and are not
/// direct token children of this `ExprList`, so they are never counted.
fn active_param_index(args: &ast::ArgList, offset: usize) -> usize {
    let Some(list) = args.expr_list() else {
        return 0;
    };
    list.syntax()
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::COMMA && usize::from(t.text_range().start()) < offset)
        .count()
}

/// Resolve a `f(...)` call's callee: a bare name via `functions()`, or a
/// `recv.field(...)` via the receiver's class fields (falling back to a
/// dotted function name, `M.helper`) — mirrors `hover::member_hover`.
fn resolve_call_signatures(sema: &FileSema, callee: &ast::Expr) -> Option<Vec<Signature>> {
    match callee {
        ast::Expr::Name(name_expr) => {
            let token = name_expr.name()?;
            signatures_from_functions(sema, token.text())
        }
        ast::Expr::Field(field) => {
            let member = field.field_name()?;
            let Some(ast::Expr::Name(recv)) = field.base() else {
                return None;
            };
            let recv_token = recv.name()?;
            let offset = usize::from(recv_token.text_range().start());
            if let Some(class) = sema.class_of_name(recv_token.text(), offset)
                && let Some(sig) = signature_from_class_field(sema, &class, member.text(), '.')
            {
                return Some(vec![sig]);
            }
            let dotted = format!("{}.{}", recv_token.text(), member.text());
            signatures_from_functions(sema, &dotted)
        }
        _ => None,
    }
}

/// Resolve a `recv:m(...)` call: the receiver's class must be known, and the
/// class must declare `m` as a `fun(...)`-typed field (`---@field m
/// fun(...)`) — mirrors `hover::member_hover`'s class-field lookup.
fn resolve_method_signatures(
    sema: &FileSema,
    receiver: Option<ast::Expr>,
    member: &SyntaxToken,
) -> Option<Vec<Signature>> {
    let Some(ast::Expr::Name(recv)) = receiver else {
        return None;
    };
    let recv_token = recv.name()?;
    let offset = usize::from(recv_token.text_range().start());
    let class = sema.class_of_name(recv_token.text(), offset)?;
    signature_from_class_field(sema, &class, member.text(), ':').map(|s| vec![s])
}

/// The primary signature plus any `---@overload`s for a `functions()`-visible
/// declaration named `name` (bare `f`, dotted `M.helper`, or `Class:method`).
fn signatures_from_functions(sema: &FileSema, name: &str) -> Option<Vec<Signature>> {
    let info = sema.functions().into_iter().find(|f| f.name == name)?;
    let sema::FnDecl {
        name,
        docs,
        params,
        returns,
        overloads,
        ..
    } = info;
    let mut signatures = vec![Signature {
        name: name.clone(),
        params,
        returns,
        doc: docs.clone(),
    }];
    signatures.extend(overloads.into_iter().map(|(params, returns)| Signature {
        name: name.clone(),
        params,
        returns,
        doc: docs.clone(),
    }));
    Some(signatures)
}

/// A signature built from a class's `---@field member fun(...)` — `sep` is
/// `.`/`:` to match how the call site spells the access.
fn signature_from_class_field(
    sema: &FileSema,
    class: &str,
    member: &str,
    sep: char,
) -> Option<Signature> {
    let (field, declaring) = sema
        .class_fields(class)
        .into_iter()
        .find(|(f, _)| matches!(&f.key, FieldKey::Name(n) if n == member))?;
    let (params, returns) = sema::as_function_type(&field.ty)?;
    Some(Signature {
        name: format!("{declaring}{sep}{member}"),
        params: params.iter().map(sema::fun_param_to_sig).collect(),
        returns: returns.iter().map(|r| r.ty.clone()).collect(),
        doc: field.desc.clone().unwrap_or_default(),
    })
}

/// Render one [`Signature`] to a [`SignatureInformation`]: the label (name,
/// rendered params, rendered returns), per-parameter [`ParameterInformation`]
/// (label as a UTF-16 offset range into the label, doc where `---@param`
/// carried one), and the active parameter clamped to this signature's own
/// arity (a vararg tail always has room).
fn render_signature(sig: &Signature, raw_active: usize) -> SignatureInformation {
    let mut label = format!("{}(", sig.name);
    let mut parameters = Vec::with_capacity(sig.params.len());
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = utf16_len(&label);
        label.push_str(&render_param(p));
        let end = utf16_len(&label);
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: p.doc.as_deref().map(markdown),
        });
    }
    label.push(')');
    if !sig.returns.is_empty() {
        label.push_str(": ");
        label.push_str(
            &sig.returns
                .iter()
                .map(sema::render_type)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    let active_parameter = if sig.params.is_empty() {
        0
    } else {
        raw_active.min(sig.params.len() - 1)
    };
    SignatureInformation {
        label,
        documentation: (!sig.doc.is_empty()).then(|| markdown(&sig.doc)),
        parameters: (!parameters.is_empty()).then_some(parameters),
        active_parameter: Some(u32::try_from(active_parameter).unwrap_or(0)),
    }
}

/// Render one parameter as it appears in a signature label: `name: type`,
/// `name?: type` when optional, `...: type` for a typed vararg, or the bare
/// name/`...` when no `---@param`/`fun(...)` type is attached.
fn render_param(p: &SigParam) -> String {
    let q = if p.optional { "?" } else { "" };
    match &p.ty {
        Some(ty) => format!("{}{q}: {}", p.name, sema::render_type(ty)),
        None => format!("{}{q}", p.name),
    }
}

fn markdown(value: &str) -> Documentation {
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: value.to_string(),
    })
}

/// The UTF-16 length of `s` — [`ParameterLabel::LabelOffsets`] are counted in
/// UTF-16 code units, the same encoding as every other LSP position.
fn utf16_len(s: &str) -> u32 {
    u32::try_from(s.encode_utf16().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    use super::*;

    fn analyze(text: &str) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let path = Path::new(if cfg!(windows) {
            r"C:\ws\main.lua"
        } else {
            "/ws/main.lua"
        })
        .to_path_buf();
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: text.to_string(),
        });
        (host.snapshot(), path)
    }

    fn labels(help: &SignatureHelp) -> Vec<&str> {
        help.signatures.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn shows_params_and_advances_active_parameter_across_commas() {
        let src = "\
---@param a number
---@param b string
local function f(a, b) end
f(1, 2)
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // Cursor right after `f(` on the last line.
        let open = src.rfind("f(1, 2)").unwrap() + "f(".len();
        let help = signature_help(&sema, open).expect("signature help");
        assert_eq!(labels(&help), vec!["f(a: number, b: string)"]);
        assert_eq!(help.active_parameter, Some(0));

        // Cursor right after the comma: the active parameter advances to 1.
        let after_comma = src.rfind("f(1, 2)").unwrap() + "f(1,".len();
        let help = signature_help(&sema, after_comma).expect("signature help");
        assert_eq!(help.active_parameter, Some(1));
    }

    #[test]
    fn clamps_active_parameter_to_the_last_declared_one() {
        let src = "\
---@param a number
local function f(a) end
f(1, 2, 3)
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // Cursor after the third argument: only one parameter is declared.
        let offset = src.rfind("f(1, 2, 3)").unwrap() + "f(1, 2, ".len();
        let help = signature_help(&sema, offset).expect("signature help");
        assert_eq!(help.active_parameter, Some(0));
    }

    #[test]
    fn method_call_resolves_via_class_fields() {
        let src = "\
---@class Point
---@field translate fun(dx: number, dy: number): Point

---@type Point
local p = nil
p:translate(1, 2)
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.rfind("translate(1, 2)").unwrap() + "translate(".len();
        let help = signature_help(&sema, offset).expect("signature help");
        assert_eq!(
            labels(&help),
            vec!["Point:translate(dx: number, dy: number): Point"]
        );
        assert_eq!(help.active_parameter, Some(0));
    }

    #[test]
    fn overloaded_function_returns_every_signature() {
        let src = "\
---@param a number
---@overload fun(a: string): boolean
local function f(a) end
f(1)
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.rfind("f(1)").unwrap() + "f(".len();
        let help = signature_help(&sema, offset).expect("signature help");
        assert_eq!(labels(&help), vec!["f(a: number)", "f(a: string): boolean"]);
    }

    #[test]
    fn cursor_outside_any_call_is_none() {
        let src = "local x = 1\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(signature_help(&sema, 8).is_none());
    }

    #[test]
    fn cursor_on_an_unresolvable_callee_is_none() {
        let src = "unknown_global(1, 2)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.find('(').unwrap() + 1;
        assert!(signature_help(&sema, offset).is_none());
    }

    #[test]
    fn nested_call_shows_the_innermost_signature() {
        let src = "\
---@param a number
local function f(a) end
---@param b string
local function g(b) end
f(g(1))
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // Cursor inside `g(...)`, nested within `f(...)`.
        let offset = src.rfind("g(1)").unwrap() + "g(".len();
        let help = signature_help(&sema, offset).expect("signature help");
        assert_eq!(labels(&help), vec!["g(b: string)"]);
    }
}
