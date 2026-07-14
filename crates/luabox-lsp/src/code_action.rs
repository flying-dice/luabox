//! Type-driven code actions and refactors (SPEC.md §8, #129), offered
//! alongside the machine-applicable lint quick-fixes of [`crate::server`].
//!
//! Four actions, each conservative — offered only when the node shape is
//! unambiguous, so an accepted edit is always syntactically valid Lua/LuaCATS:
//!
//! - **add-missing-field** (`quickfix`) — on an `LB0302` "missing required
//!   field" diagnostic, insert the named field into the offending table
//!   literal with a `nil, -- TODO` stub. The field name is parsed from the
//!   diagnostic message (the checker reports it in backticks); the action
//!   references the diagnostic it resolves.
//! - **annotate-local-from-inference** (`refactor.rewrite`) — on an
//!   unannotated `local x = <expr>`, insert a `---@type <inferred>` line above
//!   it, taking the type from the display inference (the inlay-hint surface).
//! - **generate-class-from-literal** (`refactor.rewrite`) — on
//!   `local x = { a = 1, b = "s" }`, insert a `---@class`/`---@field` block
//!   above the local (which annotates it, luals-style), one field per named
//!   entry with its inferred type.
//! - **dot-colon-convert** (`refactor.rewrite`) — flip a `function T.m(self,
//!   ...)` declaration to `function T:m(...)` (dropping the explicit `self`),
//!   or the reverse `function T:m(...)` → `function T.m(self, ...)`.
//!
//! All edits are computed as a [`WorkspaceEdit`] over the single request file.
//! Inserted annotation lines match the target statement's leading whitespace.

use std::collections::HashMap;
use std::fmt::Write as _;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Diagnostic, NumberOrString, TextEdit, Uri,
    WorkspaceEdit,
};
use luabox_db::BindingTypes;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode, SyntaxToken};
use luabox_types::ty::Ty;
use rowan::{NodeOrToken, TextRange};

use crate::sema::FileSema;

/// Gather every applicable type/refactor action for the byte range
/// `start..end` of the file. `inferred` is the display-mode binding inference
/// (for the two annotation actions); `type_diags` are the file's published
/// type diagnostics (for add-missing-field's `LB0302`). Returns `[]` when
/// nothing applies.
#[must_use]
pub fn type_actions(
    sema: &FileSema,
    inferred: Option<&BindingTypes>,
    type_diags: &[Diagnostic],
    uri: &Uri,
    start: usize,
    end: usize,
) -> Vec<CodeActionOrCommand> {
    let mut out = Vec::new();
    add_missing_field(sema, type_diags, uri, start, end, &mut out);
    annotate_local(sema, inferred, uri, start, end, &mut out);
    generate_class(sema, inferred, uri, start, end, &mut out);
    dot_colon_convert(sema, uri, start, end, &mut out);
    out
}

// === add-missing-field ===================================================

/// One `quickfix` per `LB0302` diagnostic overlapping the request: insert the
/// missing field into its table literal as a `nil, -- TODO` stub, on its own
/// line before the closing brace.
fn add_missing_field(
    sema: &FileSema,
    type_diags: &[Diagnostic],
    uri: &Uri,
    start: usize,
    end: usize,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let src = sema.index.text();
    for diag in type_diags {
        if code_str(diag) != Some("LB0302") {
            continue;
        }
        let d_start = sema.index.offset(diag.range.start);
        let d_end = sema.index.offset(diag.range.end);
        // Inclusive overlap so a bare caret at either edge still offers it.
        if d_end < start || d_start > end {
            continue;
        }
        let Some(name) = first_backtick(&diag.message) else {
            continue;
        };
        // The diagnostic's primary span is the whole table literal.
        let Some(table) = sema
            .root
            .descendants()
            .filter(|n| n.kind() == SyntaxKind::TABLE_EXPR)
            .find(|n| {
                usize::from(n.text_range().start()) == d_start
                    && usize::from(n.text_range().end()) == d_end
            })
            .and_then(ast::TableExpr::cast)
        else {
            continue;
        };
        let Some((range, new_text)) = missing_field_edit(src, &table, &name) else {
            continue;
        };
        out.push(edit_action(
            uri,
            format!("Add missing field `{name}`"),
            CodeActionKind::QUICKFIX,
            Some(vec![diag.clone()]),
            vec![TextEdit {
                range: sema.index.range(range),
                new_text,
            }],
        ));
    }
}

