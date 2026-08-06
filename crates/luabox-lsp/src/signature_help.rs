//! Signature help: while the cursor sits inside a call's argument list, the
//! resolved signature(s) of the callee — parameter names/types/docs and the
//! active parameter index — reusing the same callee resolution as
//! `hover`/`completion` (`functions()` for bare/dotted names,
//! [`crate::requires::receiver_type`] +
//! [`luabox_types::Ambient::class_members_of`] for
//! `recv:method()`/`recv.field()`, #56/#48).
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
use luabox_syntax::luacats::TypeExpr;
use luabox_types::ty::Ty;
use rowan::TextRange;

use crate::merged_ambient::MergedAmbient;
use crate::requires::{self, RequireExports};
use crate::sema::{self, FileSema, SigParam};

/// One resolved, renderable signature: the callable's display name (used as
/// the label's prefix) plus its parameters, return types, and overall doc.
///
/// Parameters and returns are pre-rendered to strings at construction,
/// rather than kept as [`luabox_syntax::luacats::TypeExpr`], because the
/// class-field route resolves through the workspace ambient (#56) and so has
/// an already-lowered [`luabox_types::ty::Ty`] to render, not a `TypeExpr` —
/// one `Signature` shape serves both producers.
struct Signature {
    name: String,
    params: Vec<RenderedParam>,
    returns: Vec<String>,
    doc: String,
}

/// One parameter, already rendered to its label text (`name: type`,
/// `name?: type`, or the bare name/`...` with none) — see [`Signature`].
struct RenderedParam {
    label: String,
    vararg: bool,
    doc: Option<String>,
}

/// The enclosing call at a cursor offset: its resolved signature(s) and the
/// raw (unclamped) active-parameter index.
struct CallSite {
    signatures: Vec<Signature>,
    active_param: usize,
}

