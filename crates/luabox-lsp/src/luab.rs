//! `.luab` shape-type resolution: parse diagnostics (see
//! [`crate::diagnostics::lb_diagnostics`]), same-file and cross-file hover /
//! goto definition, `.lua`-annotation resolution against the ambient package
//! scope (SHAPES-V2.md), and completion of scope type names.
//!
//! `.luab` files never enter the Lua analysis host — they are parsed directly
//! with [`shape::parse`] from the text the server tracks (overlay over disk).

use std::path::{Path, PathBuf};

use lsp_types::{CompletionItem, CompletionItemKind};
use luabox_syntax::lua::{
    SyntaxKind as LuaSyntaxKind, SyntaxNode as LuaSyntaxNode, SyntaxToken as LuaSyntaxToken,
};
use luabox_syntax::shape::{
    self, ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken,
    ast::{AstNode, NamedType, ShapeFile},
};
use luabox_types::shape::ShapeScope;
use rowan::{TextRange, TextSize, TokenAtOffset};

/// The `.luab` type-vocabulary builtins offered in completion (SHAPES-V2.md):
/// primitives plus the `Vec`/`HashMap`/`Option`/`Result` constructors.
const BUILTIN_TYPES: &[&str] = &[
    "number", "integer", "string", "boolean", "unknown", "any", "nil", "Vec", "HashMap", "Option",
    "Result",
];

/// The declaration of the type named under the cursor, in the *same* file:
/// `(name token range of the declaration, the declaration's source — any
/// directly preceding `---` doc-comment lines, then the `type ... = ...`
/// text)`.
#[must_use]
pub fn definition(text: &str, offset: usize) -> Option<(TextRange, String)> {
    let parse = shape::parse(text);
    let root = parse.syntax();
    let token = ident_at(&root, offset)?;
    let name = token.text();

    let file = ShapeFile::cast(root)?;
    for item in file.items() {
        if item.name().as_deref() == Some(name) {
            let node = item.syntax();
            let name_token = first_ident(node)?;
            return Some((name_token.text_range(), decl_source(text, node)));
        }
    }
    None
}

/// A type declaration resolved through the ambient package scope: the
/// absolute `.luab` file that declares it, that file's current text, the
/// declared NAME token's range within it, and the doc-prefixed declaration
/// source (see [`decl_source`]).
pub struct ScopedDecl {
    pub file: PathBuf,
    pub text: String,
    pub name_range: TextRange,
    pub source: String,
}

/// Resolve `fq` against the ambient package scope: fetch the declaring
/// file's current text via `read_text` (overlay-over-disk; `None` if it
/// cannot be read — e.g. a stale scope pointing at a since-deleted file),
/// then narrow the whole-declaration range to its NAME token, mirroring
/// same-file [`definition`].
#[must_use]
pub fn resolve_scoped(
    scope: &ShapeScope,
    root: &Path,
    fq: &str,
    read_text: impl Fn(&Path) -> Option<String>,
) -> Option<ScopedDecl> {
    let shape = scope.get(fq)?;
    let file = root.join(&shape.file);
    let text = read_text(&file)?;
    let parsed = shape::parse(&text);
    let shape_file = ShapeFile::cast(parsed.syntax())?;
    let node = shape_file
        .items()
        .map(|item| item.syntax().clone())
        .find(|node| {
            let r = node.text_range();
            usize::from(r.start()) == shape.range.start && usize::from(r.end()) == shape.range.end
        })?;
    let name_token = first_ident(&node)?;
    let source = decl_source(&text, &node);
    Some(ScopedDecl {
        file,
        text,
        name_range: name_token.text_range(),
        source,
    })
}

/// The dotted reference under the cursor in a `.luab` file: the full
/// `path()` of the enclosing `TYPE_REF` when the token sits inside one
/// (excluding generic arguments, cursor may be on any segment), else the
/// bare identifier — plus its range (the hover/goto anchor).
#[must_use]
pub fn dotted_ref_at(text: &str, offset: usize) -> Option<(String, TextRange)> {
    let parse = shape::parse(text);
    let root = parse.syntax();
    let token = ident_at(&root, offset)?;
    if let Some(named) = token.parent().and_then(NamedType::cast) {
        return Some((named.path(), named.syntax().text_range()));
    }
    Some((token.text().to_string(), token.text_range()))
}

