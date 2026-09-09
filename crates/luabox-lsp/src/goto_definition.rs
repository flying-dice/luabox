//! Goto definition: locals/upvalues via HIR resolution, class fields to
//! their `---@field` annotation site, functions to their declaration, and
//! `require("mod")` strings to the module file.
//!
//! A declaration whose doc block carries `---@source <path>[:line[:col]]`
//! redirects to that location instead (LuaLS `core/jump-source.lua`):
//! `scheme://` locations are used verbatim, relative paths resolve against
//! the annotated file's directory, and — matching LuaLS — the target is
//! *not* checked for existence.

use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use lsp_types::{Location, Position, Range};
use luabox_db::Analysis;
use luabox_hir::Resolution;
use luabox_syntax::lua::Dialect;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use rowan::{TextRange, TextSize};

use crate::merged_ambient::MergedAmbient;
use crate::requires::{self, RequireExports};
use crate::sema::{self, FileSema};
use crate::uri::path_to_uri;

/// Compute the definition location for the symbol at `offset`.
/// `project_root` anchors `require` module resolution, and `dialect` — the
/// project's edition — selects the `lua_modules/share/lua/<X.Y>/` version
/// directory of a luarocks tree. `analysis`, `exports` and `ambient` back
/// the member route (#54): the same shared `require` resolution and merged
/// workspace environment hover, completion and signature help resolve a
/// receiver's class through (#56).
#[must_use]
pub fn definition(
    sema: &FileSema,
    offset: usize,
    project_root: &Path,
    dialect: Dialect,
    analysis: &Analysis,
    exports: &RequireExports,
    ambient: &MergedAmbient,
) -> Option<Location> {
    // 1. `require("mod")` → the module file, resolved through the bundler's
    //    shared candidate search (project root, `src/`, then `lua_modules/`).
    if let Some(edge) = sema.require_at(offset) {
        let module = edge.module.clone();
        return resolve_module(project_root, &module, dialect).map(|path| Location {
            uri: path_to_uri(&path),
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        });
    }

    let token = sema.ident_at(offset)?;

    // 2. Field / method member → the `---@field` annotation site.
    if let Some(location) = member_definition(
        sema,
        &token,
        analysis,
        exports,
        ambient,
        project_root,
        dialect,
    ) {
        return Some(location);
    }

    // 3. Name resolution: local / upvalue → the binding's declaration.
    match sema.resolution_at(offset) {
        Some(Resolution::Local(id) | Resolution::Upvalue { binding: id, .. }) => {
            return Some(here(sema, sema.binding(id).range));
        }
        Some(Resolution::Global(name)) => {
            // A declared function first, then any global definition site.
            if let Some(info) = sema.functions().into_iter().find(|f| f.name == name) {
                return Some(here(sema, info.decl_range));
            }
            if let Some((_, range)) = sema
                .global_defs()
                .into_iter()
                .find(|(defined, _)| *defined == name)
            {
                return Some(here(sema, range));
            }
        }
        None => {
            // On a declaration already: answer with itself so clients show it.
            if let Some(id) = sema.binding_decl_at(offset) {
                return Some(here(sema, sema.binding(id).range));
            }
        }
    }

    None
}