/// Compute signature help at a byte `offset`. `exports` and `ambient` are the
/// same shared `require` resolution and merged workspace environment hover
/// and completion read (#54, #56), so a method call resolves a signature
/// exactly when hover/completion resolve the same receiver's members — #50
/// was this surface silently disagreeing with the other two because it had
/// neither wired in.
#[must_use]
pub fn signature_help(
    sema: &FileSema,
    offset: usize,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Option<SignatureHelp> {
    let call = enclosing_call(sema, offset, exports, ambient)?;
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
fn enclosing_call(
    sema: &FileSema,
    offset: usize,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Option<CallSite> {
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
        Callee::Call(callee) => resolve_call_signatures(sema, &callee, exports, ambient)?,
        Callee::Method(receiver, member) => {
            resolve_method_signatures(sema, receiver, &member, exports, ambient)?
        }
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
/// `recv.field(...)` via the receiver's class members resolved through the
/// workspace ambient (#56, falling back to a dotted function name,
/// `M.helper`) — mirrors `hover::member_hover`.
fn resolve_call_signatures(
    sema: &FileSema,
    callee: &ast::Expr,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Option<Vec<Signature>> {
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
            if let Some(binding) = sema.visible_binding_named(recv_token.text(), offset)
                && let Some(ty) = requires::receiver_type(sema, exports, binding)
                && let Some(sig) = signature_from_class_field(ambient, &ty, member.text(), '.')
            {
                return Some(vec![sig]);
            }
            let dotted = format!("{}.{}", recv_token.text(), member.text());
            signatures_from_functions(sema, &dotted)
        }
        _ => None,
    }
}

/// Resolve a `recv:m(...)` call: the receiver's class must be known —
/// resolved through the workspace ambient (#56), declared in this file or
/// any other — and the class must declare `m` as a `fun(...)`-typed field
/// (`---@field m fun(...)`) — mirrors `hover::member_hover`'s class-field
/// lookup.
fn resolve_method_signatures(
    sema: &FileSema,
    receiver: Option<ast::Expr>,
    member: &SyntaxToken,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Option<Vec<Signature>> {
    let Some(ast::Expr::Name(recv)) = receiver else {
        return None;
    };
    let recv_token = recv.name()?;
    let offset = usize::from(recv_token.text_range().start());
    let binding = sema.visible_binding_named(recv_token.text(), offset)?;
    let ty = requires::receiver_type(sema, exports, binding)?;
    signature_from_class_field(ambient, &ty, member.text(), ':').map(|s| vec![s])
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
    let render_params = |params: Vec<SigParam>| params.iter().map(render_sig_param).collect();
    let render_returns = |returns: Vec<luabox_syntax::luacats::TypeExpr>| {
        returns.iter().map(sema::render_type).collect()
    };
    let mut signatures = vec![Signature {
        name: name.clone(),
        params: render_params(params),
        returns: render_returns(returns),
        doc: docs.clone(),
    }];
    signatures.extend(overloads.into_iter().map(|(params, returns)| Signature {
        name: name.clone(),
        params: render_params(params),
        returns: render_returns(returns),
        doc: docs.clone(),
    }));
    Some(signatures)
}

/// A signature built from a class reference's `---@field member fun(...)` —
/// resolved through the workspace ambient (#56), the same merged surface
/// hover and completion read, so signature help cannot answer for a
/// receiver the other two surfaces already resolve while staying silent
/// itself (#50) — and monomorphised against `ty`'s own type arguments
/// exactly as they do (#48): `Box<number>:get()`'s return type is `number`,
/// not the free `T` a bare-name lookup would leave it as. `sep` is `.`/`:`
/// to match how the call site spells the access.
fn signature_from_class_field(
    ambient: &MergedAmbient,
    ty: &TypeExpr,
    member: &str,
    sep: char,
) -> Option<Signature> {
    let class = sema::named_of(ty)?;
    let shape = ambient.class_members_of(ty)?;
    let field = shape.fields.get(member)?;
    let Ty::Function(fun) = &field.ty else {
        return None;
    };
    let mut params: Vec<RenderedParam> = fun
        .params
        .iter()
        .map(|p| {
            let q = if p.optional { "?" } else { "" };
            RenderedParam {
                label: format!("{}{q}: {}", p.name, p.ty),
                vararg: false,
                doc: None,
            }
        })
        .collect();
    if let Some(vararg_ty) = &fun.varargs {
        params.push(RenderedParam {
            label: format!("...: {vararg_ty}"),
            vararg: true,
            doc: None,
        });
    }
    Some(Signature {
        name: format!("{class}{sep}{member}"),
        params,
        returns: fun.returns.iter().map(ToString::to_string).collect(),
        doc: String::new(),
    })
}

/// [`SigParam`] (from a `---@param`/`fun(...)` [`TypeExpr`][luabox_syntax::luacats::TypeExpr])
/// rendered to a [`RenderedParam`].
fn render_sig_param(p: &SigParam) -> RenderedParam {
    RenderedParam {
        label: render_param(p),
        vararg: p.vararg,
        doc: p.doc.clone(),
    }
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
        label.push_str(&p.label);
        let end = utf16_len(&label);
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: p.doc.as_deref().map(markdown),
        });
    }
    label.push(')');
    if !sig.returns.is_empty() {
        label.push_str(": ");
        label.push_str(&sig.returns.join(", "));
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
        host.set_root(path.parent().expect("a parent dir").to_path_buf());
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

    /// A workspace root every multi-file test lives under.
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" })
    }

    /// [`analyze`], generalised to several files sharing a workspace root —
    /// the first file is the one under test.
    fn analyze_files(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        host.set_root(root());
        let mut first = None;
        for (rel, text) in files {
            let path = root().join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    /// [`help_after_open`], generalised to several files.
    fn help_after_open_files(files: &[(&str, &str)], call: &str) -> Option<SignatureHelp> {
        let src = files[0].1;
        let (analysis, path) = analyze_files(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.rfind(call).expect("call present") + call.len();
        help_at(&analysis, &path, &sema, offset)
    }

    /// [`signature_help`] with the shared `require` resolution and merged
    /// ambient built exactly as the server builds them (#54, #56).
    fn help_at(
        analysis: &Analysis,
        path: &Path,
        sema: &FileSema,
        offset: usize,
    ) -> Option<SignatureHelp> {
        let exports = crate::requires::RequireExports::resolve(
            analysis,
            path,
            &luabox_types::RockSurfaces::default(),
        );
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
        signature_help(sema, offset, &exports, &ambient)
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
        let help = help_at(&analysis, &path, &sema, open).expect("signature help");
        assert_eq!(labels(&help), vec!["f(a: number, b: string)"]);
        assert_eq!(help.active_parameter, Some(0));

        // Cursor right after the comma: the active parameter advances to 1.
        let after_comma = src.rfind("f(1, 2)").unwrap() + "f(1,".len();
        let help = help_at(&analysis, &path, &sema, after_comma).expect("signature help");
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
        let help = help_at(&analysis, &path, &sema, offset).expect("signature help");
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
        let help = help_at(&analysis, &path, &sema, offset).expect("signature help");
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
        let help = help_at(&analysis, &path, &sema, offset).expect("signature help");
        assert_eq!(labels(&help), vec!["f(a: number)", "f(a: string): boolean"]);
    }

    #[test]
    fn cursor_outside_any_call_is_none() {
        let src = "local x = 1\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        assert!(help_at(&analysis, &path, &sema, 8).is_none());
    }

    #[test]
    fn cursor_on_an_unresolvable_callee_is_none() {
        let src = "unknown_global(1, 2)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.find('(').unwrap() + 1;
        assert!(help_at(&analysis, &path, &sema, offset).is_none());
    }

    /// Signature help just after the `(` of the last occurrence of `call`.
    fn help_after_open(src: &str, call: &str) -> Option<SignatureHelp> {
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.rfind(call).expect("call present") + call.len();
        help_at(&analysis, &path, &sema, offset)
    }

    #[test]
    fn a_dotted_call_resolves_via_the_receiver_class_field() {
        let src = "\
---@class Api
---@field send fun(payload: string): boolean sends it

---@type Api
local api = nil
api.send(\"x\")
";
        let help = help_after_open(src, "api.send(").expect("signature help");
        assert_eq!(labels(&help), vec!["Api.send(payload: string): boolean"]);
        assert_eq!(help.active_parameter, Some(0));
    }

    #[test]
    fn a_dotted_call_falls_back_to_the_declared_function() {
        let src = "\
local M = {}
---@param n number
---@return string
function M.helper(n) return tostring(n) end
M.helper(1)
";
        let help = help_after_open(src, "M.helper(").expect("signature help");
        assert_eq!(labels(&help), vec!["M.helper(n: number): string"]);
    }

    // === the workspace ambient reaches signature help too (#50) ===========
    //
    // The defect: hover and completion resolve a cross-file class receiver
    // through the merged ambient (#56), but signature help still called
    // `sema.class_of_name` — file-local only. Before this file wired in
    // `exports`/`ambient`, `c.grow(` showed no signature though `c.` already
    // offered `grow` and hovering it typed it — three surfaces, two answers.

    #[test]
    fn a_method_call_on_a_class_declared_in_another_file_resolves() {
        let files = [
            ("main.lua", "---@type Circle\nlocal c = nil\nc:grow(2)\n"),
            (
                "shapes.lua",
                "---@class Circle\n---@field grow fun(amount: number)\n",
            ),
        ];
        let help = help_after_open_files(&files, "grow(").expect("signature help");
        assert_eq!(labels(&help), vec!["Circle:grow(amount: number)"]);
    }

    /// Probing the other direction: a receiver whose class genuinely has no
    /// such method still answers `None`, even once cross-file resolution is
    /// wired in.
    #[test]
    fn a_method_the_cross_file_class_does_not_declare_is_none() {
        let files = [
            ("main.lua", "---@type Circle\nlocal c = nil\nc:nope(2)\n"),
            ("shapes.lua", "---@class Circle\n---@field radius number\n"),
        ];
        assert!(help_after_open_files(&files, "nope(").is_none());
    }

    // === a bound generic reference resolves its type argument (#48) =======

    #[test]
    fn a_bound_generic_receivers_method_resolves_the_bound_return_type() {
        let files = [
            ("main.lua", "---@type Box<number>\nlocal b = nil\nb:get()\n"),
            ("box.lua", "---@class Box<T>\n---@field get fun(): T\n"),
        ];
        let help = help_after_open_files(&files, "get(").expect("signature help");
        assert_eq!(labels(&help), vec!["Box:get(): number"]);
    }

    /// The one-variable control: the identical class referenced bare stays
    /// lenient — the free `T` — exactly as it did before #48.
    #[test]
    fn an_unbound_generic_receivers_method_stays_lenient() {
        let files = [
            ("main.lua", "---@type Box\nlocal b = nil\nb:get()\n"),
            ("box.lua", "---@class Box<T>\n---@field get fun(): T\n"),
        ];
        let help = help_after_open_files(&files, "get(").expect("signature help");
        assert_eq!(labels(&help), vec!["Box:get(): T"]);
    }

    #[test]
    fn a_dotted_call_on_a_class_carrier_required_from_another_file_resolves() {
        let files = [
            (
                "main.lua",
                "local api = require(\"api\")\napi.send(\"x\")\n",
            ),
            (
                "api.lua",
                "---@class Api\n---@field send fun(payload: string): boolean\nlocal A = {}\nreturn A\n",
            ),
        ];
        let help = help_after_open_files(&files, "send(").expect("signature help");
        assert_eq!(labels(&help), vec!["Api.send(payload: string): boolean"]);
    }

    #[test]
    fn a_dotted_call_on_a_non_name_receiver_is_none() {
        assert!(help_after_open("f().helper(1)\n", "helper(").is_none());
    }

    #[test]
    fn a_method_call_on_a_non_name_receiver_is_none() {
        assert!(help_after_open("f():run(1)\n", "run(").is_none());
    }

    #[test]
    fn a_method_call_on_a_receiver_with_no_class_is_none() {
        let src = "local t = {}\nt:run(1)\n";
        assert!(help_after_open(src, "run(").is_none());
    }

    #[test]
    fn a_table_argument_call_has_no_signature_help() {
        let src = "\
---@param a number
local function f(a) end
f{1}
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // Only parenthesised argument lists carry positional structure.
        let offset = src.rfind("f{1}").expect("call") + "f{".len();
        assert!(help_at(&analysis, &path, &sema, offset).is_none());
    }

    #[test]
    fn a_string_argument_call_has_no_signature_help() {
        let src = "\
---@param a string
local function f(a) end
f\"lit\"
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let offset = src.rfind("f\"lit\"").expect("call") + 2;
        assert!(help_at(&analysis, &path, &sema, offset).is_none());
    }

    #[test]
    fn an_unclosed_call_still_shows_help_at_the_very_end() {
        let src = "\
---@param a number
local function f(a) end
f(
";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        // The `)` has not been typed yet; the cursor sits at the node end.
        let offset = src.rfind("f(").expect("call") + "f(".len();
        let help = help_at(&analysis, &path, &sema, offset).expect("signature help");
        assert_eq!(labels(&help), vec!["f(a: number)"]);
    }

    #[test]
    fn a_call_with_no_parameters_reports_active_parameter_zero() {
        let src = "local function f() end\nf()\n";
        let help = help_after_open(src, "f(").expect("signature help");
        assert_eq!(labels(&help), vec!["f()"]);
        assert_eq!(help.active_parameter, Some(0));
        assert_eq!(help.signatures[0].parameters, None);
    }

    #[test]
    fn an_untyped_optional_parameter_renders_with_a_bare_question_mark() {
        let src = "\
---@overload fun(a?)
local function f(a) end
f()
";
        let help = help_after_open(src, "f(").expect("signature help");
        assert!(labels(&help).contains(&"f(a?)"), "{:?}", labels(&help));
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
        let help = help_at(&analysis, &path, &sema, offset).expect("signature help");
        assert_eq!(labels(&help), vec!["g(b: string)"]);
    }
}
