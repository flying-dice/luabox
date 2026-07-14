//! Hover: the identifier under the cursor rendered as a `lua` code block
//! (binding type, function signature, class field) plus its LuaCATS doc text
//! and the block's `---@see` references (rendered as LuaLS does: a single
//! `See: x` line, or a `See:` header with `  * x` bullets when several).

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use rowan::TextRange;

use crate::sema::{self, FileSema};

/// Compute the hover at a byte `offset`.
#[must_use]
pub fn hover(sema: &FileSema, offset: usize) -> Option<Hover> {
    let token = sema.ident_at(offset)?;
    let token_range = token.text_range();

    // 1. A function declaration site (`function f`, `function M:m`, ...).
    if let Some(info) = sema
        .functions()
        .into_iter()
        .find(|f| f.decl_range == token_range)
    {
        return Some(reply(&info.sig, &info.docs, &info.sees, token_range, sema));
    }

    // 2. A field / method access on a receiver with a known class type.
    if let Some(hover) = member_hover(sema, &token) {
        return Some(hover);
    }

    // 3. A local / upvalue: use site (via resolution) or the declaration.
    let binding = match sema.resolution_at(offset) {
        Some(Resolution::Local(id) | Resolution::Upvalue { binding: id, .. }) => {
            Some(sema.binding(id))
        }
        Some(Resolution::Global(name)) => return global_hover(sema, &name, token_range),
        None => sema.binding_decl_at(offset).map(|id| sema.binding(id)),
    };
    if let Some(binding) = binding {
        // A `local function` reads better as its signature.
        if binding.kind == BindingKind::LocalFunction
            && let Some(info) = sema
                .functions()
                .into_iter()
                .find(|f| f.decl_range == binding.range)
        {
            return Some(reply(&info.sig, &info.docs, &info.sees, token_range, sema));
        }
        let rendered_ty = sema
            .binding_type(binding)
            .map_or_else(|| "unknown".to_string(), |ty| sema::render_type(&ty));
        let keyword = match binding.kind {
            BindingKind::Param | BindingKind::SelfParam => "(param)",
            BindingKind::ForVar => "(for)",
            _ => "local",
        };
        let code = format!("{keyword} {}: {rendered_ty}", binding.name);
        let item = sema.item_covering(binding.range);
        let docs = item.map(sema::docs_of).unwrap_or_default();
        let sees = item.map(sema::sees_of).unwrap_or_default();
        return Some(reply(&code, &docs, &sees, token_range, sema));
    }

    None
}

/// Hover for `recv.field` / `recv:method` when `recv`'s class is known, with
/// a fallback to dotted function names (`M.helper`).
fn member_hover(sema: &FileSema, token: &luabox_syntax::lua::SyntaxToken) -> Option<Hover> {
    let parent = token.parent()?;
    let (receiver, member) = match parent.kind() {
        SyntaxKind::FIELD_EXPR => {
            let field = ast::FieldExpr::cast(parent)?;
            let name_token = field.field_name()?;
            if name_token.text_range() != token.text_range() {
                return None;
            }
            (field.base(), name_token)
        }
        SyntaxKind::METHOD_CALL_EXPR => {
            let call = ast::MethodCallExpr::cast(parent)?;
            let name_token = call.method_name()?;
            if name_token.text_range() != token.text_range() {
                return None;
            }
            (call.receiver(), name_token)
        }
        _ => return None,
    };
    let Some(ast::Expr::Name(recv_name)) = receiver else {
        return None;
    };
    let recv_token = recv_name.name()?;
    let offset = usize::from(recv_token.text_range().start());

    if let Some(class) = sema.class_of_name(recv_token.text(), offset) {
        let (field, declaring) = sema
            .class_fields(&class)
            .into_iter()
            .find(|(f, _)| {
                matches!(&f.key, luabox_syntax::luacats::FieldKey::Name(n) if n == member.text())
            })?;
        let q = if field.optional { "?" } else { "" };
        let code = format!(
            "(field) {declaring}.{}{q}: {}",
            member.text(),
            sema::render_type(&field.ty)
        );
        let docs = field.desc.clone().unwrap_or_default();
        return Some(reply(&code, &docs, &[], member.text_range(), sema));
    }

    // Fallback: an annotated dotted function `M.helper`.
    let dotted = format!("{}.{}", recv_token.text(), member.text());
    let info = sema.functions().into_iter().find(|f| f.name == dotted)?;
    Some(reply(
        &info.sig,
        &info.docs,
        &info.sees,
        member.text_range(),
        sema,
    ))
}

/// Hover for a global name: an annotated/declared function or a class name.
fn global_hover(sema: &FileSema, name: &str, token_range: TextRange) -> Option<Hover> {
    if let Some(info) = sema.functions().into_iter().find(|f| f.name == name) {
        return Some(reply(&info.sig, &info.docs, &info.sees, token_range, sema));
    }
    let classes = sema.classes();
    let info = classes.get(name)?;
    Some(reply(
        &format!("class {name}"),
        &info.docs,
        &info.sees,
        token_range,
        sema,
    ))
}

fn reply(code: &str, docs: &str, sees: &[String], range: TextRange, sema: &FileSema) -> Hover {
    let mut value = format!("```lua\n{code}\n```");
    if !docs.is_empty() {
        value.push_str("\n\n");
        value.push_str(docs);
    }
    if !sees.is_empty() {
        value.push_str("\n\n");
        value.push_str(&see_lines(sees));
    }
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(
            sema.index
                .range(usize::from(range.start())..usize::from(range.end())),
        ),
    }
}

/// Render `---@see` references the way LuaLS's hover does
/// (`core/hover/description.lua`, `lookUpDocSees`): one reference inline,
/// several as an indented bullet list.
fn see_lines(sees: &[String]) -> String {
    match sees {
        [only] => format!("See: {only}"),
        many => {
            let mut out = String::from("See:");
            for see in many {
                out.push_str("\n  * ");
                out.push_str(see);
            }
            out
        }
    }
}