/// `recv.field` / `recv:method` → the `@field` tag span of the receiver's
/// class, or a dotted function's declaration site.
///
/// The receiver's class reference is resolved through the workspace ambient
/// (#56, via [`requires::receiver_type`]) — declared in this file or any
/// other, bound against its own type arguments exactly as the checker binds
/// the same reference (#48) — so the existence check agrees with
/// hover/completion/signature help (#46, #47, #50). A field's *existence*
/// never depends on which concrete type a generic parameter was bound to
/// (`item: T` is there whether `T` is bound to `number` or left free), so
/// #48 changes nothing observable here — routed through
/// [`crate::merged_ambient::MergedAmbient::class_members_of`] anyway, so a
/// future divergence between the bound and unbound shapes cannot open one.
/// The declaration site itself is then located by class *name* with
/// [`sema::locate_field`] (#54), which searches every project file's own
/// `---@field` annotations and walks the parent chain across files: the
/// merged ambient's shape has no span or file to jump to, only a type, and
/// a `---@field` tag's location does not depend on the reference's bound
/// argument either.
fn member_definition(
    sema: &FileSema,
    token: &luabox_syntax::lua::SyntaxToken,
    analysis: &Analysis,
    exports: &RequireExports,
    ambient: &MergedAmbient,
    project_root: &Path,
    dialect: Dialect,
) -> Option<Location> {
    let parent = token.parent()?;
    let (receiver, member) = match parent.kind() {
        SyntaxKind::FIELD_EXPR => {
            let field = ast::FieldExpr::cast(parent)?;
            let name = field.field_name()?;
            if name.text_range() != token.text_range() {
                return None;
            }
            (field.base(), name)
        }
        SyntaxKind::METHOD_CALL_EXPR => {
            let call = ast::MethodCallExpr::cast(parent)?;
            let name = call.method_name()?;
            if name.text_range() != token.text_range() {
                return None;
            }
            (call.receiver(), name)
        }
        _ => return None,
    };
    let Some(ast::Expr::Name(recv_name)) = receiver else {
        return None;
    };
    let recv_token = recv_name.name()?;
    let recv_offset = usize::from(recv_token.text_range().start());

    if let Some(binding) = sema.visible_binding_named(recv_token.text(), recv_offset)
        && let Some(ty) = requires::receiver_type(sema, exports, binding)
        && let Some(class) = sema::named_of(&ty)
        && ambient.class_members_of(&ty).is_some_and(|shape| {
            shape.fields.contains_key(member.text())
                || !shape.indexers.is_empty()
                || shape.array.is_some()
        })
        && let Some(found) = ambient.locate_field(analysis, &sema.path, &class, member.text())
    {
        let span = found.span;
        let range = TextRange::new(
            TextSize::new(u32::try_from(span.start).ok()?),
            TextSize::new(u32::try_from(span.end).ok()?),
        );
        // The target file's own `FileSema` — its `LineIndex` is what turns
        // the byte span into a position, so it has to be the file the span
        // came from. A different path this `Analysis` cannot build a sema
        // for has no line index to offer, and falling back to the *cursor's*
        // would publish this file's line/character for the other file's URI
        // (round 8 clean-code audit, coupling finding 2): decline instead.
        let field_sema = if found.path == sema.path {
            None
        } else {
            Some(FileSema::new(analysis, &found.path)?)
        };
        let target = field_sema.as_ref().unwrap_or(sema);
        if let Some(redirect) = source_redirect(target, range) {
            return Some(redirect);
        }
        return Some(Location {
            uri: path_to_uri(&found.path),
            range: target.index.range(span.start..span.end),
        });
    }

    // Structural module exports have no class annotation to locate. Follow
    // the require binding and the module's returned table, not its import alias.
    if let Some(binding) = sema.visible_binding_named(recv_token.text(), recv_offset)
        && let Some((module, fields)) =
            requires::require_struct_fields(sema, exports, ambient, analysis, binding)
        && fields.contains_key(member.text())
        && let Some(path) = luabox_bundle::resolve_candidates(project_root, module, dialect)
            .into_iter()
            .find(|path| analysis.file_text(path).is_some())
        && let Some(target) = FileSema::new(analysis, &path)
        && let Some(range) = exported_member_range(&target, member.text())
    {
        return Some(here(&target, range));
    }

    // Dotted function: `M.helper` → its declaration.
    let dotted = format!("{}.{}", recv_token.text(), member.text());
    let info = sema.functions().into_iter().find(|f| f.name == dotted)?;
    Some(here(sema, info.decl_range))
}

/// Follow only the module-level return, never a nested function's return.
/// Compare bindings so a shadowed table cannot steal the definition.
fn exported_member_range(sema: &FileSema, member: &str) -> Option<TextRange> {
    let block = ast::SourceFile::cast(sema.root.clone())?.block()?;
    let returned = block.stmts().find_map(|stmt| match stmt {
        ast::Stmt::Return(ret) => ret.exprs()?.exprs().next(),
        _ => None,
    })?;
    let ast::Expr::Name(name) = returned else {
        return None;
    };
    let name = name.name()?;
    let binding =
        sema.visible_binding_named(name.text(), usize::from(name.text_range().start()))?;
    let dotted = format!("{}.{}", name.text(), member);
    let method = format!("{}:{}", name.text(), member);
    sema.functions().into_iter().find_map(|info| {
        if info.name != dotted && info.name != method {
            return None;
        }
        let owner =
            sema.visible_binding_named(name.text(), usize::from(info.decl_range.start()))?;
        (owner.range == binding.range).then_some(info.decl_range)
    })
}