/// The byte range to replace and the replacement text that inserts
/// `name = nil, -- TODO` on its own line before the table's closing brace,
/// keeping the result valid whether the table is empty, single-line, or
/// multi-line.
fn missing_field_edit(
    src: &str,
    table: &ast::TableExpr,
    name: &str,
) -> Option<(std::ops::Range<usize>, String)> {
    let node = table.syntax();
    let open = node_token(node, SyntaxKind::L_BRACE)?;
    let close = last_node_token(node, SyntaxKind::R_BRACE)?;
    let open_end = usize::from(open.text_range().end());
    let close_start = usize::from(close.text_range().start());
    if close_start < open_end {
        return None;
    }

    let stmt_indent = line_indent(src, usize::from(open.text_range().start()));
    #[expect(
        clippy::string_slice,
        reason = "open_end and close_start are rowan token offsets (char boundaries) and close_start >= open_end is checked above"
    )]
    let body = &src[open_end..close_start];

    // Field indent: reuse an existing field's line indent when the table is
    // already laid out one-per-line, else one step in from the statement.
    let last_field = table.fields().last();
    let field_indent = match &last_field {
        Some(field) => {
            let field_start = usize::from(field.syntax().text_range().start());
            if line_start(src, field_start) == line_start(src, open_end) {
                format!("{stmt_indent}    ")
            } else {
                line_indent(src, field_start).to_string()
            }
        }
        None => format!("{stmt_indent}    "),
    };

    let stub = format!("\n{field_indent}{name} = nil, -- TODO\n{stmt_indent}");
    if body.trim().is_empty() {
        // Empty table: replace the whitespace between the braces.
        Some((open_end..close_start, stub))
    } else {
        // Non-empty: insert after the last content, adding a separator when
        // the previous field has no trailing `,`/`;`.
        let trimmed = body.trim_end();
        let content_end = open_end + trimmed.len();
        let needs_sep = !matches!(trimmed.chars().last(), Some(',' | ';'));
        let sep = if needs_sep { "," } else { "" };
        Some((content_end..close_start, format!("{sep}{stub}")))
    }
}

// === annotate-local-from-inference =======================================

/// A `refactor.rewrite` inserting `---@type <inferred>` above an unannotated
/// single-name `local x = <expr>` whose type the display inference resolved.
fn annotate_local(
    sema: &FileSema,
    inferred: Option<&BindingTypes>,
    uri: &Uri,
    start: usize,
    end: usize,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(inferred) = inferred else {
        return;
    };
    let Some((local, name_tok)) = simple_local(sema, start, end) else {
        return;
    };
    if has_type_annotation(sema, &name_tok) {
        return;
    }
    let Some(ty) = inferred_binding_ty(inferred, &name_tok) else {
        return;
    };
    if matches!(ty, Ty::Unknown) {
        return;
    }
    let src = sema.index.text();
    let stmt_start = usize::from(local.syntax().text_range().start());
    let ls = line_start(src, stmt_start);
    #[expect(
        clippy::string_slice,
        reason = "stmt_start is a rowan range start and ls follows a `\\n` byte, so both are char boundaries"
    )]
    let indent = &src[ls..stmt_start];
    let new_text = format!("{indent}---@type {ty}\n");
    out.push(edit_action(
        uri,
        format!("Annotate `{}` with inferred type", name_tok.text()),
        CodeActionKind::REFACTOR_REWRITE,
        None,
        vec![TextEdit {
            range: sema.index.range(ls..ls),
            new_text,
        }],
    ));
}

// === generate-class-from-literal =========================================

