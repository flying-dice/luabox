//! Goto type-definition: from a value (or `self`) to the declaration site of
//! the `---@class` / `---@alias` / `---@enum` that names its type.
//!
//! The type name a cursor refers to comes from three sources, most
//! authoritative first: the enclosing method's class for `self`, the binding's
//! `---@type` / `---@param` annotation, then the display inference's
//! [`Ty::Named`] for a value whose class/enum type was inferred (e.g. a
//! `---@return Foo` call result bound to a local). Aliases only survive in
//! annotations — inference expands them away — so an alias-typed value must
//! reach us through the annotation path.
//!
//! The declaration itself is workspace-global (#85/#110): named types live in
//! any file, so once we have the name we scan every file's annotations for the
//! matching `---@class` / `---@alias` / `---@enum` tag, target file first.
//! When the type is a primitive, `unknown`, or an inferred anonymous table —
//! anything that is not a declared named type — nothing matches and we return
//! `None` rather than jump somewhere wrong.

use lsp_types::Location;
use luabox_db::Analysis;
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxToken;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::luacats::{Span, Tag};
use luabox_types::ty::Ty;
use rowan::TextRange;

use crate::sema::{self, FileSema};
use crate::uri::path_to_uri;

/// The type-definition location for the symbol at `offset`: the declaration of
/// the named type its value carries, or `None` when that type is not a declared
/// class/alias/enum. `analysis` supplies the other workspace files (declarations
/// are workspace-global) and the display inference; `target` is the already-built
/// view of the file under the cursor.
#[must_use]
pub fn goto_type_definition(
    analysis: &Analysis,
    target: &FileSema,
    offset: usize,
) -> Option<Location> {
    let name = type_name_at(analysis, target, offset)?;
    declaration_of(analysis, target, &name)
}

/// The name of the type the symbol at `offset` carries, if it resolves to a
/// named type. `self` in a method is answered from the method's class; a
/// local/upvalue from its annotation, falling back to the display inference.
fn type_name_at(analysis: &Analysis, target: &FileSema, offset: usize) -> Option<String> {
    let token = target.ident_at(offset)?;
    let resolution = target.resolution_at(offset);

    // The binding the cursor resolves to (a local, or an upvalue captured from
    // an enclosing scope — `self` inside a nested closure is one).
    let binding_id = match &resolution {
        Some(Resolution::Local(id) | Resolution::Upvalue { binding: id, .. }) => Some(*id),
        _ => None,
    };

    // `self` in a `:` method → the class the method is declared on. The
    // `SelfParam` kind is what distinguishes it from a parameter literally
    // named `self` in a plain function.
    if binding_id.is_some_and(|id| target.binding(id).kind == BindingKind::SelfParam)
        && let Some(name) = enclosing_method_class(&token)
    {
        return Some(name);
    }

    let id = binding_id?;
    let binding = target.binding(id);

    // An annotated type wins over inference: it is authoritative, and it is the
    // only path that preserves an alias name (inference expands aliases).
    if let Some(ty) = target.binding_type(binding)
        && let Some(name) = sema::named_of(&ty)
    {
        return Some(name);
    }

    // Otherwise the display inference: a value whose class/enum type was
    // inferred (an alias would already be expanded, so only `Ty::Named`
    // reaches us here).
    inferred_named(analysis, target, binding.range)
}

/// The class name a `function Class:method` (or `function M.Class:method`)
/// declares its implicit `self` on: the path segment immediately before the
/// method name.
fn enclosing_method_class(token: &SyntaxToken) -> Option<String> {
    let decl = token
        .parent()?
        .ancestors()
        .find_map(ast::FunctionDeclStmt::cast)?;
    let name = decl.name()?;
    if !name.is_method() {
        return None;
    }
    let segments: Vec<SyntaxToken> = name.segments().collect();
    let class = segments.get(segments.len().checked_sub(2)?)?;
    Some(class.text().to_string())
}

/// The inferred type name of the binding declared at `range`, when the display
/// inference reified it to a declared class/enum ([`Ty::Named`]).
fn inferred_named(analysis: &Analysis, target: &FileSema, range: TextRange) -> Option<String> {
    let start = usize::from(range.start());
    let end = usize::from(range.end());
    let types = analysis.binding_types(&target.path)?;
    let binding = types
        .bindings()
        .iter()
        .find(|b| b.range.start == start && b.range.end == end)?;
    match &binding.ty {
        Ty::Named(name) => Some(name.clone()),
        _ => None,
    }
}

