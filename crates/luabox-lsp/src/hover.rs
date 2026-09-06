//! Hover: the identifier under the cursor rendered as a `lua` code block
//! (binding type, function signature, class field) plus its LuaCATS doc text
//! and the block's `---@see` references (rendered as LuaLS does: a single
//! `See: x` line, or a `See:` header with `  * x` bullets when several).

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};
use luabox_db::Analysis;
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_types::ty::Ty;
use rowan::TextRange;

use crate::merged_ambient::MergedAmbient;
use crate::render::OwnParamErasure;
use crate::requires::{self, RequireExports};
use crate::sema::{self, FileSema};

/// Compute the hover at a byte `offset`.
///
/// `exports` is the shared `require` resolution ([`RequireExports`]) — the
/// same map the type pass checks against, so a `require` binding hovers with
/// the type the problems pane already agrees it has (#54). `ambient` is the
/// merged workspace environment the same pass enforces (#56), so a
/// class-typed value's members hover with the types `luabox check` gives
/// them, wherever the class is declared — including when the receiver's own
/// class is declared in *this* file but inherits from one declared
/// elsewhere, which the file-local-only lookup this replaced could not see
/// (#46). `analysis` backs the cross-file `---@field` description lookup
/// ([`sema::locate_field`], #51) — the merged ambient's shape carries a
/// field's type but not its doc text.
#[must_use]
pub fn hover(
    sema: &FileSema,
    offset: usize,
    exports: &RequireExports,
    ambient: &MergedAmbient,
    analysis: &Analysis,
) -> Option<Hover> {
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
    if let Some(hover) = member_hover(sema, &token, exports, ambient, analysis) {
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

/// Hover for `recv.field` / `recv:method` when `recv`'s class is known —
/// resolved through the workspace ambient (#56), which already includes
/// every class this file itself declares — when `recv` is a `require`
/// binding whose module exports the member (#54), with a fallback to
/// dotted function names (`M.helper`).
fn member_hover(
    sema: &FileSema,
    token: &luabox_syntax::lua::SyntaxToken,
    exports: &RequireExports,
    ambient: &MergedAmbient,
    analysis: &Analysis,
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

    if let Some(binding) = sema.visible_binding_named(recv_token.text(), offset) {
        // A member of a `require` binding via its module's structural
        // export: the field comes out of the module's export type, which
        // is the same type the problems pane checks the access against
        // (#54). Qualified by the *module* rather than the local name, so
        // the hover names what it came from. Only when the binding carries
        // no class reference of its own — an explicit `---@type` beats an
        // inferred module export, the same precedence the rest of the
        // toolchain uses (R7): `requires::require_struct_fields` is the one
        // place both hover and completion check that before falling back to
        // the table shape.
        if let Some((module, fields)) =
            requires::require_struct_fields(sema, exports, ambient, analysis, binding)
            && let Some(field) = fields.get(member.text())
        {
            let q = if field.optional { "?" } else { "" };
            let code = format!("(field) {module}.{}{q}: {}", member.text(), field.ty);
            return Some(reply(&code, "", &[], member.text_range(), sema));
        }

        // The receiver's class reference, resolved through the workspace
        // ambient (#56): its annotated type names a class — declared in
        // this file or any other, the merged ambient already includes both
        // (#46) — or it is a `require` binding whose module exports a class
        // (both `---@class` spellings cross the boundary as [`Ty::Named`]).
        // Members come out of the same merged surface the checker resolves
        // against, monomorphised against the reference's own type
        // arguments exactly as the checker monomorphises the same
        // reference at its use site (#48): `Box<number>`'s `item` hovers
        // `number`, not the free `T` a bare-name lookup would leave it as.
        // A *bare* reference's own free parameter (M21, round 6 review) is
        // separately erased to `unknown` through `render::OwnParamErasure`
        // — matching `TypeEnv::class_shape_bound_export`'s erasure, which
        // this member route used to skip. That renderer is shared with
        // completion and signature help now, so all three agree about the
        // same field of the same reference (production readiness review,
        // finding 3).
        if let Some(ty) = requires::receiver_type(sema, exports, binding)
            && let Some(class) = sema::named_of(&ty)
            && let Some(shape) = ambient.class_members_of(&ty)
        {
            if let Some(field) = shape.fields.get(member.text()) {
                let q = if field.optional { "?" } else { "" };
                let erasure =
                    OwnParamErasure::at_reference(&ty, &class, analysis, &sema.path, ambient);
                let rendered = erasure.render(&field.ty);
                let code = format!("(field) {class}.{}{q}: {rendered}", member.text());
                let docs = ambient
                    .locate_field(analysis, &sema.path, &class, member.text())
                    .and_then(|found| found.desc)
                    .unwrap_or_default();
                return Some(reply(&code, &docs, &[], member.text_range(), sema));
            }
            // A dynamic-access class (#53): an indexer or array part makes
            // any member access lenient to the checker (`infer.rs`'s
            // `provable = false` — a declared indexer/array admits any
            // string key), so hover agrees rather than showing nothing for
            // a member `luabox check` accepts. No specific type is
            // promised — the checker itself does not type the access
            // beyond leniency.
            if !shape.indexers.is_empty() || shape.array.is_some() {
                let code = format!("(field) {class}.{}: unknown", member.text());
                return Some(reply(&code, "", &[], member.text_range(), sema));
            }
            // `class`'s ancestry was too deep for `class_members_of` to
            // resolve fully (`LB0317`) and `member` is not among the fields
            // it did reach — say so, rather than silently falling through to
            // "no hover" as if `member` plainly does not exist. The Problems
            // panel already carries `LB0317` for this class's declaration
            // (`crate::diagnostics`, same file, same class, its own
            // `TypeEnv`); hover reads through `ambient`'s separate,
            // long-lived env instead (production readiness review, finding
            // 2), so it needs its own check of the same fact to agree with
            // what the panel already told the user.
            if ambient.class_ancestry_truncated(&class) {
                let code = format!("(field) {class}.{}: unknown", member.text());
                let note = format!(
                    "`{class}`'s `---@class` ancestry exceeds the \
                     {}-class limit this checker enforces to resolve it safely (LB0317) — \
                     `{}` may be declared above that limit and missing here for that reason \
                     alone.",
                    luabox_types::MAX_ANCESTRY_DEPTH,
                    member.text()
                );
                return Some(reply(&code, &note, &[], member.text_range(), sema));
            }
        }
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
    let info = sema.class_named(name)?;
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
        at_in(files, files[0].0, needle, nth, rocks)
    }

    /// [`at_with`] with the cursor in a named file rather than the first one
    /// loaded. Load order is what `merge_file_types` resolves a same-name
    /// collision by, so a test that needs the cursor's file to *lose* that
    /// collision (#70) has to name the two separately.
    fn at_in(
        files: &[(&str, &str)],
        cursor: &str,
        needle: &str,
        nth: usize,
        rocks: &RockSurfaces,
    ) -> Option<String> {
        let src = files
            .iter()
            .find(|(rel, _)| *rel == cursor)
            .expect("the cursor file must be one of `files`")
            .1;
        let (analysis, _) = analyze_files(files);
        let path = root().join(cursor);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, rocks);
        // The merged workspace layer, exactly as the server builds it (#56).
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), rocks.types());
        hover(
            &sema,
            offset_of(src, needle, nth),
            &exports,
            &ambient,
            &analysis,
        )
        .map(|h| match h.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        })
    }

    /// [`at_with`] across `files` and no rock tree.
    fn at_files(files: &[(&str, &str)], needle: &str, nth: usize) -> Option<String> {
        at_with(files, needle, nth, &RockSurfaces::default())
    }

    /// [`at_files`] with the cursor in a file that is **not** the first one
    /// loaded (#70) — see [`at_in`].
    fn at_files_from(
        files: &[(&str, &str)],
        cursor: &str,
        needle: &str,
        nth: usize,
    ) -> Option<String> {
        at_in(files, cursor, needle, nth, &RockSurfaces::default())
    }

    /// #70: one class, its `---@field`s split across two files, cursor in the
    /// file that **lost** the merge. The type comes from `collect_class` and
    /// reads `string`; the description used to come from a separate walk that
    /// checks the cursor's own file first and read `from main` — one tooltip
    /// quoting two different declarations. Both now come from the same
    /// resolved answer.
    #[test]
    fn a_cross_file_split_fields_type_and_description_name_one_declaration() {
        let hover = at_files_from(
            &[
                ("a.lua", "---@class Split\n---@field f string from a\n"),
                (
                    "main.lua",
                    "---@class Split\n---@field f number from main\n\n---@type Split\nlocal s = nil\nprint(s.f)\n",
                ),
            ],
            "main.lua",
            "f)",
            0,
        )
        .expect("hover");
        assert!(
            hover.contains("(field) Split.f: string"),
            "the type is the merge's: {hover}"
        );
        assert!(
            hover.contains("from a"),
            "…and so is the description: {hover}"
        );
        assert!(
            !hover.contains("from main"),
            "the losing declaration must not be quoted: {hover}"
        );
    }

    /// [`at_files`] over a workspace with a **dependency** definition package
    /// (round 8 review, F7).
    ///
    /// `defs` stands in for `lua_modules/<dep>/defs/*.d.lua`: contributed to
    /// the ambient layer the way `server.rs` contributes it, and — the whole
    /// point — deliberately **not** written into the analysis, because
    /// `layout::collect_lua_files` excludes that directory from the source
    /// walk, so those files are never in `Analysis::files()` for the real
    /// server either.
    fn at_files_with_defs(
        files: &[(&str, &str)],
        defs: &[&str],
        needle: &str,
        nth: usize,
    ) -> Option<String> {
        let src = files[0].1;
        let (analysis, path) = analyze_files(files);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let rocks = RockSurfaces::default();
        let exports = RequireExports::resolve(&analysis, &path, &rocks);
        let sources: Vec<String> = defs.iter().map(|d| (*d).to_string()).collect();
        let base = luabox_types::build_ambient(luabox_syntax::lua::Dialect::Lua54, &sources);
        let ambient = MergedAmbient::build(&base, &analysis.project_types(), rocks.types())
            .with_ambient_alias_names(crate::merged_ambient::alias_or_enum_names(&sources));
        hover(
            &sema,
            offset_of(src, needle, nth),
            &exports,
            &ambient,
            &analysis,
        )
        .map(|h| match h.contents {
            HoverContents::Markup(markup) => markup.value,
            other => panic!("expected markup hover contents, got {other:?}"),
        })
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

    /// Production readiness review, finding 2: `member_hover` reads a
    /// class's members through [`MergedAmbient::class_members_of`], a
    /// *different*, long-lived env than the one `crate::diagnostics` drains
    /// per file for the Problems panel — this module's own doc (#56)
    /// promises "the types `luabox check` gives them", but before this fix a
    /// field genuinely declared by `C0`, just past the resolver's ancestry
    /// cutoff, silently vanished from hover with no signal at all, while the
    /// Problems panel for the same file correctly showed `LB0317`.
    #[test]
    fn a_field_past_the_ancestry_cutoff_hovers_with_a_truncation_note() {
        let mut src = String::from("---@class C0\n---@field item number\n");
        for i in 1..=400 {
            use std::fmt::Write as _;
            let _ = writeln!(src, "---@class C{i} : C{}", i - 1);
        }
        src.push_str("\n---@type C400\nlocal x = nil\nprint(x.item)\n");
        let text = at(&src, "item)", 0).expect("hover must say something, not vanish silently");
        assert!(text.contains("LB0317"), "{text}");
    }

    #[test]
    fn hover_range_covers_exactly_the_identifier() {
        let src = "local answer = 42\nprint(answer)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports = RequireExports::resolve(&analysis, &path, &RockSurfaces::default());
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
        let hovered = hover(
            &sema,
            offset_of(src, "answer", 1),
            &exports,
            &ambient,
            &analysis,
        )
        .expect("hover");
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

    /// R7: an explicit `---@type` on a `require` binding must win over the
    /// module's plain structural table export for a *member* hover too, not
    /// just the binding hover `an_annotated_require_binding_keeps_its_annotation`
    /// already pinned. Before the fix, the structural-export arm ran first
    /// unconditionally and answered `(field) m.x: 42` with no doc, dropping
    /// the annotation; the caret must resolve through `Point` instead.
    #[test]
    fn an_explicit_annotation_on_a_require_binding_wins_a_member_hover_too() {
        let files = [
            (
                "main.lua",
                "---@class Point\n---@field x number the point's x\n\n---@type Point\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let text = at_files(&files, "x)", 0).expect("hover");
        assert!(text.contains("Point.x: number"), "{text}");
        assert!(text.contains("the point's x"), "{text}");
        assert!(!text.contains("m.x"), "{text}");
    }

    /// The one-variable control: with no `---@type` at all, the same `m.lua`
    /// module still hovers off its structural table export exactly as
    /// before — the fix must not have swallowed the plain-table case.
    #[test]
    fn an_unannotated_require_bindings_member_still_hovers_the_structural_export() {
        let files = [
            ("main.lua", "local m = require(\"m\")\nprint(m.x)\n"),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let text = at_files(&files, "x)", 0).expect("hover");
        assert!(text.contains("(field) m.x: 42"), "{text}");
    }

    /// N16: the R7 gate must be a *resolves* check, not a *presence* check.
    /// `---@type table` names a real LuaCATS type, but `table` is not a class
    /// the ambient can resolve members from — `luabox check` reports zero
    /// diagnostics for this exact fixture. Before the fix,
    /// `require_struct_fields` bailed on `receiver_type(...).is_some()`
    /// alone, so this non-resolving annotation suppressed the structural
    /// route too and the member hover went `None` — the class arm right
    /// below it also fails to resolve, and nothing catches it.
    #[test]
    fn an_annotation_that_does_not_resolve_to_a_class_falls_back_to_the_structural_export() {
        let files = [
            (
                "main.lua",
                "---@type table\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let text = at_files(&files, "x)", 0).expect("hover");
        assert!(text.contains("(field) m.x: 42"), "{text}");
    }

    /// Same shape, an annotation naming a class that simply does not exist —
    /// the mid-edit case: a partially-typed class name resolves to nothing
    /// too, and must not blank out the structural fallback either.
    #[test]
    fn an_annotation_naming_an_undeclared_class_falls_back_to_the_structural_export() {
        let files = [
            (
                "main.lua",
                "---@type Bogus\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let text = at_files(&files, "x)", 0).expect("hover");
        assert!(text.contains("(field) m.x: 42"), "{text}");
    }

    /// M20 (round 6 review): the R7 gate is a *resolves* check, but before
    /// this fix `require_struct_fields` only recognised a **class** as
    /// resolving (`ambient.class_members_of`) — an explicit `---@type`
    /// naming an `---@alias` (even one that itself expands to a class) fell
    /// through to the raw structural export and hovered the confidently
    /// wrong `(field) m.x: 42`, while `luabox check` enforces the alias's
    /// real type through the same annotation. Confidently wrong is worse
    /// than the honest `None` a fully unresolvable annotation gets (round
    /// 5's behaviour for the equivalent shape) — this crate cannot expand
    /// the alias itself (`sema::is_declared_alias_or_enum`'s own doc), so
    /// `None` is the fixed answer, not the fully-correct `Point.x: string`.
    #[test]
    fn an_annotation_naming_an_alias_is_not_treated_as_the_structural_export() {
        let files = [
            (
                "main.lua",
                "---@alias Foo Point\n\n---@class Point\n---@field x string the point's x\n\n---@type Foo\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        assert_eq!(at_files(&files, "x)", 0), None);
    }

    /// H3 (claim C23): the require-vs-annotation gate accepts *any*
    /// resolvable annotated type, not only a class (M20) —
    /// `an_annotation_naming_an_alias_is_not_treated_as_the_structural_export`
    /// above only ever exercised the alias half of that claim; nothing
    /// exercised an `---@enum`, the gate's other non-class case
    /// (`sema::is_declared_alias_or_enum` checks both `Tag::Alias` and
    /// `Tag::Enum` in the same pass). Reverting the gate to classes-only
    /// (dropping the `is_declared_alias_or_enum` arm from
    /// `requires::require_struct_fields`) turns both of these back into the
    /// confidently wrong `(field) m.x: 42` structural export instead of the
    /// honest `None` — proved by hand: reverting that arm, running this
    /// test, and confirming RED before restoring it.
    #[test]
    fn an_annotation_naming_an_alias_or_enum_is_not_treated_as_the_structural_export() {
        let alias_files = [
            (
                "main.lua",
                "---@alias Foo string\n\n---@type Foo\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        assert_eq!(
            at_files(&alias_files, "x)", 0),
            None,
            "an alias-annotated binding must reject the structural export"
        );

        let enum_files = [
            (
                "main.lua",
                "---@enum Bar\nlocal B = { a = 1 }\n\n---@type Bar\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        assert_eq!(
            at_files(&enum_files, "x)", 0),
            None,
            "an enum-annotated binding must reject the structural export"
        );
    }

    /// F7 (round 8 review): M20's shape one tier down. The two tests above
    /// declare their alias/enum in a *project* file, which
    /// `sema::is_declared_alias_or_enum`'s `analysis.files()` scan finds. A
    /// **dependency**'s defs never appear in that set at any revision —
    /// `layout::collect_lua_files` excludes `lua_modules/<dep>/defs/` from
    /// the source walk entirely — while `luabox check` enforces exactly those
    /// declarations through the merged ambient. So the gate saw
    /// `---@type DepAlias` as resolving to nothing and fell through to the
    /// `require`d module's raw structural export: `(field) m.x: 42`,
    /// confidently wrong about a binding the checker types as a `string`.
    ///
    /// Both spellings, because the ambient tier has the same two:
    /// `---@alias` and `---@enum`.
    #[test]
    fn an_annotation_naming_a_dependency_defs_alias_or_enum_is_not_the_structural_export() {
        let module = ("m.lua", "local M = {}\nM.x = 42\nreturn M\n");

        let alias_defs = ["---@meta\n\n---@alias DepAlias string\n"];
        let alias_files = [
            (
                "main.lua",
                "---@type DepAlias\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            module,
        ];
        assert_eq!(
            at_files_with_defs(&alias_files, &alias_defs, "x)", 0),
            None,
            "a dependency-defs alias must reject the structural export the \
             same way a project-file one does"
        );

        let enum_defs = ["---@meta\n\n---@enum DepEnum\nlocal E = { a = 1 }\n"];
        let enum_files = [
            (
                "main.lua",
                "---@type DepEnum\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            module,
        ];
        assert_eq!(
            at_files_with_defs(&enum_files, &enum_defs, "x)", 0),
            None,
            "and so must a dependency-defs enum"
        );
    }

    /// The control the fix must not break: with the ambient layer carrying no
    /// alias of that name, an annotation naming nothing at all still falls
    /// back to the structural export (round 5's behaviour, unchanged) — the
    /// fallback must recognise what the ambient really declares, not answer
    /// `true` for every name it is asked about.
    #[test]
    fn an_annotation_naming_nothing_the_dependency_defs_declare_still_falls_back() {
        let files = [
            (
                "main.lua",
                "---@type NotInTheDefs\nlocal m = require(\"m\")\nprint(m.x)\n",
            ),
            ("m.lua", "local M = {}\nM.x = 42\nreturn M\n"),
        ];
        let defs = ["---@meta\n\n---@alias DepAlias string\n"];
        let text = at_files_with_defs(&files, &defs, "x)", 0).expect("hover");
        assert!(text.contains("(field) m.x: 42"), "{text}");
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

    /// The shape that used to be the recorded boundary (#54), closed by #56:
    /// a module whose export is a `---@class` instance. The binding hovers
    /// as the class, and the class's `---@field`s — declared in the *other*
    /// file — now resolve through the merged workspace ambient, the same
    /// environment diagnostics check the access against.
    #[test]
    fn a_class_instance_module_hovers_as_the_class_with_member_hover() {
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
        let member = at_files(&files, "x)", 0).expect("member hover");
        assert!(member.contains("Point.x"), "{member}");
        assert!(member.contains("number"), "{member}");
    }

    /// The carrier spelling closes identically (#56): the export crosses as
    /// the class it carries, so the binding hovers as the class name and
    /// members resolve through the same merged ambient.
    #[test]
    fn a_class_carrier_module_hovers_as_the_class_with_member_hover() {
        let files = [
            (
                "main.lua",
                "local p = require(\"point\")\nprint(p)\nprint(p.x)\n",
            ),
            (
                "point.lua",
                "---@class Point\n---@field x number\nlocal P = {}\nreturn P\n",
            ),
        ];
        let binding = at_files(&files, "p)", 0).expect("hover");
        assert!(binding.contains("local p: Point"), "{binding}");
        let member = at_files(&files, "x)", 0).expect("member hover");
        assert!(member.contains("Point.x"), "{member}");
        assert!(member.contains("number"), "{member}");
    }

    // === the ambient arm reaches a file-local receiver class too (#46) ====
    //
    // The defect: when the receiver's own class was declared in *this*
    // file, `class_of_name` succeeded and the old file-local branch's `?`
    // returned from the whole function on a miss — so an inherited member
    // whose parent lives in another file was never reachable, even though
    // the merged ambient (which already includes this file's own classes)
    // resolves it. Measured before the fix: `s.id` hovered `None`.

    #[test]
    fn a_member_inherited_from_a_parent_declared_in_another_file_hovers() {
        let files = [
            (
                "main.lua",
                "---@class Sub : Base\n---@field name string\n\n---@type Sub\nlocal s = nil\nprint(s.id)\n",
            ),
            ("other.lua", "---@class Base\n---@field id number\n"),
        ];
        let text = at_files(&files, "id)", 0).expect("hover");
        assert!(text.contains("Sub.id"), "{text}");
        assert!(text.contains("number"), "{text}");
    }

    /// Probing the other direction: a member neither `Sub` nor `Base`
    /// declares must still have no hover, even once the ambient arm is
    /// reachable for a file-local receiver.
    #[test]
    fn a_member_neither_the_class_nor_its_cross_file_parent_declares_has_no_hover() {
        let files = [
            (
                "main.lua",
                "---@class Sub : Base\n---@field name string\n\n---@type Sub\nlocal s = nil\nprint(s.nope)\n",
            ),
            ("other.lua", "---@class Base\n---@field id number\n"),
        ];
        assert_eq!(at_files(&files, "nope", 0), None);
    }

    // === the ambient arm's field description (#51) ========================

    #[test]
    fn an_ambient_members_field_description_survives_crossing_require() {
        let files = [
            ("main.lua", "local p = require(\"point\")\nprint(p.x)\n"),
            (
                "point.lua",
                "---@class Point\n---@field x number the x coordinate\nlocal P = {}\nreturn P\n",
            ),
        ];
        let text = at_files(&files, "x)", 0).expect("hover");
        assert!(text.contains("the x coordinate"), "{text}");
    }

    // === a bound generic reference resolves its type argument (#48) =======
    //
    // `---@type Box<number>` monomorphises at the reference site — the
    // checker's `lower.rs` does this for every use. The ambient arm used to
    // extract just the bare name (`sema::named_of` dropped `args`) and ask
    // for `Box`'s unbound shape, so `item` stayed the free `T`.

    #[test]
    fn a_bound_generic_carriers_member_resolves_the_bound_argument() {
        let files = [
            (
                "main.lua",
                "---@type Box<number>\nlocal b = nil\nprint(b.item)\n",
            ),
            ("box.lua", "---@class Box<T>\n---@field item T\n"),
        ];
        let text = at_files(&files, "item)", 0).expect("hover");
        assert!(text.contains("Box.item: number"), "{text}");
        assert!(!text.contains(": T"), "{text}");
    }

    /// M21 (round 6 review): the identical class referenced bare must hover
    /// `unknown`, not the literal free parameter name `T` — the answer this
    /// test asserted *before* the fix, and the answer `luabox check` never
    /// gave (the checker's own `class_shape_bound_export` erases a bare
    /// reference's free trailing parameters at the reference site; hover's
    /// member route used to skip that erasure entirely). Renamed from
    /// `an_unbound_generic_receiver_still_hovers_leniently`: "leniently"
    /// described the *old*, wrong behaviour this fix reverses.
    #[test]
    fn an_unbound_generic_receivers_free_parameter_hovers_as_unknown() {
        let files = [
            ("main.lua", "---@type Box\nlocal b = nil\nprint(b.item)\n"),
            ("box.lua", "---@class Box<T>\n---@field item T\n"),
        ];
        let text = at_files(&files, "item)", 0).expect("hover");
        assert!(text.contains("Box.item: unknown"), "{text}");
        assert!(!text.contains(": T"), "{text}");
    }

    // === a dynamic-access class agrees with the checker's leniency (#53) ==

    #[test]
    fn a_member_covered_by_a_declared_indexer_hovers_instead_of_declining() {
        let files = [
            (
                "main.lua",
                "local h = require(\"handlers\")\nprint(h.one)\n",
            ),
            (
                "handlers.lua",
                "---@class Handlers\n---@field [string] fun(): string\nlocal H = {}\nreturn H\n",
            ),
        ];
        let text = at_files(&files, "one)", 0).expect("hover");
        assert!(text.contains("Handlers.one"), "{text}");
    }

    /// Control: a class with no indexer/array part still declines an
    /// undeclared member — the leniency is specific to a dynamic-access
    /// class, not a general relaxation.
    #[test]
    fn a_member_on_a_class_with_no_indexer_still_has_no_hover() {
        let files = [
            ("main.lua", "local p = require(\"point\")\nprint(p.nope)\n"),
            (
                "point.lua",
                "---@class Point\n---@field x number\nlocal P = {}\nreturn P\n",
            ),
        ];
        assert_eq!(at_files(&files, "nope", 0), None);
    }

    // === an alias-typed class field (round 4 review R27) ==================
    //
    // Hover and completion both resolve a class field's member through the
    // merged ambient's `Ty` now (#56), and `Ty` has no dedicated alias
    // variant — an `---@alias` always expands to its underlying shape at
    // lowering time (`lower.rs`'s own doc). No test declared this shape on
    // either surface before; this pins hover's half.

    // === precedence-flip parity between hover's type and description (M2) =
    //
    // Round 6 review, finding M2: after the precedence flip, hover's *type*
    // (resolved through the merged ambient — the same authority
    // `luabox check` uses) and its *description*/goto-definition target
    // (resolved through `sema::locate_field`) could name two different
    // ancestors for the same field. `locate_field` walked the parent chain
    // breadth-first while `TypeEnv::collect_class` resolves depth-first
    // preorder; for `C : A, B` with `A : X` and both `X` and `B` declaring
    // `f`, BFS could return `B`'s value while the checker resolved `X`'s.

    /// The review's own repro, reproduced here: three declarations of `f`
    /// reachable from `C`, the checker's own merge (via the ambient) landing
    /// on `X`'s. All three answers a hover renders — the type, the
    /// description, and (in `goto_definition.rs`'s sibling test) the jump
    /// target — must agree on the same winner.
    #[test]
    fn hovers_type_and_description_agree_on_the_precedence_winner() {
        let src = "\
---@class X
---@field f string the f from X
---@class B
---@field f number the f from B
---@class A : X
---@class C : A, B

---@type C
local c = nil
print(c.f)
";
        let text = at(src, "f)", 0).expect("hover");
        assert!(
            text.contains("C.f: string"),
            "the type must agree with `luabox check`'s own resolution (X's): {text}"
        );
        assert!(
            text.contains("the f from X"),
            "the description must name the same winner as the type: {text}"
        );
        assert!(
            !text.contains("the f from B"),
            "B's declaration must not win: {text}"
        );
    }

    #[test]
    fn an_alias_typed_field_hovers_with_the_alias_expanded() {
        let src = "\
---@alias Direction \"up\"|\"down\"

---@class Compass
---@field dir Direction the facing direction

---@type Compass
local c = nil
print(c.dir)
";
        let text = at(src, "dir)", 0).expect("hover");
        assert!(text.contains("Compass.dir: \"up\"|\"down\""), "{text}");
        assert!(text.contains("the facing direction"), "{text}");
        // The alias name itself does not leak into the rendered type — `Ty`
        // has no alias variant, so it cannot round-trip the spelling.
        assert!(!text.contains("Direction"), "{text}");
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

    // === search_order cache wall time by ancestry depth (M57) =============

    /// M57's own real-numbers sweep (round 6 review): `search_order` — and
    /// its `may_declare_class` text scan — used to be recomputed from
    /// scratch on every `locate_field` call, even a repeat hover of the
    /// *same* class at the *same* revision. A representative stand-in for
    /// the review's 17 MB defs corpus (many padded "noise" classes, so the
    /// per-file scan has real bytes to walk) plus one linear ancestry chain
    /// of the probed depth. Manual — run explicitly with `cargo test -p
    /// luabox-lsp --release -- --ignored --nocapture
    /// search_order_cache_wall_time_by_depth`; the round 6 report carries
    /// the before/after numbers this prints. This probe backs no CI claim
    /// (round 8 review F10): M57's FUNCTIONAL pin is the non-ignored
    /// `search_order_cache_skips_the_may_declare_class_scan_on_a_repeat_call`
    /// in sema.rs — this one only puts wall-clock numbers on it by hand.
    #[test]
    #[ignore = "manual wall-clock measurement, see the doc comment"]
    fn search_order_cache_wall_time_by_depth() {
        use std::fmt::Write as _;
        for depth in [2usize, 10, 30, 60] {
            let mut files: Vec<(String, String)> = Vec::new();
            for i in 0..400 {
                let mut src = format!("---@class Noise{i}\n");
                for f in 0..20 {
                    let _ = writeln!(
                        src,
                        "---@field f{f} number field number {f} of Noise{i}, padding this \
                         declaration out the way a real defs file's documentation comments would"
                    );
                }
                files.push((format!("noise{i}.lua"), src));
            }
            let mut chain = String::from("---@class Depth0\n---@field item number\n");
            for i in 1..=depth {
                let _ = writeln!(chain, "---@class Depth{i} : Depth{}", i - 1);
            }
            let _ = write!(
                chain,
                "\n---@type Depth{depth}\nlocal x = nil\nprint(x.item)\n"
            );
            files.push(("main.lua".to_string(), chain));

            let root = root();
            let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
            host.set_root(root.clone());
            for (rel, text) in &files {
                host.apply_change(Change::SetFileText {
                    path: root.join(rel),
                    dialect: Dialect::Lua54,
                    text: text.clone(),
                });
            }
            let analysis = host.snapshot();
            let main_path = root.join("main.lua");
            let sema = FileSema::new(&analysis, &main_path).expect("sema");
            let exports = RequireExports::resolve(&analysis, &main_path, &RockSurfaces::default());
            let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
            let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
            let main_src = &files.last().expect("main.lua present").1;
            let offset = offset_of(main_src, "item)", 0);

            let first_start = std::time::Instant::now();
            hover(&sema, offset, &exports, &ambient, &analysis).expect("first hover");
            let first = first_start.elapsed();

            let repeat_start = std::time::Instant::now();
            for _ in 0..3 {
                hover(&sema, offset, &exports, &ambient, &analysis).expect("repeat hover");
            }
            let repeat = repeat_start.elapsed() / 3;

            eprintln!("depth={depth}: first={first:?} repeat(avg of 3)={repeat:?}");
        }
    }

    // === is_declared_alias_or_enum cache wall time by file count (H2) =====

    /// H2's own real-numbers sweep: `sema::is_declared_alias_or_enum` is
    /// `analysis.files().any(...)` — every project file's every annotation
    /// item's every tag, uncached — reached from `require_struct_fields` on
    /// every hover/completion against a `require`d binding whose explicit
    /// `---@type` names an alias or enum rather than a class (the M20 gate,
    /// `an_annotation_naming_an_alias_is_not_treated_as_the_structural_export`
    /// exercises the same gate for correctness). A representative stand-in
    /// for "a few hundred files with a sizeable defs set": padded noise
    /// classes (20 fields each, the same shape [`search_order_cache_wall_time_by_depth`]
    /// uses) so the per-file annotation scan has real tags to walk, plus one
    /// file whose `require`d binding is annotated `---@type Foo` naming an
    /// alias declared in a dedicated file. Manual — run explicitly with
    /// `cargo test -p luabox-lsp --release -- --ignored --nocapture
    /// is_declared_alias_or_enum_cache_wall_time_by_file_count`.
    #[test]
    #[ignore = "manual wall-clock measurement, see the doc comment"]
    fn is_declared_alias_or_enum_cache_wall_time_by_file_count() {
        use std::fmt::Write as _;
        for n in [100usize, 300, 600] {
            let mut files: Vec<(String, String)> = Vec::new();
            for i in 0..n {
                let mut src = format!("---@class Noise{i}\n");
                for f in 0..20 {
                    let _ = writeln!(
                        src,
                        "---@field f{f} number field number {f} of Noise{i}, padding this \
                         declaration out the way a real defs file's documentation comments would"
                    );
                }
                files.push((format!("noise{i}.lua"), src));
            }
            files.push((
                "foo_alias.lua".to_string(),
                "---@alias Foo string\n".to_string(),
            ));
            let main_src = "---@type Foo\nlocal m = require(\"m\")\nprint(m.x)\n".to_string();
            files.push((
                "m.lua".to_string(),
                "local M = {}\nM.x = 42\nreturn M\n".to_string(),
            ));
            files.push(("main.lua".to_string(), main_src.clone()));

            let root = root();
            let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
            host.set_root(root.clone());
            for (rel, text) in &files {
                host.apply_change(Change::SetFileText {
                    path: root.join(rel),
                    dialect: Dialect::Lua54,
                    text: text.clone(),
                });
            }
            let analysis = host.snapshot();
            let main_path = root.join("main.lua");
            let sema = FileSema::new(&analysis, &main_path).expect("sema");
            let exports = RequireExports::resolve(&analysis, &main_path, &RockSurfaces::default());
            let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
            let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
            let offset = offset_of(&main_src, "x)", 0);

            let first_start = std::time::Instant::now();
            let result = hover(&sema, offset, &exports, &ambient, &analysis);
            let first = first_start.elapsed();
            assert!(
                result.is_none(),
                "the gate must reject the structural export"
            );

            let repeat_start = std::time::Instant::now();
            for _ in 0..3 {
                let _ = hover(&sema, offset, &exports, &ambient, &analysis);
            }
            let repeat = repeat_start.elapsed() / 3;

            eprintln!("files={n}: first={first:?} repeat(avg of 3)={repeat:?}");
        }
    }
}
