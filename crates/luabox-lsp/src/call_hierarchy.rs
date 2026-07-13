//! Call hierarchy: `prepareCallHierarchy` identifies the function the cursor
//! names (a declaration or a call site) and returns it as a
//! [`CallHierarchyItem`]; `outgoingCalls` lists the functions called *within*
//! that function's body; `incomingCalls` lists the call sites across the
//! workspace that call it, grouped by their enclosing function.
//!
//! The call graph is built here from the syntax tree plus the declaration
//! walk shared with the rest of the LSP (`sema::functions()` for name
//! rendering, mirrored by [`fn_decls`] which also keeps the body node the
//! `outgoing_calls`/`incoming_calls` scans need — the `luabox_db`
//! `outgoing_calls` salsa query is a parameter-seeding name→types map, not a
//! call graph with call-site ranges, so it is not used here).
//!
//! # Precision (by-name, like find-references)
//!
//! Callees are matched by name, never by the receiver's inferred type — the
//! same trade-off [`crate::references`] documents. A bare/dotted call
//! (`f()`, `M.helper()`) matches a declaration by its exact spelled name
//! (`f`, `M.helper`); a method call (`recv:m()`) matches any `X:m`
//! declaration by the bare method name `m`, so `a:m()` and `b:m()` both map
//! to every `:m` declaration. Calls with no resolvable declaration in the
//! workspace (stdlib globals, unknown names) are omitted from `outgoing`.

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind,
};
use luabox_db::Analysis;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::lua::{SyntaxKind, SyntaxNode};
use rowan::TextRange;

use crate::sema::FileSema;
use crate::uri::path_to_uri;

/// How a callee identifies its target, matched by name (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
enum TargetKey {
    /// A bare or dotted name, matched by its exact spelling: `f`, `M.helper`.
    Named(String),
    /// A `recv:m()` method call, matched against any `X:m` declaration by the
    /// bare method name `m`.
    Method(String),
}

/// One function declaration with the geometry call hierarchy needs: the
/// display name (`f` / `M.helper` / `C:m`), how it is matched as a callee,
/// the name token (selection range), the whole statement (full range), and
/// the function node whose body is scanned for outgoing calls / used to
/// attribute a call to its enclosing function.
struct FnDecl {
    display: String,
    key: TargetKey,
    /// The name region for cursor hit-testing (the whole `FunctionName` node,
    /// or the bare name token for `local function`/`local f = function`).
    name_node: TextRange,
    /// The last name segment — the item's `selection_range`.
    selection: TextRange,
    /// The declaring statement — the item's `range`.
    full_range: TextRange,
    /// The function-defining node (`FUNCTION_DECL_STMT`,
    /// `LOCAL_FUNCTION_STMT`, or the `FUNCTION_EXPR` of `local f = function`).
    fn_node: SyntaxNode,
}

/// Every named function declaration in the file. Mirrors the declaration
/// forms [`crate::sema::FileSema::functions`] walks, keeping the AST nodes
/// call hierarchy needs (body to scan, name node to hit-test) which the
/// rendered `FnInfo` drops.
fn fn_decls(sema: &FileSema) -> Vec<FnDecl> {
    let mut out = Vec::new();
    for node in sema.root.descendants() {
        match node.kind() {
            SyntaxKind::FUNCTION_DECL_STMT => {
                let Some(decl) = ast::FunctionDeclStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(name) = decl.name() else { continue };
                let segments: Vec<_> = name.segments().collect();
                let Some(last) = segments.last() else {
                    continue;
                };
                let (display, key) = if name.is_method() && segments.len() >= 2 {
                    let base: Vec<&str> = segments[..segments.len() - 1]
                        .iter()
                        .map(luabox_syntax::lua::SyntaxToken::text)
                        .collect();
                    (
                        format!("{}:{}", base.join("."), last.text()),
                        TargetKey::Method(last.text().to_string()),
                    )
                } else {
                    let dotted = segments
                        .iter()
                        .map(luabox_syntax::lua::SyntaxToken::text)
                        .collect::<Vec<_>>()
                        .join(".");
                    (dotted.clone(), TargetKey::Named(dotted))
                };
                out.push(FnDecl {
                    display,
                    key,
                    name_node: name.syntax().text_range(),
                    selection: last.text_range(),
                    full_range: node.text_range(),
                    fn_node: node,
                });
            }
            SyntaxKind::LOCAL_FUNCTION_STMT => {
                let Some(decl) = ast::LocalFunctionStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(token) = decl.name() else { continue };
                out.push(FnDecl {
                    display: token.text().to_string(),
                    key: TargetKey::Named(token.text().to_string()),
                    name_node: token.text_range(),
                    selection: token.text_range(),
                    full_range: node.text_range(),
                    fn_node: node,
                });
            }
            SyntaxKind::LOCAL_STMT => {
                let Some(local) = ast::LocalStmt::cast(node.clone()) else {
                    continue;
                };
                let Some(ast::Expr::Function(func)) = local.values().and_then(|v| v.exprs().next())
                else {
                    continue;
                };
                let Some(token) = local.names().next().and_then(|n| n.name()) else {
                    continue;
                };
                out.push(FnDecl {
                    display: token.text().to_string(),
                    key: TargetKey::Named(token.text().to_string()),
                    name_node: token.text_range(),
                    selection: token.text_range(),
                    full_range: node.text_range(),
                    fn_node: func.syntax().clone(),
                });
            }
            _ => {}
        }
    }
    out
}

