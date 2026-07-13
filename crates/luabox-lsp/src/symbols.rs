//! Document symbols: functions (including nested ones, as children of their
//! container), top-level locals, and `---@class` declarations from the
//! annotation harvest.
//!
//! [`workspace_symbols`] is the flat, cross-file counterpart used by
//! `workspace/symbol` (#131): it reuses [`document_symbols`]' derivation
//! (rather than re-walking the syntax tree) and flattens the resulting tree,
//! then adds `---@alias`/`---@enum` declarations, which are workspace-global
//! (#110) and so are not part of the per-file document-symbol tree.

use lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind, Uri};
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};
use luabox_syntax::luacats::Tag;
use rowan::TextRange;

use crate::sema::FileSema;
use crate::uri::path_to_uri;

/// The hierarchical symbol tree for one file.
#[must_use]
pub fn document_symbols(sema: &FileSema) -> Vec<DocumentSymbol> {
    let mut out = walk(&sema.root, sema, true);

    // `---@class` declarations, with their `@field`s as children.
    for (name, info) in sema.classes() {
        let block = info.tag.span;
        let fields = info
            .fields
            .iter()
            .filter_map(|field| {
                let luabox_syntax::luacats::FieldKey::Name(field_name) = &field.key else {
                    return None;
                };
                Some(symbol(
                    field_name.clone(),
                    SymbolKind::FIELD,
                    Some(crate::sema::render_type(&field.ty)),
                    sema.index.range(field.span.start..field.span.end),
                    sema.index.range(field.span.start..field.span.end),
                    Vec::new(),
                ))
            })
            .collect();
        out.push(symbol(
            name.to_string(),
            SymbolKind::CLASS,
            None,
            sema.index.range(block.start..block.end),
            sema.index.range(block.start..block.end),
            fields,
        ));
    }

    out.sort_by(|a, b| {
        (a.range.start.line, a.range.start.character)
            .cmp(&(b.range.start.line, b.range.start.character))
    });
    out
}

/// Every symbol in `sema` matching `query` — case-insensitive substring, an
/// empty query matches everything — the per-file search space for
/// `workspace/symbol`: everything [`document_symbols`] finds (functions,
/// including nested ones, top-level locals, and `---@class` declarations with
/// their fields), flattened, plus `---@alias` and `---@enum` declarations.
///
/// `---@alias` renders as `SymbolKind::INTERFACE`: LuaCATS aliases are a named
/// type definition, and the LSP has no dedicated "type alias" kind, so
/// `INTERFACE` (rather than e.g. `TYPE_PARAMETER`, which means something
/// narrower — a generic parameter) is the closest fit, matching what
/// `lua-language-server` itself advertises for aliases.
#[must_use]
pub fn workspace_symbols(sema: &FileSema, query: &str) -> Vec<SymbolInformation> {
    let uri = path_to_uri(&sema.path);
    let query = query.to_lowercase();
    let mut out = Vec::new();
    flatten(&document_symbols(sema), &uri, &query, &mut out);
    for item in sema.items() {
        for tag in &item.block.tags {
            match tag {
                Tag::Alias(a) if !a.name.is_empty() && name_matches(&a.name, &query) => {
                    out.push(symbol_information(
                        a.name.clone(),
                        SymbolKind::INTERFACE,
                        Location {
                            uri: uri.clone(),
                            range: sema.index.range(a.span.start..a.span.end),
                        },
                    ));
                }
                Tag::Enum(e) if !e.name.is_empty() && name_matches(&e.name, &query) => {
                    out.push(symbol_information(
                        e.name.clone(),
                        SymbolKind::ENUM,
                        Location {
                            uri: uri.clone(),
                            range: sema.index.range(e.span.start..e.span.end),
                        },
                    ));
                }
                _ => {}
            }
        }
    }
    out
}

/// Depth-first flatten of a document-symbol tree into `SymbolInformation`,
/// keeping only names matching `query` (already lower-cased).
fn flatten(symbols: &[DocumentSymbol], uri: &Uri, query: &str, out: &mut Vec<SymbolInformation>) {
    for sym in symbols {
        if name_matches(&sym.name, query) {
            out.push(symbol_information(
                sym.name.clone(),
                sym.kind,
                Location {
                    uri: uri.clone(),
                    range: sym.range,
                },
            ));
        }
        if let Some(children) = &sym.children {
            flatten(children, uri, query, out);
        }
    }
}

/// Case-insensitive substring match; `query` must already be lower-cased.
fn name_matches(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(query)
}

#[allow(
    deprecated,
    reason = "SymbolInformation::deprecated must be initialised"
)]
fn symbol_information(name: String, kind: SymbolKind, location: Location) -> SymbolInformation {
    SymbolInformation {
        name,
        kind,
        tags: None,
        deprecated: None,
        location,
        container_name: None,
    }
}

