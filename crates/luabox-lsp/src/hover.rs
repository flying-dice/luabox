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

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

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

    /// Byte offset just inside the `nth` (0-based) occurrence of `needle`.
    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    /// The rendered markdown of the hover at the `nth` occurrence of `needle`.
    fn at(src: &str, needle: &str, nth: usize) -> Option<String> {
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        hover(&sema, offset_of(src, needle, nth)).map(|h| match h.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        })
    }

    #[test]
    fn function_declaration_name_hovers_as_its_signature() {
        let src = "\
---Adds one.
---@param n number
---@return number
function bump(n) return n + 1 end
";
        // Cursor on the declaration name itself, not a call site.
        let text = at(src, "bump", 0).expect("hover");
        assert!(text.contains("function bump(n: number): number"), "{text}");
        assert!(text.contains("Adds one."), "{text}");
    }

    #[test]
    fn global_function_use_hovers_as_its_signature() {
        let src = "\
---@param n number
function bump(n) return n + 1 end
bump(1)
";
        // The use site resolves to a global, routed through `global_hover`.
        let text = at(src, "bump", 1).expect("hover");
        assert!(text.contains("function bump(n: number)"), "{text}");
    }

    #[test]
    fn global_class_name_hovers_as_a_class() {
        let src = "\
---A 2-D point.
---@class Point
---@field x number

local alias = Point
";
        let text = at(src, "Point", 1).expect("hover");
        assert!(text.contains("```lua\nclass Point\n```"), "{text}");
        assert!(text.contains("A 2-D point."), "{text}");
    }

    #[test]
    fn unknown_global_has_no_hover() {
        // A global that is neither a declared function nor a class.
        assert_eq!(at("print(nothing_here)\n", "nothing_here", 0), None);
    }

    #[test]
    fn identifier_that_is_neither_a_name_use_nor_a_binding_has_no_hover() {
        // A table-constructor key is an identifier with no resolution and no
        // binding, so every hover route declines.
        assert_eq!(at("local t = { key = 1 }\n", "key", 0), None);
    }

    #[test]
    fn local_declaration_site_hovers_as_its_annotated_type() {
        let src = "---the answer\n---@type number\nlocal answer = 42\n";
        // The declaration name has no resolution; `binding_decl_at` answers.
        let text = at(src, "answer = 42", 0).expect("hover");
        assert!(text.contains("local answer: number"), "{text}");
        assert!(text.contains("the answer"), "{text}");
    }

    #[test]
    fn unannotated_local_renders_as_unknown() {
        let src = "local thing = 42\nprint(thing)\n";
        let text = at(src, "thing", 1).expect("hover");
        assert!(text.contains("local thing: unknown"), "{text}");
    }

    #[test]
    fn parameter_hovers_with_the_param_keyword() {
        let src = "\
---@param n number
local function f(n) return n end
";
        let text = at(src, "n) return", 0).expect("hover");
        assert!(text.contains("(param) n: number"), "{text}");
    }

    #[test]
    fn self_parameter_hovers_with_the_param_keyword() {
        let src = "\
---@class Greeter
local G = {}
function G:greet() return self end
";
        let text = at(src, "self", 0).expect("hover");
        assert!(text.starts_with("```lua\n(param) self:"), "{text}");
    }

    #[test]
    fn for_variable_hovers_with_the_for_keyword() {
        let src = "for i = 1, 10 do print(i) end\n";
        let text = at(src, "i) end", 0).expect("hover");
        assert!(text.contains("(for) i: unknown"), "{text}");
    }

    #[test]
    fn method_call_name_hovers_as_the_class_field() {
        let src = "\
---@class Greeter
---@field greet fun(self: Greeter): string the greeting

---@type Greeter
local g = nil
g:greet()
";
        let text = at(src, "greet()", 0).expect("hover");
        assert!(
            text.contains("(field) Greeter.greet: fun(self: Greeter): string"),
            "{text}"
        );
        assert!(text.contains("the greeting"), "{text}");
    }

    #[test]
    fn optional_class_field_is_marked_with_a_question_mark() {
        let src = "\
---@class Config
---@field debug? boolean

---@type Config
local cfg = nil
print(cfg.debug)
";
        let text = at(src, "debug)", 0).expect("hover");
        assert!(text.contains("(field) Config.debug?: boolean"), "{text}");
    }

    #[test]
    fn field_receiver_hovers_as_its_own_binding_not_the_field() {
        let src = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p.x)
";
        // The cursor is on `p`, not on `x`: the member route must decline.
        let text = at(src, "p.x", 0).expect("hover");
        assert!(text.contains("local p: Point"), "{text}");
    }

    #[test]
    fn dotted_function_without_a_class_falls_back_to_its_signature() {
        let src = "\
local M = {}
---Helps.
---@param n number
---@return string
function M.helper(n) return tostring(n) end
M.helper(1)
";
        // `M` has no `---@class` type, so the dotted-name fallback answers.
        let text = at(src, "helper(1)", 0).expect("hover");
        assert!(
            text.contains("function M.helper(n: number): string"),
            "{text}"
        );
        assert!(text.contains("Helps."), "{text}");
    }

    #[test]
    fn member_access_on_a_non_name_receiver_has_no_hover() {
        // The receiver is a call expression, not a bare name.
        assert_eq!(at("local t = f().field\n", "field", 0), None);
    }

    #[test]
    fn unknown_field_on_a_known_class_has_no_hover() {
        let src = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p.z)
";
        assert_eq!(at(src, "z)", 0), None);
    }

    #[test]
    fn hover_range_covers_exactly_the_identifier() {
        let src = "local answer = 42\nprint(answer)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let hovered = hover(&sema, offset_of(src, "answer", 1)).expect("hover");
        let range = hovered.range.expect("range");
        assert_eq!(range.start, lsp_types::Position::new(1, 6));
        assert_eq!(range.end, lsp_types::Position::new(1, 12));
    }

    #[test]
    fn see_lines_render_one_inline_and_several_as_bullets() {
        assert_eq!(see_lines(&["a.b".to_string()]), "See: a.b");
        assert_eq!(
            see_lines(&["a.b".to_string(), "c.d".to_string()]),
            "See:\n  * a.b\n  * c.d"
        );
        assert_eq!(see_lines(&[]), "See:");
    }
}