/// The [`TargetKey`] and call-site name-token range of a call node, or `None`
/// for a call whose callee is not a plain name/field/method chain.
fn call_target(node: &SyntaxNode) -> Option<(TargetKey, TextRange)> {
    match node.kind() {
        SyntaxKind::CALL_EXPR => {
            let call = ast::CallExpr::cast(node.clone())?;
            match call.callee()? {
                ast::Expr::Name(name) => {
                    let token = name.name()?;
                    Some((
                        TargetKey::Named(token.text().to_string()),
                        token.text_range(),
                    ))
                }
                callee @ ast::Expr::Field(_) => {
                    let ast::Expr::Field(field) = &callee else {
                        return None;
                    };
                    let name = field.field_name()?;
                    let dotted = dotted_path(&callee)?;
                    Some((TargetKey::Named(dotted), name.text_range()))
                }
                _ => None,
            }
        }
        SyntaxKind::METHOD_CALL_EXPR => {
            let call = ast::MethodCallExpr::cast(node.clone())?;
            let name = call.method_name()?;
            Some((
                TargetKey::Method(name.text().to_string()),
                name.text_range(),
            ))
        }
        _ => None,
    }
}

/// The dotted spelling of a name/field chain (`a.b.c`), or `None` if any
/// segment is not a plain name or field access (e.g. an index or a call).
fn dotted_path(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(name) => Some(name.name()?.text().to_string()),
        ast::Expr::Field(field) => {
            let base = dotted_path(&field.base()?)?;
            Some(format!("{base}.{}", field.field_name()?.text()))
        }
        _ => None,
    }
}

/// The nearest enclosing declared function of `node` (the caller), or `None`
/// for a call at file top level (or inside only anonymous closures).
fn enclosing_decl<'a>(decls: &'a [FnDecl], node: &SyntaxNode) -> Option<&'a FnDecl> {
    node.ancestors()
        .find_map(|anc| decls.iter().find(|d| d.fn_node == anc))
}

/// An LSP range for a byte range in `sema`'s file.
fn lsp_range(sema: &FileSema, range: TextRange) -> Range {
    sema.index
        .range(usize::from(range.start())..usize::from(range.end()))
}

/// Whether `range` contains `offset` (inclusive of the end, so a cursor just
/// past the last character of a name still hits it).
fn covers(range: TextRange, offset: usize) -> bool {
    usize::from(range.start()) <= offset && offset <= usize::from(range.end())
}

/// Build a [`CallHierarchyItem`] for a declaration in `sema`'s file. The
/// `data` field carries the resolved display name so incoming/outgoing can
/// reconstruct the target without re-resolving from a position.
fn item_for_decl(sema: &FileSema, decl: &FnDecl) -> CallHierarchyItem {
    CallHierarchyItem {
        name: decl.display.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail: None,
        uri: path_to_uri(&sema.path),
        range: lsp_range(sema, decl.full_range),
        selection_range: lsp_range(sema, decl.selection),
        data: Some(serde_json::json!({ "name": decl.display })),
    }
}

/// A synthetic module-level item for a file, used to group `incomingCalls`
/// that occur at file top level (outside any declared function).
fn module_item(sema: &FileSema) -> CallHierarchyItem {
    let name = sema
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    CallHierarchyItem {
        name,
        kind: SymbolKind::MODULE,
        tags: None,
        detail: None,
        uri: path_to_uri(&sema.path),
        range: lsp_range(sema, sema.root.text_range()),
        selection_range: Range::new(Position::new(0, 0), Position::new(0, 0)),
        data: None,
    }
}

