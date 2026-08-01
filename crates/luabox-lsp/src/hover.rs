//! Hover: the identifier under the cursor rendered as a `lua` code block
//! (binding type, function signature, class field) plus its LuaCATS doc text
//! and the block's `---@see` references (rendered as LuaLS does: a single
//! `See: x` line, or a `See:` header with `  * x` bullets when several).

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_types::ty::Ty;
use rowan::TextRange;

use crate::requires::{self, RequireExports};
use crate::sema::{self, FileSema};

/// Compute the hover at a byte `offset`.
///
/// `exports` is the shared `require` resolution ([`RequireExports`]) — the
/// same map the type pass checks against, so a `require` binding hovers with
/// the type the problems pane already agrees it has (#54).
#[must_use]
pub fn hover(sema: &FileSema, offset: usize, exports: &RequireExports) -> Option<Hover> {
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
    if let Some(hover) = member_hover(sema, &token, exports) {
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
        // An explicit `---@type`/`---@param` first, then — for a `require`
        // binding — the module's export type out of the shared resolution
        // (#54). Explicit beats implicit, the same precedence the rest of the
        // toolchain uses; what changed is that "implicit" is no longer a
        // synonym for `unknown`.
        let rendered_ty = sema
            .binding_type(binding)
            .map(|ty| sema::render_type(&ty))
            .or_else(|| exports.binding_export(sema, binding).map(Ty::to_string))
            .unwrap_or_else(|| "unknown".to_string());
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

/// Hover for `recv.field` / `recv:method` when `recv`'s class is known, when
/// `recv` is a `require` binding whose module exports the member, with a
/// fallback to dotted function names (`M.helper`).
fn member_hover(
    sema: &FileSema,
    token: &luabox_syntax::lua::SyntaxToken,
    exports: &RequireExports,
) -> Option<Hover> {
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

    // A member of a `require` binding: the field comes out of the module's
    // export type, which is the same type the problems pane checks the
    // access against (#54). Qualified by the *module* rather than the local
    // name, so the hover names what it came from.
    if let Some(binding) = sema.visible_binding_named(recv_token.text(), offset)
        && let Some(module) = requires::require_module_of(sema, binding)
        && let Some(fields) = exports.get(module).and_then(requires::export_fields)
        && let Some(field) = fields.get(member.text())
    {
        let q = if field.optional { "?" } else { "" };
        let code = format!("(field) {module}.{}{q}: {}", member.text(), field.ty);
        return Some(reply(&code, "", &[], member.text_range(), sema));
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
    clippy::string_slice,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};
    use luabox_types::RockSurfaces;

    /// The workspace root every test file lives under.
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" })
    }

    /// Analyse `files` (the first is the file under test) with the workspace
    /// root set, so cross-file `require` resolution finds the siblings.
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

    fn analyze(text: &str) -> (Analysis, PathBuf) {
        analyze_files(&[("main.lua", text)])
    }

    /// Byte offset just inside the `nth` (0-based) occurrence of `needle`.
    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    /// The rendered markdown of the hover at the `nth` occurrence of `needle`
    /// in the first of `files`, with `rocks` in the shared `require` resolution.
    fn at_with(
        files: &[(&str, &str)],
        needle: &str,
        nth: usize,
        rocks: &RockSurfaces,
    ) -> Option<String> {
        let src = files[0].1;
        let (analysis, path) = analyze_files(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, rocks);
        hover(&sema, offset_of(src, needle, nth), &exports).map(|h| match h.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        })
    }

    /// [`at_with`] across `files` and no rock tree.
    fn at_files(files: &[(&str, &str)], needle: &str, nth: usize) -> Option<String> {
        at_with(files, needle, nth, &RockSurfaces::default())
    }

    /// The rendered markdown of the hover at the `nth` occurrence of `needle`.
    fn at(src: &str, needle: &str, nth: usize) -> Option<String> {
        at_files(&[("main.lua", src)], needle, nth)
    }

    /// One annotated rock installed as `mylib`, harvested the way the server
    /// harvests a vendored `lua_modules` tree (#30).
    fn mylib_rock() -> RockSurfaces {
        let source = luabox_types::RockModule {
            module: "mylib".to_string(),
            label: "lua_modules/share/lua/5.4/mylib/init.lua".to_string(),
            path: root().join("lua_modules/share/lua/5.4/mylib/init.lua"),
            text: "\
local M = {}

---Greets.
---@param who string
---@return string
function M.greet(who) return \"hi \" .. who end

return M
"
            .to_string(),
        };
        let ambient = luabox_types::build_ambient(Dialect::Lua54, &[]);
        luabox_types::rocks::harvest(&ambient, &[source])
    }

    /// A two-file workspace: `main.lua` requires the annotated `other.lua`.
    const OTHER: &str = "\
local M = {}

---Helps.
---@param n number
---@return string
function M.helper(n) return tostring(n) end

return M
";

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
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let hovered = hover(&sema, offset_of(src, "answer", 1), &exports).expect("hover");
        let range = hovered.range.expect("range");
        assert_eq!(range.start, lsp_types::Position::new(1, 6));
        assert_eq!(range.end, lsp_types::Position::new(1, 12));
    }

    // === `require` bindings (#54) =========================================
    //
    // The defect: `local m = require("mod")` hovered `unknown` while the type
    // pass — CLI and LSP diagnostics alike — already knew the module's export
    // type for the same binding. These pin the agreement.

    #[test]
    fn a_require_binding_hovers_as_the_module_export_type() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nprint(m)\n"),
            ("other.lua", OTHER),
        ];
        let text = at_files(&files, "m)", 0).expect("hover");
        assert!(text.contains("local m: "), "{text}");
        assert!(text.contains("helper"), "{text}");
        assert!(!text.contains("local m: unknown"), "{text}");
    }

    /// The declaration site answers the same as the use site: the two go
    /// through different hover routes (`binding_decl_at` vs the resolution),
    /// and the regression was visible at both.
    #[test]
    fn a_require_binding_hovers_the_same_at_its_declaration() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nprint(m)\n"),
            ("other.lua", OTHER),
        ];
        let decl = at_files(&files, "m = require", 0).expect("hover");
        let use_site = at_files(&files, "m)", 0).expect("hover");
        assert_eq!(decl, use_site, "declaration and use must agree");
    }

    #[test]
    fn a_rock_require_binding_hovers_as_the_harvested_export() {
        // `mylib` is not a project file: the export comes from the rock
        // harvest's module map, the same source the type pass reads.
        let files = [(
            "main.lua",
            "local mylib = require(\"mylib\")\nprint(mylib)\n",
        )];
        let text = at_with(&files, "mylib)", 0, &mylib_rock()).expect("hover");
        assert!(text.contains("greet"), "{text}");
        assert!(!text.contains("local mylib: unknown"), "{text}");
    }

    #[test]
    fn a_field_of_a_require_binding_hovers_as_the_module_field() {
        let files = [
            (
                "main.lua",
                "local m = require(\"other\")\nprint(m.helper(1))\n",
            ),
            ("other.lua", OTHER),
        ];
        let text = at_files(&files, "helper(1)", 0).expect("hover");
        assert!(text.contains("(field) other.helper: fun("), "{text}");
        assert!(text.contains("string"), "{text}");
    }

    #[test]
    fn a_field_a_required_module_does_not_export_has_no_hover() {
        let files = [
            ("main.lua", "local m = require(\"other\")\nprint(m.nope)\n"),
            ("other.lua", OTHER),
        ];
        assert_eq!(at_files(&files, "nope", 0), None);
    }

    /// An explicit annotation beats the inferred module export, the same
    /// precedence the rest of the toolchain uses.
    #[test]
    fn an_annotated_require_binding_keeps_its_annotation() {
        let files = [
            (
                "main.lua",
                "---@type string\nlocal m = require(\"other\")\nprint(m)\n",
            ),
            ("other.lua", OTHER),
        ];
        let text = at_files(&files, "m)", 0).expect("hover");
        assert!(text.contains("local m: string"), "{text}");
    }

    #[test]
    fn a_require_of_a_module_that_does_not_exist_hovers_gracefully() {
        let files = [("main.lua", "local m = require(\"absent\")\nprint(m)\n")];
        let text = at_files(&files, "m)", 0).expect("hover");
        assert!(text.contains("local m: unknown"), "{text}");
    }

    /// By design: a dynamic require has no statically known module, so there
    /// is nothing to show. `unknown` is the correct answer, not a gap.
    #[test]
    fn a_dynamic_require_binding_hovers_as_unknown_by_design() {
        let files = [
            (
                "main.lua",
                "local name = \"other\"\nlocal m = require(name)\nprint(m)\n",
            ),
            ("other.lua", OTHER),
        ];
        let text = at_files(&files, "m)", 0).expect("hover");
        assert!(text.contains("local m: unknown"), "{text}");
    }

    /// Also by design: `require("a") or require("b")` resolves at runtime, so
    /// naming either module's type would be a guess dressed as a fact.
    #[test]
    fn an_or_chained_require_binding_hovers_as_unknown_by_design() {
        let files = [
            (
                "main.lua",
                "local m = require(\"other\") or require(\"spare\")\nprint(m)\n",
            ),
            ("other.lua", OTHER),
            ("spare.lua", "return 1\n"),
        ];
        let text = at_files(&files, "m)", 0).expect("hover");
        assert!(text.contains("local m: unknown"), "{text}");
    }

    #[test]
    fn a_shadowed_require_binding_hovers_as_its_own_module() {
        let files = [
            (
                "main.lua",
                "local m = require(\"other\")\nprint(m)\nlocal m = require(\"spare\")\nprint(m)\n",
            ),
            ("other.lua", OTHER),
            (
                "spare.lua",
                "local S = {}\n---@return number\nfunction S.count() return 1 end\nreturn S\n",
            ),
        ];
        let first = at_files(&files, "m)", 0).expect("hover");
        let second = at_files(&files, "m)", 1).expect("hover");
        assert!(first.contains("helper"), "{first}");
        assert!(!first.contains("count"), "{first}");
        assert!(second.contains("count"), "{second}");
        assert!(!second.contains("helper"), "{second}");
    }

    /// The one shape that does *not* fully close: a module whose export is a
    /// `---@class` instance rather than a table literal. The export type is
    /// `Ty::Named("Point")`, so the binding hovers as the class — useful, and
    /// what the type pass has — but the class's `---@field`s live in the
    /// *declaring* file, and resolving them needs the ambient environment
    /// only the type pass holds. Members of such a module therefore have no
    /// hover here while diagnostics still check them. Pinned so the boundary
    /// is a recorded fact rather than a surprise; the README scopes the claim
    /// to match.
    #[test]
    fn a_class_instance_module_hovers_as_the_class_but_has_no_member_hover() {
        let files = [
            (
                "main.lua",
                "local p = require(\"point\")\nprint(p)\nprint(p.x)\n",
            ),
            (
                "point.lua",
                "---@class Point\n---@field x number\n\n---@type Point\nlocal P = nil\nreturn P\n",
            ),
        ];
        let binding = at_files(&files, "p)", 0).expect("hover");
        assert!(binding.contains("local p: Point"), "{binding}");
        assert_eq!(at_files(&files, "x)", 0), None);
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