/// A `refactor.rewrite` inserting a `---@class`/`---@field` block above an
/// unannotated `local x = { ... }`, one field per named entry with its
/// inferred type. The block annotates the local (luals class-on-assignment).
fn generate_class(
    sema: &FileSema,
    inferred: Option<&BindingTypes>,
    uri: &Uri,
    start: usize,
    end: usize,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some((local, name_tok)) = simple_local(sema, start, end) else {
        return;
    };
    let Some(ast::Expr::Table(table)) = local.values().and_then(|v| v.exprs().next()) else {
        return;
    };
    // Named entries only (`a = 1`); array items and dynamic keys are skipped.
    let named: Vec<(String, Option<ast::Expr>)> = table
        .fields()
        .filter_map(|field| match field {
            ast::TableField::Name(f) => Some((f.name()?.text().to_string(), f.value())),
            _ => None,
        })
        .collect();
    if named.is_empty() {
        return;
    }
    if has_type_annotation(sema, &name_tok) {
        return;
    }

    // Prefer the inferred field types (the display inference reifies the
    // literal to a `Ty::Table`); fall back to a per-value AST guess.
    let field_types: HashMap<String, String> = inferred
        .and_then(|bt| inferred_binding_ty(bt, &name_tok))
        .and_then(|ty| match ty {
            Ty::Table(table) => Some(
                table
                    .fields
                    .iter()
                    .map(|(n, f)| (n.clone(), f.ty.to_string()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();

    let class = capitalize(name_tok.text());
    let src = sema.index.text();
    let stmt_start = usize::from(local.syntax().text_range().start());
    let ls = line_start(src, stmt_start);
    #[expect(
        clippy::string_slice,
        reason = "stmt_start is a rowan range start and ls follows a `\\n` byte, so both are char boundaries"
    )]
    let indent = &src[ls..stmt_start];
    let mut block = format!("{indent}---@class {class}\n");
    for (name, value) in &named {
        let ty = field_types
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_field_type(value.as_ref()));
        let _ = writeln!(block, "{indent}---@field {name} {ty}");
    }
    out.push(edit_action(
        uri,
        format!("Generate `---@class {class}` from table literal"),
        CodeActionKind::REFACTOR_REWRITE,
        None,
        vec![TextEdit {
            range: sema.index.range(ls..ls),
            new_text: block,
        }],
    ));
}

// === dot-colon-convert ===================================================

/// A `refactor.rewrite` flipping a `function` declaration between the dotted
/// form with an explicit `self` and the `:` method form. Only the applicable
/// direction is offered.
fn dot_colon_convert(
    sema: &FileSema,
    uri: &Uri,
    start: usize,
    end: usize,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(node) = sema
        .root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::FUNCTION_DECL_STMT)
        .filter(|n| {
            ast::FunctionDeclStmt::cast(n.clone())
                .and_then(|d| d.name())
                .is_some_and(|name| intersects(name.syntax().text_range(), start, end))
        })
        .min_by_key(|n| u32::from(n.text_range().len()))
    else {
        return;
    };
    let Some(decl) = ast::FunctionDeclStmt::cast(node) else {
        return;
    };
    let Some(name) = decl.name() else {
        return;
    };
    let Some(params) = decl.param_list() else {
        return;
    };

    if name.is_method() {
        colon_to_dot(sema, uri, &name, &params, out);
    } else {
        dot_to_colon(sema, uri, &name, &params, out);
    }
}

/// `function T:m(...)` → `function T.m(self, ...)`: colon to dot, prepend an
/// explicit `self` parameter.
fn colon_to_dot(
    sema: &FileSema,
    uri: &Uri,
    name: &ast::FunctionName,
    params: &ast::ParamList,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(colon) = node_token(name.syntax(), SyntaxKind::COLON) else {
        return;
    };
    let Some(lparen) = node_token(params.syntax(), SyntaxKind::L_PAREN) else {
        return;
    };
    let has_params = params.params().next().is_some();
    let self_text = if has_params { "self, " } else { "self" };
    let insert_at = usize::from(lparen.text_range().end());
    out.push(edit_action(
        uri,
        "Convert `:` method to `.` function with explicit self".to_string(),
        CodeActionKind::REFACTOR_REWRITE,
        None,
        vec![
            TextEdit {
                range: sema.index.range(token_range(&colon)),
                new_text: ".".to_string(),
            },
            TextEdit {
                range: sema.index.range(insert_at..insert_at),
                new_text: self_text.to_string(),
            },
        ],
    ));
}

/// `function T.m(self, ...)` → `function T:m(...)`: dot to colon, drop the
/// leading `self` parameter. Offered only when the name is dotted and the
/// first parameter is literally `self`.
fn dot_to_colon(
    sema: &FileSema,
    uri: &Uri,
    name: &ast::FunctionName,
    params: &ast::ParamList,
    out: &mut Vec<CodeActionOrCommand>,
) {
    // Need a `.` separator to flip (`function f` has none).
    let Some(dot) = last_node_token(name.syntax(), SyntaxKind::DOT) else {
        return;
    };
    let param_nodes: Vec<ast::Param> = params.params().collect();
    let Some(first) = param_nodes.first() else {
        return;
    };
    if first.name().map(|t| t.text().to_string()).as_deref() != Some("self") {
        return;
    }
    // Remove `self` and, when other params follow, the `, ` up to the next one.
    let self_start = usize::from(first.syntax().text_range().start());
    let removal_end = param_nodes.get(1).map_or_else(
        || usize::from(first.syntax().text_range().end()),
        |second| usize::from(second.syntax().text_range().start()),
    );
    out.push(edit_action(
        uri,
        "Convert `.` function to `:` method (drop explicit self)".to_string(),
        CodeActionKind::REFACTOR_REWRITE,
        None,
        vec![
            TextEdit {
                range: sema.index.range(token_range(&dot)),
                new_text: ":".to_string(),
            },
            TextEdit {
                range: sema.index.range(self_start..removal_end),
                new_text: String::new(),
            },
        ],
    ));
}

// === shared helpers ======================================================

/// Build a single-file code action from `edits`, sorted ascending by start
/// (LSP requires the edits of a change to be non-overlapping and applied in
/// order; ours never overlap).
#[allow(
    clippy::mutable_key_type,
    reason = "WorkspaceEdit keys its edits by Uri; Uri's hash is not affected by interior mutability"
)]
fn edit_action(
    uri: &Uri,
    title: String,
    kind: CodeActionKind,
    diagnostics: Option<Vec<Diagnostic>>,
    mut edits: Vec<TextEdit>,
) -> CodeActionOrCommand {
    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(kind),
        diagnostics,
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        }),
        ..CodeAction::default()
    })
}