fn here(sema: &FileSema, range: TextRange) -> Location {
    if let Some(redirect) = source_redirect(sema, range) {
        return redirect;
    }
    Location {
        uri: path_to_uri(&sema.path),
        range: sema
            .index
            .range(usize::from(range.start())..usize::from(range.end())),
    }
}

// === `---@source` redirect ================================================

/// If the declaration at `range` is governed by a `---@source` tag, the
/// annotated location (a zero-width range, like LuaLS's jump target).
fn source_redirect(sema: &FileSema, range: TextRange) -> Option<Location> {
    let text = sema.source_tag_covering(range)?;
    let (path, line, col) = parse_source_location(text);
    let uri = if let Some(uri) = as_uri(&path) {
        uri
    } else {
        let p = Path::new(&path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            normalize(&sema.path.parent()?.join(p))
        };
        path_to_uri(&abs)
    };
    // `@source` lines are 1-based (default 1), columns 0-based (default 0):
    // LuaLS emits `positionOf(doc.line - 1, doc.char)`.
    let pos = Position::new(line.saturating_sub(1), col);
    Some(Location {
        uri,
        range: Range::new(pos, pos),
    })
}

/// Split `path[:line[:col]]` following LuaLS's
/// `fullSource:match('^(.-):?(%d*):?(%d*)$')` — up to two all-digit suffix
/// segments peel off the end (first is the 1-based line, second the 0-based
/// column), so drive letters (`C:/x.c`) and `scheme://` prefixes survive.
/// One deliberate divergence: LuaLS's optional colons let it eat trailing
/// digits with *no* separator (`file2` → path `file`, line 2); here a digit
/// segment only peels across an explicit `:`, so `---@source 123` stays the
/// path `123` — the saner reading of a pathological input.
fn parse_source_location(text: &str) -> (String, u32, u32) {
    let mut rest = text;
    let mut nums: Vec<&str> = Vec::new();
    for _ in 0..2 {
        match rest.rsplit_once(':') {
            Some((head, tail)) if tail.bytes().all(|b| b.is_ascii_digit()) => {
                nums.push(tail);
                rest = head;
            }
            _ => break,
        }
    }
    nums.reverse();
    let line = nums.first().and_then(|n| n.parse().ok()).unwrap_or(1);
    let col = nums.get(1).and_then(|n| n.parse().ok()).unwrap_or(0);
    (rest.to_string(), line, col)
}

/// Parse `path` as a URI when it starts with a scheme. Mirrors LuaLS's
/// "scheme of two or more characters" rule (`furi.split` + `#scheme >= 2`),
/// which keeps single-letter Windows drives (`C:/x`) as filesystem paths.
fn as_uri(path: &str) -> Option<lsp_types::Uri> {
    let (scheme, _) = path.split_once(':')?;
    if scheme.len() < 2
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    {
        return None;
    }
    lsp_types::Uri::from_str(path).ok()
}