/// The target key an item carries (from its `data`, else parsed from its
/// display name). A `:` separates a method's receiver path from its name, so
/// its presence unambiguously distinguishes a method from a bare/dotted name.
fn key_of_item(item: &CallHierarchyItem) -> TargetKey {
    let name = item
        .data
        .as_ref()
        .and_then(|d| d.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(item.name.as_str());
    key_of_name(name)
}

fn key_of_name(name: &str) -> TargetKey {
    match name.rsplit_once(':') {
        Some((_, method)) => TargetKey::Method(method.to_string()),
        None => TargetKey::Named(name.to_string()),
    }
}

/// The first declaration matching `key` across the workspace (the target file
/// first), rendered as an item — the resolution used both to answer `prepare`
/// on a call site and to build each `outgoing` callee's `to` item.
fn find_decl_item(
    analysis: &Analysis,
    target: &FileSema,
    key: &TargetKey,
) -> Option<CallHierarchyItem> {
    if let Some(decl) = fn_decls(target).iter().find(|d| d.key == *key) {
        return Some(item_for_decl(target, decl));
    }
    for path in analysis.files() {
        if path == target.path {
            continue;
        }
        let Some(sema) = FileSema::new(analysis, path) else {
            continue;
        };
        if let Some(decl) = fn_decls(&sema).iter().find(|d| d.key == *key) {
            return Some(item_for_decl(&sema, decl));
        }
    }
    None
}

/// `prepareCallHierarchy`: the function the cursor names — a declaration name
/// (`function f`, `local function f`, `M.g`, `C:m`) or a call/name use that
/// resolves to a declaration. Returns a single-item vector (or `None`).
#[must_use]
pub fn prepare(
    analysis: &Analysis,
    sema: &FileSema,
    offset: usize,
) -> Option<Vec<CallHierarchyItem>> {
    // On a declaration name: answer with that exact declaration.
    if let Some(decl) = fn_decls(sema).iter().find(|d| covers(d.name_node, offset)) {
        return Some(vec![item_for_decl(sema, decl)]);
    }
    // On a call site / name use: resolve the callee to its declaration.
    let key = key_at(sema, offset)?;
    Some(vec![find_decl_item(analysis, sema, &key)?])
}

/// The [`TargetKey`] the cursor names at a use site: a method name, a dotted
/// field access, or a bare name.
fn key_at(sema: &FileSema, offset: usize) -> Option<TargetKey> {
    let token = sema.ident_at(offset)?;
    let parent = token.parent()?;
    match parent.kind() {
        SyntaxKind::METHOD_CALL_EXPR => {
            let name = ast::MethodCallExpr::cast(parent)?.method_name()?;
            (name.text_range() == token.text_range())
                .then(|| TargetKey::Method(name.text().to_string()))
        }
        SyntaxKind::FIELD_EXPR => {
            let field = ast::FieldExpr::cast(parent.clone())?;
            let name = field.field_name()?;
            if name.text_range() != token.text_range() {
                return None;
            }
            dotted_path(&ast::Expr::Field(field)).map(TargetKey::Named)
        }
        SyntaxKind::NAME_EXPR => Some(TargetKey::Named(token.text().to_string())),
        _ => None,
    }
}

/// `callHierarchy/outgoingCalls`: the functions called within `item`'s body,
/// each grouped with the call-site ranges (name tokens) inside `item`. Calls
/// nested in a *declared* function within the body belong to that inner
/// function, not `item`, and are excluded.
#[must_use]
pub fn outgoing_calls(
    analysis: &Analysis,
    sema: &FileSema,
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyOutgoingCall> {
    let decls = fn_decls(sema);
    let Some(target) = decls
        .iter()
        .find(|d| lsp_range(sema, d.selection) == item.selection_range)
    else {
        return Vec::new();
    };

    // Group call sites by callee key, preserving first-seen order.
    let mut groups: Vec<(TargetKey, Vec<TextRange>)> = Vec::new();
    for node in target.fn_node.descendants() {
        let Some((key, name_range)) = call_target(&node) else {
            continue;
        };
        // Skip calls that belong to a nested declared function.
        if !enclosing_decl(&decls, &node).is_some_and(|d| d.fn_node == target.fn_node) {
            continue;
        }
        if let Some((_, ranges)) = groups.iter_mut().find(|(k, _)| *k == key) {
            ranges.push(name_range);
        } else {
            groups.push((key, vec![name_range]));
        }
    }

    let mut out = Vec::new();
    for (key, mut ranges) in groups {
        let Some(to) = find_decl_item(analysis, sema, &key) else {
            continue;
        };
        ranges.sort_by_key(|r| r.start());
        out.push(CallHierarchyOutgoingCall {
            to,
            from_ranges: ranges.iter().map(|r| lsp_range(sema, *r)).collect(),
        });
    }
    out.sort_by_key(|c| range_key(c.from_ranges.first()));
    out
}

/// One caller's incoming calls: the caller item and the call-site ranges
/// within it, accumulated across the scan.
struct IncomingGroup {
    from: CallHierarchyItem,
    ranges: Vec<Range>,
}

/// `callHierarchy/incomingCalls`: every call site across the workspace whose
/// callee resolves to `item`, grouped by its enclosing function (the caller);
/// top-level calls group under a synthetic module item for their file.
#[must_use]
pub fn incoming_calls(
    analysis: &Analysis,
    target: &FileSema,
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyIncomingCall> {
    let key = key_of_item(item);
    let mut groups: Vec<((String, Range), IncomingGroup)> = Vec::new();

    for path in analysis.files() {
        let sema = if path == target.path {
            // Reuse the already-built view for the target's own file.
            None
        } else {
            FileSema::new(analysis, path)
        };
        let sema_ref = sema.as_ref().unwrap_or(target);
        let decls = fn_decls(sema_ref);
        for node in sema_ref.root.descendants() {
            let Some((call_key, name_range)) = call_target(&node) else {
                continue;
            };
            if call_key != key {
                continue;
            }
            let from = match enclosing_decl(&decls, &node) {
                Some(decl) => item_for_decl(sema_ref, decl),
                None => module_item(sema_ref),
            };
            let id = (from.uri.as_str().to_string(), from.selection_range);
            let range = lsp_range(sema_ref, name_range);
            if let Some((_, group)) = groups.iter_mut().find(|(gid, _)| *gid == id) {
                group.ranges.push(range);
            } else {
                groups.push((
                    id,
                    IncomingGroup {
                        from,
                        ranges: vec![range],
                    },
                ));
            }
        }
    }

    let mut out: Vec<CallHierarchyIncomingCall> = groups
        .into_iter()
        .map(|(_, mut group)| {
            group
                .ranges
                .sort_by_key(|r| (r.start.line, r.start.character));
            CallHierarchyIncomingCall {
                from: group.from,
                from_ranges: group.ranges,
            }
        })
        .collect();
    out.sort_by_key(|c| {
        (
            c.from.uri.as_str().to_string(),
            c.from.selection_range.start.line,
            c.from.selection_range.start.character,
        )
    });
    out
}

/// A sort key over an optional range's start (for ordering call groups).
fn range_key(range: Option<&Range>) -> (u32, u32) {
    range.map_or((u32::MAX, u32::MAX), |r| (r.start.line, r.start.character))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    /// Build an analysis over `files`, returning the snapshot and the absolute
    /// path of the first file.
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

    fn sema_for(analysis: &Analysis, path: &Path) -> FileSema {
        FileSema::new(analysis, path).expect("sema")
    }

    fn offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    #[test]
    fn prepare_on_a_declaration_returns_the_function_item() {
        let src = "local function greet() end\ngreet()\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);
        // Cursor on the `greet` in the declaration.
        let offset = offset_of(src, "greet") + 1;
        let items = prepare(&analysis, &sema, offset).expect("prepare");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "greet");
        assert_eq!(items[0].kind, SymbolKind::FUNCTION);
        // The selection range is the name; the full range spans the statement.
        assert_eq!(items[0].selection_range, range((0, 15), (0, 20)));
        assert_eq!(items[0].range.start, Position::new(0, 0));
    }

    #[test]
    fn prepare_on_a_call_site_resolves_to_the_declaration() {
        let src = "local function greet() end\ngreet()\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);
        // Cursor on the `greet` call on line 1.
        let offset = src.rfind("greet").expect("call") + 1;
        let items = prepare(&analysis, &sema, offset).expect("prepare");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "greet");
        // Points back at the declaration on line 0, not the call site.
        assert_eq!(items[0].selection_range.start.line, 0);
    }

    #[test]
    fn outgoing_lists_callees_within_the_body_with_ranges() {
        let src = "\
local function a() end
local function b() end
local function caller()
  a()
  b()
  a()
end
";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);
        let offset = offset_of(src, "caller") + 1;
        let item = prepare(&analysis, &sema, offset).expect("prepare")[0].clone();
        let calls = outgoing_calls(&analysis, &sema, &item);
        let names: Vec<&str> = calls.iter().map(|c| c.to.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "{calls:?}");
        // `a` is called twice, `b` once.
        let a = calls.iter().find(|c| c.to.name == "a").unwrap();
        assert_eq!(a.from_ranges.len(), 2, "{a:?}");
        let b = calls.iter().find(|c| c.to.name == "b").unwrap();
        assert_eq!(b.from_ranges.len(), 1, "{b:?}");
    }

    #[test]
    fn outgoing_excludes_calls_in_nested_functions() {
        let src = "\
local function inner_target() end
local function outer()
  local function inner()
    inner_target()
  end
  inner()
end
";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);
        let offset = offset_of(src, "outer") + 1;
        let item = prepare(&analysis, &sema, offset).expect("prepare")[0].clone();
        let calls = outgoing_calls(&analysis, &sema, &item);
        let names: Vec<&str> = calls.iter().map(|c| c.to.name.as_str()).collect();
        // Only the direct `inner()` call; `inner_target()` belongs to `inner`.
        assert_eq!(names, vec!["inner"], "{calls:?}");
    }

    #[test]
    fn outgoing_skips_unresolvable_callees() {
        let src = "\
local function caller()
  print(1)
  undefined_global()
end
";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);
        let offset = offset_of(src, "caller") + 1;
        let item = prepare(&analysis, &sema, offset).expect("prepare")[0].clone();
        // Neither `print` nor `undefined_global` is a workspace declaration.
        assert!(outgoing_calls(&analysis, &sema, &item).is_empty());
    }

    #[test]
    fn incoming_finds_callers_across_files_grouped_by_enclosing_function() {
        let files = &[
            ("a.lua", "function greet() return 1 end\n"),
            (
                "b.lua",
                "local function useGreet()\n  greet()\n  greet()\nend\n",
            ),
        ];
        let (analysis, _) = analyze(files);
        let a_path = analysis
            .files()
            .find(|p| p.ends_with("a.lua"))
            .unwrap()
            .to_path_buf();
        let sema = sema_for(&analysis, &a_path);
        // Prepare on the `greet` declaration in a.lua.
        let offset = offset_of("function greet() return 1 end\n", "greet");
        let item = prepare(&analysis, &sema, offset).expect("prepare")[0].clone();
        let calls = incoming_calls(&analysis, &sema, &item);
        assert_eq!(calls.len(), 1, "one caller: {calls:?}");
        assert_eq!(calls[0].from.name, "useGreet");
        assert!(calls[0].from.uri.as_str().ends_with("b.lua"));
        // Both call sites are collected as from_ranges in the caller.
        assert_eq!(calls[0].from_ranges.len(), 2, "{calls:?}");
    }

    #[test]
    fn incoming_groups_top_level_calls_under_a_module_item() {
        let files = &[
            ("a.lua", "function greet() return 1 end\n"),
            ("b.lua", "greet()\n"),
        ];
        let (analysis, _) = analyze(files);
        let a_path = analysis
            .files()
            .find(|p| p.ends_with("a.lua"))
            .unwrap()
            .to_path_buf();
        let sema = sema_for(&analysis, &a_path);
        let offset = offset_of("function greet() return 1 end\n", "greet");
        let item = prepare(&analysis, &sema, offset).expect("prepare")[0].clone();
        let calls = incoming_calls(&analysis, &sema, &item);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].from.kind, SymbolKind::MODULE);
        assert!(calls[0].from.name.ends_with("b.lua"));
    }

    #[test]
    fn method_and_dotted_calls_resolve_by_name() {
        let src = "\
function M.helper() end
function C:run()
  M.helper()
end
local function driver()
  local c = nil
  c:run()
end
";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = sema_for(&analysis, &path);

        // Outgoing of `C:run` finds the dotted `M.helper` callee.
        let run_off = offset_of(src, "C:run") + 2;
        let run_item = prepare(&analysis, &sema, run_off).expect("prepare")[0].clone();
        assert_eq!(run_item.name, "C:run");
        let run_out = outgoing_calls(&analysis, &sema, &run_item);
        assert_eq!(run_out.len(), 1, "{run_out:?}");
        assert_eq!(run_out[0].to.name, "M.helper");

        // Incoming of `C:run` finds the `c:run()` method call in `driver`.
        let run_in = incoming_calls(&analysis, &sema, &run_item);
        assert_eq!(run_in.len(), 1, "{run_in:?}");
        assert_eq!(run_in[0].from.name, "driver");
    }

    fn range(start: (u32, u32), end: (u32, u32)) -> Range {
        Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
    }
}