/// The innermost single-name `local <x> = <expr>` whose range overlaps the
/// request, with the name token — the shape both annotation actions require.
fn simple_local(
    sema: &FileSema,
    start: usize,
    end: usize,
) -> Option<(ast::LocalStmt, SyntaxToken)> {
    let node = sema
        .root
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::LOCAL_STMT)
        .filter(|n| intersects(n.text_range(), start, end))
        .min_by_key(|n| u32::from(n.text_range().len()))?;
    let local = ast::LocalStmt::cast(node)?;
    let names: Vec<ast::LocalName> = local.names().collect();
    let [first] = &names[..] else {
        return None; // multi-name (or empty) local: annotation is ambiguous
    };
    let name_tok = first.name()?;
    // Must have an initializer to infer from.
    local.values().and_then(|v| v.exprs().next())?;
    Some((local, name_tok))
}

/// Whether an annotation block targeting the local carrying `name_tok`
/// already declares its type (`---@type` or `---@class`), in which case the
/// annotation actions stand down.
fn has_type_annotation(sema: &FileSema, name_tok: &SyntaxToken) -> bool {
    use luabox_syntax::luacats::Tag;
    sema.item_covering(name_tok.text_range())
        .is_some_and(|item| {
            item.block
                .tags
                .iter()
                .any(|t| matches!(t, Tag::Type(_) | Tag::Class(_)))
        })
}

/// The display-inference type of the binding declared at `name_tok`, widened
/// (literals to primitives), or `None` when inference has no entry for it.
fn inferred_binding_ty(inferred: &BindingTypes, name_tok: &SyntaxToken) -> Option<Ty> {
    let ns = usize::from(name_tok.text_range().start());
    let ne = usize::from(name_tok.text_range().end());
    inferred
        .bindings()
        .iter()
        .find(|b| b.range.start == ns && b.range.end == ne)
        .map(|b| b.ty.widened())
}

/// A LuaCATS type name for a table field's value expression, used only when
/// the display inference has no reified field type (a fallback).
fn guess_field_type(value: Option<&ast::Expr>) -> String {
    let ty = match value {
        Some(ast::Expr::Literal(lit)) => match lit.token().map(|t| t.kind()) {
            Some(SyntaxKind::NUMBER) => "number",
            Some(SyntaxKind::STRING) => "string",
            Some(SyntaxKind::TRUE_KW | SyntaxKind::FALSE_KW) => "boolean",
            Some(SyntaxKind::NIL_KW) => "nil",
            _ => "any",
        },
        Some(ast::Expr::Table(_)) => "table",
        _ => "any",
    };
    ty.to_string()
}

/// Uppercase the first character of `name` (`config` → `Config`).
fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The field name inside the first pair of backticks on the first line of a
/// diagnostic message (`missing required field `y` ...` → `y`).
#[expect(
    clippy::string_slice,
    reason = "open and close index an ASCII backtick, so `open + 1` and `..close` are char boundaries"
)]
fn first_backtick(message: &str) -> Option<String> {
    let line = message.lines().next()?;
    let open = line.find('`')?;
    let rest = &line[open + 1..];
    let close = rest.find('`')?;
    Some(rest[..close].to_string())
}

