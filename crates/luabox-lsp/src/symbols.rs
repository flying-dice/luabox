//! Document symbols: functions (including nested ones, as children of their
//! container), top-level locals, and `---@class` declarations from the
//! annotation harvest.

use lsp_types::{DocumentSymbol, SymbolKind};
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};
use rowan::TextRange;

use crate::sema::FileSema;

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