/// Derive `path`'s dot-separated namespace from its location under one of
/// `shape_paths` (SHAPES-V2.md): `shapes/love/graphics.luab` → `love.graphics`.
/// Falls back to the bare file stem when `path` is not under any configured
/// shape path. Duplicated from `luabox_types`' private equivalent (not
/// exported from that crate) — used only to build the sibling-short-name
/// fallback for cross-file `.luab` resolution.
#[must_use]
pub fn namespace_of(shape_paths: &[PathBuf], path: &Path) -> String {
    let Some(shape_path) = shape_paths.iter().find(|p| path.starts_with(p)) else {
        return path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
    };
    let rel = path.strip_prefix(shape_path).unwrap_or(path);
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if let Some(last) = parts.last_mut()
        && let Some(stem) = last.strip_suffix(".luab")
    {
        *last = stem.to_string();
    }
    parts.join(".")
}

/// `ns` + `.` + `name` (bare `name` for an empty namespace) — mirrors
/// `luabox_types`' private FQ-naming convention.
#[must_use]
pub fn fq_name(ns: &str, name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}.{name}")
    }
}

/// The dotted type name (`geometry.Point`) around `offset` inside a `---`
/// doc-comment token in a `.lua` file (`----`, a prose demotion, does not
/// count) — the identifier-and-dot run containing the cursor, plus its
/// range. Used for LuaCATS `---@type` / `---@param` / `---@return`
/// positions naming a shape type (SHAPES-V2.md): the annotation carries no
/// new tags, so any dotted run in a doc comment is a candidate.
#[must_use]
pub fn dotted_ident_in_lua_comment(
    root: &LuaSyntaxNode,
    offset: usize,
) -> Option<(String, TextRange)> {
    let token = lua_comment_at(root, offset)?;
    let text = token.text();
    if !is_doc_comment(text) {
        return None;
    }
    let base = usize::from(token.text_range().start());
    let local = offset.saturating_sub(base).min(text.len());
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'.';
    let mut s = local;
    while s > 0 && is_word(bytes[s - 1]) {
        s -= 1;
    }
    let mut e = local;
    while e < bytes.len() && is_word(bytes[e]) {
        e += 1;
    }
    let trimmed = text[s..e].trim_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    let trim_left = text[s..e].len() - text[s..e].trim_start_matches('.').len();
    let start = base + s + trim_left;
    let end = start + trimmed.len();
    Some((
        trimmed.to_string(),
        TextRange::new(u32_size(start)?, u32_size(end)?),
    ))
}

/// Whether `offset` sits in a LuaCATS type-annotation position inside a
/// `---` doc comment: right after `@type`, `@return`, or a completed
/// `@param <name> ` — the positions from which a shape type name may be
/// typed (SHAPES-V2.md consumes types through standard annotation positions
/// only, so there is no dedicated tag to anchor on).
#[must_use]
pub fn in_lua_type_position(root: &LuaSyntaxNode, offset: usize) -> bool {
    let Some(token) = lua_comment_at(root, offset) else {
        return false;
    };
    let text = token.text();
    if !is_doc_comment(text) {
        return false;
    }
    let base = usize::from(token.text_range().start());
    let local = offset.saturating_sub(base).min(text.len());
    let before = &text[..local];
    let rest = before.trim_start_matches("---").trim_start();
    if let Some(after) = rest
        .strip_prefix("@type")
        .or_else(|| rest.strip_prefix("@return"))
    {
        return after.is_empty()
            || (after.starts_with(char::is_whitespace) && only_type_chars(after));
    }
    let Some(after_kw) = rest.strip_prefix("@param") else {
        return false;
    };
    if !after_kw.starts_with(char::is_whitespace) {
        return false;
    }
    let named = after_kw.trim_start();
    let Some(sep) = named.find(char::is_whitespace) else {
        return false;
    };
    only_type_chars(&named[sep..])
}