/// Collect symbols under `node`. `top_level` gates plain locals: only the
/// chunk's own `local`s become symbols; function/loop-body locals do not.
fn walk(node: &SyntaxNode, sema: &FileSema, top_level: bool) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    for child in node.children() {
        match child.kind() {
            SyntaxKind::FUNCTION_DECL_STMT => {
                if let Some(decl) = ast::FunctionDeclStmt::cast(child.clone()) {
                    let kind = if decl.name().is_some_and(|n| n.is_method()) {
                        SymbolKind::METHOD
                    } else {
                        SymbolKind::FUNCTION
                    };
                    let name = decl.name().map_or_else(
                        || "?".to_string(),
                        |n| {
                            let segments: Vec<String> =
                                n.segments().map(|s| s.text().to_string()).collect();
                            segments.join(if n.is_method() { ":" } else { "." })
                        },
                    );
                    let selection = decl
                        .name()
                        .map_or_else(|| child.text_range(), |n| n.syntax().text_range());
                    out.push(node_symbol(sema, &child, name, kind, selection));
                    continue;
                }
            }
            SyntaxKind::LOCAL_FUNCTION_STMT => {
                if let Some(decl) = ast::LocalFunctionStmt::cast(child.clone()) {
                    let (name, selection) = decl.name().map_or_else(
                        || ("?".to_string(), child.text_range()),
                        |t| (t.text().to_string(), t.text_range()),
                    );
                    out.push(node_symbol(
                        sema,
                        &child,
                        name,
                        SymbolKind::FUNCTION,
                        selection,
                    ));
                    continue;
                }
            }
            SyntaxKind::LOCAL_STMT if top_level => {
                if let Some(local) = ast::LocalStmt::cast(child.clone()) {
                    let is_function = matches!(
                        local.values().and_then(|v| v.exprs().next()),
                        Some(ast::Expr::Function(_))
                    );
                    for name in local.names().filter_map(|n| n.name()) {
                        let kind = if is_function {
                            SymbolKind::FUNCTION
                        } else {
                            SymbolKind::VARIABLE
                        };
                        out.push(node_symbol(
                            sema,
                            &child,
                            name.text().to_string(),
                            kind,
                            name.text_range(),
                        ));
                    }
                    // Nested functions inside the initializer still count.
                    if is_function && let Some(last) = out.last_mut() {
                        last.children = Some(walk(&child, sema, false));
                    }
                    continue;
                }
            }
            _ => {}
        }
        // Not a symbol-bearing statement: recurse (blocks, if/while bodies,
        // expressions) so nested functions are found, but drop top-level
        // status once inside anything.
        out.extend(walk(
            &child,
            sema,
            top_level && child.kind() == SyntaxKind::BLOCK,
        ));
    }
    out
}

/// A symbol for `node` whose children are the symbols found inside it.
fn node_symbol(
    sema: &FileSema,
    node: &SyntaxNode,
    name: String,
    kind: SymbolKind,
    selection: TextRange,
) -> DocumentSymbol {
    let children = walk(node, sema, false);
    symbol(
        name,
        kind,
        None,
        sema.index
            .range(usize::from(node.text_range().start())..usize::from(node.text_range().end())),
        sema.index
            .range(usize::from(selection.start())..usize::from(selection.end())),
        children,
    )
}

#[allow(deprecated, reason = "DocumentSymbol::deprecated must be initialised")]
fn symbol(
    name: String,
    kind: SymbolKind,
    detail: Option<String>,
    range: lsp_types::Range,
    selection_range: lsp_types::Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: (!children.is_empty()).then_some(children),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    /// Build a single-file `FileSema` for `text` at an absolute test path.
    fn sema(text: &str) -> FileSema {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let path = if cfg!(windows) {
            std::path::PathBuf::from(r"C:\ws\main.lua")
        } else {
            std::path::PathBuf::from("/ws/main.lua")
        };
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: text.to_string(),
        });
        FileSema::new(&host.snapshot(), &path).expect("sema")
    }

    #[test]
    fn covers_classes_fields_functions_aliases_and_enums() {
        let source = "\
---@class Shape
---@field kind string

---@alias Id string

---@enum Direction

function M.helper() end
";
        let sema = sema(source);
        let symbols = workspace_symbols(&sema, "");
        let by_name = |name: &str| {
            symbols
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("missing `{name}` in {symbols:?}"))
        };
        assert_eq!(by_name("Shape").kind, SymbolKind::CLASS);
        assert_eq!(by_name("kind").kind, SymbolKind::FIELD);
        assert_eq!(by_name("Id").kind, SymbolKind::INTERFACE);
        assert_eq!(by_name("Direction").kind, SymbolKind::ENUM);
        assert_eq!(by_name("M.helper").kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn query_matches_case_insensitively_as_a_substring() {
        let sema = sema("---@class Shape\nlocal top = 1\n");
        let symbols = workspace_symbols(&sema, "sha");
        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0].name, "Shape");
    }

    #[test]
    fn non_matching_query_returns_empty() {
        let sema = sema("---@class Shape\nlocal top = 1\n");
        assert!(workspace_symbols(&sema, "zzz_no_such_symbol").is_empty());
    }

    #[test]
    fn nested_functions_are_flattened_into_the_results() {
        let sema = sema("local function outer()\n  local function inner() end\nend\n");
        let symbols = workspace_symbols(&sema, "inner");
        assert_eq!(symbols.len(), 1, "{symbols:?}");
        assert_eq!(symbols[0].name, "inner");
        assert_eq!(symbols[0].kind, SymbolKind::FUNCTION);
    }
}