/// The string code of a diagnostic, when it carries one.
fn code_str(diag: &Diagnostic) -> Option<&str> {
    match &diag.code {
        Some(NumberOrString::String(code)) => Some(code),
        _ => None,
    }
}

/// Whether `range` overlaps `[start, end]` inclusively.
fn intersects(range: TextRange, start: usize, end: usize) -> bool {
    usize::from(range.start()) <= end && start <= usize::from(range.end())
}

/// The first direct token child of `node` with kind `kind`.
fn node_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .find(|t| t.kind() == kind)
}

/// The last direct token child of `node` with kind `kind`.
fn last_node_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| t.kind() == kind)
        .last()
}

/// A token's byte range.
fn token_range(token: &SyntaxToken) -> std::ops::Range<usize> {
    let r = token.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// The byte offset of the start of the line containing `offset`.
#[expect(
    clippy::string_slice,
    reason = "callers pass rowan-derived byte offsets, which are char boundaries"
)]
fn line_start(src: &str, offset: usize) -> usize {
    src[..offset].rfind('\n').map_or(0, |i| i + 1)
}

/// The leading whitespace (indentation) of the line containing `offset`.
#[expect(
    clippy::string_slice,
    reason = "ls is a line start (follows a `\\n` byte or 0), so it is a char boundary"
)]
fn line_indent(src: &str, offset: usize) -> &str {
    let ls = line_start(src, offset);
    let rest = &src[ls..];
    let end = rest
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(rest.len());
    &rest[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use lsp_types::{DiagnosticSeverity, Position, Range};
    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    use crate::LineIndex;
    use crate::uri::path_to_uri;

    /// Analyse one `main.lua` source, returning the snapshot and its path.
    fn analyze(src: &str) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let root = Path::new(if cfg!(windows) { r"C:\ws" } else { "/ws" });
        let path = root.join("main.lua");
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: src.to_string(),
        });
        (host.snapshot(), path)
    }

    /// The [`CodeAction`]s among `actions` (all four helpers only emit these).
    fn only_actions(actions: Vec<CodeActionOrCommand>) -> Vec<CodeAction> {
        actions
            .into_iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(a) => Some(a),
                CodeActionOrCommand::Command(_) => None,
            })
            .collect()
    }

    /// Apply a single-file action's edits to `src`, highest offset first so
    /// earlier ranges stay valid.
    #[allow(
        clippy::mutable_key_type,
        reason = "WorkspaceEdit keys its edits by Uri; only the values are read here"
    )]
    fn apply(src: &str, action: &CodeAction) -> String {
        let index = LineIndex::new(src);
        let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
        let mut edits: Vec<&TextEdit> = changes.values().next().unwrap().iter().collect();
        edits.sort_by_key(|e| std::cmp::Reverse((e.range.start.line, e.range.start.character)));
        let mut out = src.to_string();
        for edit in edits {
            let start = index.offset(edit.range.start);
            let end = index.offset(edit.range.end);
            out.replace_range(start..end, &edit.new_text);
        }
        out
    }

    /// Assert `src` parses with no errors — the invariant every edit's output
    /// must satisfy.
    fn assert_parses(src: &str) {
        let parse = luabox_syntax::lua::parse(src, Dialect::Lua54);
        assert!(parse.errors().is_empty(), "does not parse:\n{src}");
    }

    /// A byte-range → LSP range over `src`.
    fn range_of(src: &str, start: usize, end: usize) -> Range {
        let index = LineIndex::new(src);
        Range {
            start: index.position(start),
            end: index.position(end),
        }
    }

    /// A synthetic `LB0302` over the sole `{ ... }` in `src`, naming `field`.
    fn missing_field_diag(src: &str, field: &str) -> Diagnostic {
        let open = src.find('{').expect("table open");
        let close = src.rfind('}').expect("table close") + 1;
        Diagnostic {
            range: range_of(src, open, close),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String("LB0302".to_string())),
            source: Some("luabox".to_string()),
            message: format!("missing required field `{field}` in table literal"),
            ..Diagnostic::default()
        }
    }

    // --- add-missing-field ---

    #[test]
    fn add_missing_field_into_single_line_table() {
        let src = "local p = { x = 1 }\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let diags = vec![missing_field_diag(src, "y")];
        let mut out = Vec::new();
        add_missing_field(&sema, &diags, &uri, 0, src.len(), &mut out);
        let actions = only_actions(out);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0].kind, Some(CodeActionKind::QUICKFIX));
        // The quick-fix references the LB0302 it resolves.
        assert_eq!(actions[0].diagnostics.as_ref().unwrap().len(), 1);
        let after = apply(src, &actions[0]);
        assert_eq!(after, "local p = { x = 1,\n    y = nil, -- TODO\n}\n");
        assert_parses(&after);
    }

    #[test]
    fn add_missing_field_into_empty_table() {
        let src = "local p = {}\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let diags = vec![missing_field_diag(src, "y")];
        let mut out = Vec::new();
        add_missing_field(&sema, &diags, &uri, 0, src.len(), &mut out);
        let actions = only_actions(out);
        assert_eq!(actions.len(), 1, "{actions:?}");
        let after = apply(src, &actions[0]);
        assert_eq!(after, "local p = {\n    y = nil, -- TODO\n}\n");
        assert_parses(&after);
    }

    #[test]
    fn add_missing_field_absent_without_lb0302() {
        let src = "local p = { x = 1 }\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        // No diagnostics → nothing offered.
        add_missing_field(&sema, &[], &uri, 0, src.len(), &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    // --- annotate-local-from-inference ---

    #[test]
    fn annotate_local_inserts_inferred_type() {
        let src = "local n = 42\nprint(n)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let inferred = analysis.binding_types(&path);
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        annotate_local(&sema, inferred.as_ref(), &uri, 0, 5, &mut out);
        let actions = only_actions(out);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0].kind, Some(CodeActionKind::REFACTOR_REWRITE));
        let after = apply(src, &actions[0]);
        assert_eq!(after, "---@type integer\nlocal n = 42\nprint(n)\n");
        assert_parses(&after);
    }

    #[test]
    fn annotate_local_absent_when_already_annotated() {
        let src = "---@type number\nlocal n = 42\nprint(n)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let inferred = analysis.binding_types(&path);
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        // Cursor on the `local` line (line 1).
        let index = LineIndex::new(src);
        let at = index.offset(Position::new(1, 6));
        annotate_local(&sema, inferred.as_ref(), &uri, at, at, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    // --- generate-class-from-literal ---

    #[test]
    fn generate_class_from_literal_emits_class_and_fields() {
        let src = "local cfg = { count = 1, label = \"x\" }\nprint(cfg)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let inferred = analysis.binding_types(&path);
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        generate_class(&sema, inferred.as_ref(), &uri, 0, 5, &mut out);
        let actions = only_actions(out);
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(actions[0].kind, Some(CodeActionKind::REFACTOR_REWRITE));
        let after = apply(src, &actions[0]);
        assert!(after.contains("---@class Cfg"), "{after}");
        assert!(after.contains("---@field count integer"), "{after}");
        assert!(after.contains("---@field label string"), "{after}");
        assert_parses(&after);
    }

    #[test]
    fn generate_class_absent_for_non_table_local() {
        let src = "local n = 42\nprint(n)\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let inferred = analysis.binding_types(&path);
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        generate_class(&sema, inferred.as_ref(), &uri, 0, 5, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    // --- dot-colon-convert ---

    #[test]
    fn dot_to_colon_drops_self() {
        let src = "function T.m(self, x) end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        let actions = only_actions(out);
        assert_eq!(actions.len(), 1, "{actions:?}");
        let after = apply(src, &actions[0]);
        assert_eq!(after, "function T:m(x) end\n");
        assert_parses(&after);
    }

    #[test]
    fn dot_to_colon_drops_sole_self() {
        let src = "function T.m(self) end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        let after = apply(src, &only_actions(out)[0]);
        assert_eq!(after, "function T:m() end\n");
        assert_parses(&after);
    }

    #[test]
    fn colon_to_dot_adds_self() {
        let src = "function T:m(x) end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        let after = apply(src, &only_actions(out)[0]);
        assert_eq!(after, "function T.m(self, x) end\n");
        assert_parses(&after);
    }

    #[test]
    fn colon_to_dot_adds_sole_self() {
        let src = "function T:m() end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        let after = apply(src, &only_actions(out)[0]);
        assert_eq!(after, "function T.m(self) end\n");
        assert_parses(&after);
    }

    #[test]
    fn dot_convert_absent_without_self_param() {
        // A dotted function whose first param is not `self` is not a method.
        let src = "function T.m(x) end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn dot_convert_absent_for_bare_name() {
        // `function f` has no `.`/`:` to flip.
        let src = "function f(x) end\n";
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let uri = path_to_uri(&path);
        let mut out = Vec::new();
        dot_colon_convert(&sema, &uri, 0, src.len(), &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