/// Completion items for a `.luab` file: builtin type names, sibling types
/// declared in this file, and every fully-qualified name in the ambient
/// package scope.
#[must_use]
pub fn completion(text: &str, scope: &ShapeScope) -> Vec<CompletionItem> {
    let mut items: std::collections::BTreeMap<String, CompletionItem> =
        std::collections::BTreeMap::new();
    for name in BUILTIN_TYPES {
        items.insert(
            (*name).to_string(),
            CompletionItem {
                label: (*name).to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..CompletionItem::default()
            },
        );
    }
    if let Some(file) = ShapeFile::cast(shape::parse(text).syntax()) {
        for item in file.items() {
            let Some(name) = item.name() else { continue };
            items.insert(
                name.clone(),
                CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some(format!("type {name}")),
                    ..CompletionItem::default()
                },
            );
        }
    }
    for name in scope.types.keys() {
        items.insert(
            name.clone(),
            CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::CLASS),
                detail: Some(format!("type {name}")),
                ..CompletionItem::default()
            },
        );
    }
    items.into_values().collect()
}

// === shared helpers ========================================================

fn ident_at(root: &ShapeSyntaxNode, offset: usize) -> Option<ShapeSyntaxToken> {
    let offset = TextSize::new(u32::try_from(offset).ok()?);
    if offset > root.text_range().end() {
        return None;
    }
    let pick = |t: ShapeSyntaxToken| (t.kind() == ShapeSyntaxKind::IDENT).then_some(t);
    match root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(t),
        TokenAtOffset::Between(l, r) => pick(l).or_else(|| pick(r)),
    }
}

fn first_ident(node: &ShapeSyntaxNode) -> Option<ShapeSyntaxToken> {
    node.children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| t.kind() == ShapeSyntaxKind::IDENT)
}

/// The declaration source for `node`: its own text, extended backward to
/// include any `---` doc-comment lines directly preceding it (no blank line
/// separating them — SHAPES-V2.md comment conventions).
fn decl_source(text: &str, node: &ShapeSyntaxNode) -> String {
    let start = doc_start_before(node).unwrap_or_else(|| node.text_range().start());
    text[usize::from(start)..usize::from(node.text_range().end())].to_string()
}

/// The byte offset of the earliest line in the `---` doc-comment run
/// directly preceding `node` (no blank line breaking the run) — `None` when
/// there is no such run.
fn doc_start_before(node: &ShapeSyntaxNode) -> Option<TextSize> {
    let node_start = node.text_range().start();
    let root = node.ancestors().last()?;
    let preceding: Vec<ShapeSyntaxToken> = root
        .descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .take_while(|t| t.text_range().start() < node_start)
        .collect();
    let mut start = None;
    for t in preceding.into_iter().rev() {
        match t.kind() {
            ShapeSyntaxKind::WHITESPACE
                if t.text().bytes().filter(|&b| b == b'\n').count() <= 1 => {}
            ShapeSyntaxKind::DOC_COMMENT => start = Some(t.text_range().start()),
            _ => break,
        }
    }
    start
}

fn lua_comment_at(root: &LuaSyntaxNode, offset: usize) -> Option<LuaSyntaxToken> {
    let offset_ts = TextSize::new(u32::try_from(offset).ok()?);
    if offset_ts > root.text_range().end() {
        return None;
    }
    let pick = |t: LuaSyntaxToken| (t.kind() == LuaSyntaxKind::COMMENT).then_some(t);
    match root.token_at_offset(offset_ts) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => pick(t),
        TokenAtOffset::Between(l, r) => pick(l).or_else(|| pick(r)),
    }
}

/// A `---` line (not `----`, which LuaCATS demotes to a plain comment).
fn is_doc_comment(text: &str) -> bool {
    text.starts_with("---") && !text.starts_with("----")
}

fn only_type_chars(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_whitespace() || c == '.' || c.is_alphanumeric() || c == '_')
}

fn u32_size(n: usize) -> Option<TextSize> {
    Some(TextSize::new(u32::try_from(n).ok()?))
}
