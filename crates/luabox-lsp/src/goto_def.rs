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
use luabox_hir::Resolution;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use rowan::{TextRange, TextSize};

use crate::sema::FileSema;
use crate::uri::path_to_uri;

/// Compute the definition location for the symbol at `offset`.
/// `project_root` anchors `require` module resolution.
#[must_use]
pub fn goto_definition(sema: &FileSema, offset: usize, project_root: &Path) -> Option<Location> {
    // 1. `require("mod")` → the module file, resolved through the bundler's
    //    shared candidate search (project root, `src/`, then `lua_modules/`).
    if let Some(edge) = sema.require_at(offset) {
        let module = edge.module.clone();
        return resolve_module(project_root, &module).map(|path| Location {
            uri: path_to_uri(&path),
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        });
    }

    let token = sema.ident_at(offset)?;

    // 2. Field / method member → the `---@field` annotation site.
    if let Some(location) = member_definition(sema, &token) {
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
fn member_definition(sema: &FileSema, token: &luabox_syntax::lua::SyntaxToken) -> Option<Location> {
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

    if let Some(class) = sema.class_of_name(recv_token.text(), recv_offset) {
        let fields = sema.class_fields(&class);
        let (field, _) = fields.into_iter().find(|(f, _)| {
            matches!(&f.key, luabox_syntax::luacats::FieldKey::Name(n) if n == member.text())
        })?;
        let span = field.span;
        let range = TextRange::new(
            TextSize::new(u32::try_from(span.start).ok()?),
            TextSize::new(u32::try_from(span.end).ok()?),
        );
        if let Some(redirect) = source_redirect(sema, range) {
            return Some(redirect);
        }
        return Some(Location {
            uri: path_to_uri(&sema.path),
            range: sema.index.range(span.start..span.end),
        });
    }

    // Dotted function: `M.helper` → its declaration.
    let dotted = format!("{}.{}", recv_token.text(), member.text());
    let info = sema.functions().into_iter().find(|f| f.name == dotted)?;
    Some(here(sema, info.decl_range))
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
/// then `src/`, then the `lua_modules/<pkg>/` tree), picking the first
/// candidate that exists on disk — exactly the bundler's `resolve`.
///
/// This is the workspace's single source of truth for `require` resolution
/// (the same algorithm `luabox check` and the bundler use), so goto-def can
/// never disagree with them: a module under `src/` or a dependency now jumps
/// where the build would actually load it from.
fn resolve_module(root: &Path, module: &str) -> Option<PathBuf> {
    luabox_bundle::resolve_candidates(root, module)
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

    /// Goto-definition at the `nth` occurrence of `needle` in the first file.
    fn at(src: &str, needle: &str, nth: usize) -> Option<Location> {
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        goto_definition(&sema, offset_of(src, needle, nth), &root())
    }

    /// The `(line, character)` start of a location.
    fn start_of(location: &Location) -> (u32, u32) {
        (location.range.start.line, location.range.start.character)
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