/// The declaration location of the named type `name`, searched across the
/// workspace (target file first). `None` when no file declares it.
fn declaration_of(analysis: &Analysis, target: &FileSema, name: &str) -> Option<Location> {
    if let Some(span) = declared_span(target, name) {
        return Some(location(target, span));
    }
    for path in analysis.files() {
        if path == target.path {
            continue;
        }
        if let Some(file) = FileSema::new(analysis, path)
            && let Some(span) = declared_span(&file, name)
        {
            return Some(location(&file, span));
        }
    }
    None
}

/// The span of a `---@class` / `---@alias` / `---@enum` named `name` in one
/// file, if it declares one.
fn declared_span(sema: &FileSema, name: &str) -> Option<Span> {
    for item in sema.items() {
        for tag in &item.block.tags {
            let span = match tag {
                Tag::Class(c) if c.name == name => c.span,
                Tag::Alias(a) if a.name == name => a.span,
                Tag::Enum(e) if e.name == name => e.span,
                _ => continue,
            };
            return Some(span);
        }
    }
    None
}

fn location(sema: &FileSema, span: Span) -> Location {
    Location {
        uri: path_to_uri(&sema.path),
        range: sema.index.range(span.start..span.end),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    /// Build an analysis over `files`, returning the snapshot and the absolute
    /// path of the first file (the request target).
    fn analyze(files: &[(&str, &str)]) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let mut first = None;
        for (rel, text) in files {
            let path = root.join(rel);
            first.get_or_insert_with(|| path.clone());
            host.apply_change(Change::SetFileText {
                path,
                dialect: Dialect::Lua54,
                text: (*text).to_string(),
            });
        }
        (host.snapshot(), first.expect("at least one file"))
    }

    /// Byte offset of the first occurrence of `needle` in `text`.
    fn offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    fn run(files: &[(&str, &str)], offset: usize) -> Option<Location> {
        let (analysis, path) = analyze(files);
        let target = FileSema::new(&analysis, &path).expect("target sema");
        goto_type_definition(&analysis, &target, offset)
    }

    #[test]
    fn typed_local_jumps_to_its_class_declaration() {
        let src = "\
---@class Point
---@field x number

---@type Point
local p = nil
print(p)
";
        // Cursor on `p` inside `print(p)`.
        let offset = src
            .rfind('p')
            .and_then(|i| src[..i].rfind("print").map(|_| i));
        let offset = offset.expect("p in print");
        let loc = run(&[("main.lua", src)], offset).expect("type definition");
        // The `---@class Point` tag is on line 0.
        assert_eq!(loc.range.start.line, 0, "{loc:?}");
    }

    #[test]
    fn alias_typed_local_jumps_to_the_alias_declaration() {
        let src = "\
---@alias Id string

---@type Id
local key = nil
print(key)
";
        let offset = offset_of(src, "print(key") + "print(".len();
        let loc = run(&[("main.lua", src)], offset).expect("type definition");
        // The `---@alias Id` tag is on line 0.
        assert_eq!(loc.range.start.line, 0, "{loc:?}");
    }

    #[test]
    fn self_in_a_method_jumps_to_the_class() {
        let src = "\
---@class Widget
local Widget = {}

function Widget:render()
  return self
end
";
        let offset = offset_of(src, "return self") + "return ".len();
        let loc = run(&[("main.lua", src)], offset).expect("type definition");
        assert_eq!(loc.range.start.line, 0, "{loc:?}");
    }

    #[test]
    fn class_declaration_lives_in_another_file() {
        let files = &[
            ("use.lua", "---@type Point\nlocal p = nil\nprint(p)\n"),
            ("point.lua", "---@class Point\n---@field x number\n"),
        ];
        let src = "---@type Point\nlocal p = nil\nprint(p)\n";
        let offset = offset_of(src, "print(p") + "print(".len();
        let loc = run(files, offset).expect("type definition");
        assert!(
            loc.uri.as_str().ends_with("point.lua"),
            "{}",
            loc.uri.as_str()
        );
    }

    #[test]
    fn primitive_typed_local_has_no_type_definition() {
        let src = "---@type number\nlocal n = 1\nprint(n)\n";
        let offset = offset_of(src, "print(n") + "print(".len();
        assert!(run(&[("main.lua", src)], offset).is_none());
    }

    #[test]
    fn inferred_class_value_jumps_to_the_class() {
        // `make` is annotated `---@return Point`; the inference reifies `p`
        // to `Point` even though `p` itself carries no annotation.
        let src = "\
---@class Point
---@field x number

---@return Point
local function make() end

local p = make()
print(p)
";
        let offset = offset_of(src, "print(p") + "print(".len();
        let loc = run(&[("main.lua", src)], offset).expect("type definition");
        assert_eq!(loc.range.start.line, 0, "{loc:?}");
    }
}