/// Lexically normalize `.` / `..` components (the target need not exist, so
/// no filesystem canonicalization).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve `module` to its file through the bundler's shared candidate
/// ordering ([`luabox_bundle::resolve_candidates`], SPEC.md §7: project root,
/// then `src/`, then the `lua_modules/` trees — flat and luarocks), picking the first
/// candidate that exists on disk — exactly the bundler's `resolve`.
///
/// This is the workspace's single source of truth for `require` resolution
/// (the same algorithm `luabox check` and the bundler use), so goto-def can
/// never disagree with them: a module under `src/` or a dependency now jumps
/// where the build would actually load it from.
fn resolve_module(root: &Path, module: &str, dialect: Dialect) -> Option<PathBuf> {
    luabox_bundle::resolve_candidates(root, module, dialect)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    /// The workspace root every test file lives under.
    fn root() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\ws" } else { "/ws" })
    }

    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
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

    /// Byte offset just inside the `nth` (0-based) occurrence of `needle`.
    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    /// Goto-definition at the `nth` occurrence of `needle` in `files`' first
    /// file, with `exports`/`ambient` built exactly as the server builds
    /// them (#54, #56).
    fn at_files(files: &[(&str, &str)], needle: &str, nth: usize) -> Option<Location> {
        at_files_from(files, files[0].0, needle, nth)
    }

    /// Goto-definition at the `nth` occurrence of `needle` in the first file.
    fn at(src: &str, needle: &str, nth: usize) -> Option<Location> {
        at_files(&[("main.lua", src)], needle, nth)
    }

    #[test]
    fn imported_plain_table_method_resolves_in_src_directory() {
        let location = at_files(
            &[
                ("main.lua", "local g = require('greeter')\ng:greet()\n"),
                (
                    "src/greeter.lua",
                    "local M = {}\nfunction M:greet() return 'hello' end\nreturn M\n",
                ),
            ],
            "greet()",
            0,
        )
        .expect("imported method definition");
        assert_eq!(location.uri, path_to_uri(&root().join("src/greeter.lua")));
        assert_eq!(
            location.range,
            Range::new(Position::new(1, 11), Position::new(1, 16))
        );
    }

    #[test]
    fn imported_plain_table_does_not_jump_to_shadowed_exporter() {
        let location = at_files(
            &[
                ("main.lua", "local g = require('greeter')\ng.greet()\n"),
                ("greeter.lua", "local M = {}\nfunction M.greet() end\nlocal M = {}\nfunction M.greet() end\nreturn M\n"),
            ], "greet()", 0,
        ).expect("second exported binding");
        assert_eq!(location.range.start, Position::new(3, 11));
    }

    #[test]
    fn imported_plain_table_function_uses_exporting_files_range() {
        let location = at_files(
            &[
                (
                    "main.lua",
                    "local g = require('greeter')\nprint(g.greet())\n",
                ),
                (
                    "greeter.lua",
                    "-- module\n\nlocal M = {}\nfunction M.greet() return 'hello' end\nreturn M\n",
                ),
            ],
            "greet()",
            0,
        )
        .expect("imported function definition");
        assert_eq!(location.uri, path_to_uri(&root().join("greeter.lua")));
        assert_eq!(
            location.range,
            Range::new(Position::new(3, 11), Position::new(3, 16))
        );
    }

    /// [`at_files`] with the cursor in a named file rather than the first one
    /// loaded (#70) — see `hover.rs`'s `at_in` for why load order and the
    /// cursor have to be separable to reach that shape at all.
    fn at_files_from(
        files: &[(&str, &str)],
        cursor: &str,
        needle: &str,
        nth: usize,
    ) -> Option<Location> {
        let src = files
            .iter()
            .find(|(rel, _)| *rel == cursor)
            .expect("the cursor file must be one of `files`")
            .1;
        let (analysis, _) = analyze(files);
        let path = root().join(cursor);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let exports =
            RequireExports::resolve(&analysis, &path, &luabox_types::RockSurfaces::default());
        let base = luabox_types::stdlib_defs(luabox_syntax::lua::Dialect::Lua54);
        let ambient = MergedAmbient::build(base, &analysis.project_types(), &[]);
        definition(
            &sema,
            offset_of(src, needle, nth),
            &root(),
            Dialect::Lua54,
            &analysis,
            &exports,
            &ambient,
        )
    }

    /// #70, the goto-definition half of `hover.rs`'s
    /// `a_cross_file_split_fields_type_and_description_name_one_declaration`:
    /// the jump must land on the declaration the merge resolved the type
    /// from, in the other file, not on the cursor's own losing one — three
    /// surfaces, one declaration.
    #[test]
    fn goto_definition_follows_a_cross_file_splits_merge_winner() {
        let location = at_files_from(
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
        .expect("definition");
        assert!(
            location.uri.to_string().ends_with("a.lua"),
            "must land in the file the merge took `f` from: {location:?}"
        );
        assert_eq!(start_of(&location), (1, 3), "a.lua's own `---@field f` tag");
    }

    /// The goto-definition half of `hover.rs`'s
    /// `a_carrier_attached_member_still_shows_its_parents_description`
    /// (round 9 review, thread on `sema.rs:952`): a member the owner attaches
    /// by writing the function still jumps to the parent's `---@field`, the
    /// only declaration of it anywhere.
    #[test]
    fn goto_definition_reaches_a_carrier_attached_members_parent_declaration() {
        let location = at_files_from(
            &[
                (
                    "base.lua",
                    "---@class Base\n---@field greet fun() the greeting from Base\n",
                ),
                (
                    "main.lua",
                    "---@class Sub : Base\nlocal S = {}\nfunction S.greet() end\n\n\
                     ---@type Sub\nlocal s = nil\nprint(s.greet)\n",
                ),
            ],
            "main.lua",
            "greet)",
            0,
        )
        .expect("definition");
        assert!(
            location.uri.to_string().ends_with("base.lua"),
            "the parent's file is where `greet` is declared: {location:?}"
        );
        assert_eq!(
            start_of(&location),
            (1, 3),
            "base.lua's own `---@field` tag"
        );
    }

    /// The goto-definition half of `hover.rs`'s
    /// `a_carrier_attached_member_keeps_a_ruled_out_ancestors_description`
    /// (round 10 review): a declaration the merge did not take
    /// the type from is still the jump target when it sits on the **owner's
    /// own ancestry**, because that is the member's override lineage. Move
    /// the same declaration onto a sibling branch (`Leaf : Mid, Other`) and
    /// there is no target at all — `sema`'s
    /// `locate_field_roots_the_walk_at_the_owner_not_the_queried_class`.
    #[test]
    fn goto_definition_reaches_a_ruled_out_declaration_on_the_owners_own_ancestry() {
        let location = at_files_from(
            &[
                (
                    "mid.lua",
                    "---@class Mid : Other\nlocal M = {}\nfunction M.f() end\nreturn M\n",
                ),
                (
                    "other.lua",
                    "---@class Other\n---@field f string the f from Other\n",
                ),
                (
                    "main.lua",
                    "---@class Leaf : Mid\n\n\
                     ---@type Leaf\nlocal l = nil\nprint(l.f)\n",
                ),
            ],
            "main.lua",
            "f)",
            0,
        )
        .expect("definition");
        assert!(
            location.uri.to_string().ends_with("other.lua"),
            "the lineage's own earlier link is the only declaration: {location:?}"
        );
        assert_eq!(
            start_of(&location),
            (1, 3),
            "other.lua's own `---@field` tag"
        );
    }

    /// The `(line, character)` start of a location.
    fn start_of(location: &Location) -> (u32, u32) {
        (location.range.start.line, location.range.start.character)
    }

    /// M2 (round 6 review): goto-definition's target must agree with
    /// hover's type and description on the same precedence winner — the
    /// review's own repro (`hover.rs`'s
    /// `hovers_type_and_description_agree_on_the_precedence_winner` pins
    /// the other two). `locate_field`'s parent-chain walk used to be
    /// breadth-first, which could land on `B`'s `---@field f` (line 3) even
    /// though the checker's own depth-first merge resolves `X`'s (line 1).
    #[test]
    fn goto_definition_agrees_with_hovers_precedence_winner() {
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
        let location = at(src, "f)", 0).expect("definition");
        assert_eq!(
            location.range.start.line, 1,
            "must land on X's own `---@field f` (line 1), not B's (line 3): {location:?}"
        );
    }

    #[test]
    fn a_global_assignment_target_is_a_definition_site() {
        let src = "answer = 42
print(answer)
";
        let location = at(src, "answer)", 0).expect("definition");
        assert_eq!(start_of(&location), (0, 0));
    }

    #[test]
    fn a_cursor_on_a_local_declaration_answers_with_itself() {
        let src = "local thing = 1
print(thing)
";
        let location = at(src, "thing = 1", 0).expect("definition");
        assert_eq!(start_of(&location), (0, 6));
    }

    #[test]
    fn an_identifier_with_no_symbol_has_no_definition() {
        // A table-constructor key names no binding, global, or member.
        assert!(at("local t = { key = 1 }\n", "key", 0).is_none());
    }

    #[test]
    fn an_undeclared_global_has_no_definition() {
        assert!(at("print(never_defined)\n", "never_defined", 0).is_none());
    }

    #[test]
    fn a_method_call_jumps_to_the_class_field_annotation() {
        let src = "\
---@class Greeter
---@field greet fun(self: Greeter): string

---@type Greeter
local g = nil
g:greet()
";
        let location = at(src, "greet()", 0).expect("definition");
        // The `---@field greet` tag site on line 1.
        assert_eq!(location.range.start.line, 1);
    }

    // === the workspace ambient reaches goto-definition too (#54) ==========
    //
    // Hover now names `Point.x` for a cross-file member (#46); goto-def used
    // to stay silent for the identical receiver, because it only ever
    // resolved through `class_of_name` — file-local. `locate_field` gives it
    // the same cross-file, parent-chain-aware reach hover and completion
    // already have.

    #[test]
    fn a_method_call_on_a_class_declared_in_another_file_jumps_to_its_field_tag() {
        let files = [
            ("main.lua", "---@type Greeter\nlocal g = nil\ng:greet()\n"),
            (
                "other.lua",
                "---@class Greeter\n---@field greet fun(self: Greeter): string\n",
            ),
        ];
        let location = at_files(&files, "greet()", 0).expect("definition");
        assert!(location.uri.as_str().ends_with("other.lua"), "{location:?}");
        assert_eq!(location.range.start.line, 1);
    }

    /// The cross-file inheritance shape (#46's fixture): the field is
    /// declared on a *parent* class living in a third file.
    #[test]
    fn a_member_inherited_from_a_parent_in_another_file_jumps_to_the_parent() {
        let files = [
            (
                "main.lua",
                "---@class Sub : Base\n---@field name string\n\n---@type Sub\nlocal s = nil\nprint(s.id)\n",
            ),
            ("base.lua", "---@class Base\n---@field id number\n"),
        ];
        let location = at_files(&files, "id)", 0).expect("definition");
        assert!(location.uri.as_str().ends_with("base.lua"), "{location:?}");
    }

    /// Probing the other direction: a member neither class declares still
    /// has no definition once cross-file resolution is wired in.
    #[test]
    fn a_member_no_cross_file_ancestor_declares_has_no_definition() {
        let files = [
            ("main.lua", "---@type Greeter\nlocal g = nil\ng:nope()\n"),
            (
                "other.lua",
                "---@class Greeter\n---@field greet fun(self: Greeter): string\n",
            ),
        ];
        assert!(at_files(&files, "nope()", 0).is_none());
    }

    // === a bound generic reference still agrees with the checker (#48) ====
    //
    // A field's existence does not depend on which type a generic parameter
    // is bound to, so a bound receiver must jump exactly like an unbound
    // one — this is the "does not open a new divergence" probe for
    // goto-definition, alongside the type-rendering probes in `hover.rs`,
    // `completion.rs`, and `signature_help.rs`.

    #[test]
    fn a_bound_generic_receivers_member_still_jumps_to_its_field_tag() {
        let files = [
            (
                "main.lua",
                "---@type Box<number>\nlocal b = nil\nprint(b.item)\n",
            ),
            ("box.lua", "---@class Box<T>\n---@field item T\n"),
        ];
        let location = at_files(&files, "item)", 0).expect("definition");
        assert!(location.uri.as_str().ends_with("box.lua"), "{location:?}");
    }

    #[test]
    fn a_member_access_on_a_non_name_receiver_has_no_definition() {
        assert!(at("local v = f().field\n", "field", 0).is_none());
    }

    #[test]
    fn a_field_receiver_resolves_to_its_own_binding_not_the_field() {
        let src = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p.x)
";
        // Cursor on `p`, not on `x`: the member route must decline.
        let location = at(src, "p.x", 0).expect("definition");
        assert_eq!(start_of(&location), (4, 6));
    }

    #[test]
    fn a_dotted_function_call_jumps_to_its_declaration() {
        let src = "\
local M = {}
function M.helper(n) return n end
M.helper(1)
";
        let location = at(src, "helper(1)", 0).expect("definition");
        assert_eq!(start_of(&location), (1, 11));
    }

    #[test]
    fn a_source_redirect_uses_a_uri_verbatim() {
        let src = "---@source file:///opt/impl.c:12:4\nlocal function f() end\nf()\n";
        let location = at(src, "f()", 0).expect("definition");
        assert_eq!(location.uri.as_str(), "file:///opt/impl.c");
        assert_eq!(start_of(&location), (11, 4));
    }

    #[test]
    fn a_source_redirect_keeps_an_absolute_path() {
        let absolute = if cfg!(windows) {
            r"C:/native/impl.c"
        } else {
            "/native/impl.c"
        };
        let src = format!("---@source {absolute}:3\nlocal function f() end\nf()\n");
        let location = at(&src, "f()", 0).expect("definition");
        assert!(
            location.uri.as_str().ends_with("/native/impl.c"),
            "{location:?}"
        );
        assert_eq!(start_of(&location), (2, 0));
    }

    #[test]
    fn a_source_redirect_resolves_a_relative_path_against_the_file() {
        let src = "---@source ../native/impl.c\nlocal function f() end\nf()\n";
        let location = at(src, "f()", 0).expect("definition");
        assert!(
            location.uri.as_str().ends_with("/native/impl.c"),
            "{location:?}"
        );
        // Line defaults to 1 → zero-based line 0.
        assert_eq!(start_of(&location), (0, 0));
    }

    #[test]
    fn a_source_redirect_survives_being_the_only_statement_in_the_file() {
        // The annotated statement shares its text range with the enclosing
        // `BLOCK`; goto-def on the declaration must still redirect.
        let src = "---@source impl.c\nlocal function f() end\n";
        let location = at(src, "f() end", 0).expect("definition");
        assert!(location.uri.as_str().ends_with("/impl.c"), "{location:?}");
        assert_eq!(start_of(&location), (0, 0));
    }

    #[test]
    fn a_require_string_jumps_to_the_start_of_the_module_file() {
        // Nothing exists on disk under the fake root, so resolution declines.
        assert!(at("local m = require(\"other\")\n", "other", 0).is_none());
    }

    #[test]
    fn normalize_keeps_a_leading_parent_segment_it_cannot_pop() {
        // Nothing to pop, so the `..` is preserved verbatim.
        assert_eq!(
            normalize(Path::new("../vendor/impl.c")),
            PathBuf::from("../vendor/impl.c")
        );
        // A second `..` then pops the first, lexically.
        assert_eq!(
            normalize(Path::new("../../vendor/impl.c")),
            PathBuf::from("vendor/impl.c")
        );
    }

    #[test]
    fn source_location_splits_path_line_and_col() {
        assert_eq!(
            parse_source_location("impl/native.c:12:4"),
            ("impl/native.c".to_string(), 12, 4)
        );
        assert_eq!(
            parse_source_location("impl/native.c:12"),
            ("impl/native.c".to_string(), 12, 0)
        );
        assert_eq!(
            parse_source_location("impl/native.c"),
            ("impl/native.c".to_string(), 1, 0)
        );
    }

    #[test]
    fn source_location_keeps_drive_letters_and_schemes() {
        // The drive colon is followed by non-digits, so it never peels.
        assert_eq!(
            parse_source_location("C:/src/impl.c:100:8"),
            ("C:/src/impl.c".to_string(), 100, 8)
        );
        assert_eq!(
            parse_source_location("file:///proj/impl.c:7"),
            ("file:///proj/impl.c".to_string(), 7, 0)
        );
    }

    #[test]
    fn uri_detection_requires_a_two_char_scheme() {
        // LuaLS treats a `scheme://` (scheme length >= 2) as a URI verbatim
        // and a single drive letter as a path.
        assert!(as_uri("file:///proj/impl.c").is_some());
        assert!(as_uri("C:/src/impl.c").is_none());
        assert!(as_uri("impl/native.c").is_none());
    }

    #[test]
    fn normalize_collapses_dot_segments() {
        assert_eq!(
            normalize(Path::new("/proj/src/../vendor/./impl.c")),
            PathBuf::from("/proj/vendor/impl.c")
        );
    }
}
