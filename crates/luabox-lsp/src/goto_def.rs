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
    // 1. `require("mod")` → the module file (best-effort, project-relative).
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

/// Resolve `a.b.c` to `<root>/a/b/c.lua` or `<root>/a/b/c/init.lua`.
fn resolve_module(root: &Path, module: &str) -> Option<PathBuf> {
    let rel: PathBuf = module.split('.').collect();
    let direct = root.join(&rel).with_extension("lua");
    if direct.is_file() {
        return Some(direct);
    }
    let init = root.join(&rel).join("init.lua");
    init.is_file().then_some(init)
}

#[cfg(test)]
mod tests {
    use super::*;

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
