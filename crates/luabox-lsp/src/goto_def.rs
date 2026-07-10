//! Goto definition: locals/upvalues via HIR resolution, class fields to
//! their `---@field` annotation site, functions to their declaration, and
//! `require("mod")` strings to the module file.

use std::path::{Path, PathBuf};

use lsp_types::{Location, Position, Range};
use luabox_hir::Resolution;
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use rowan::TextRange;

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
    Location {
        uri: path_to_uri(&sema.path),
        range: sema
            .index
            .range(usize::from(range.start())..usize::from(range.end())),
    }
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
