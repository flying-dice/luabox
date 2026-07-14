//! Find references: the inverse of goto-definition. Given a position, collect
//! every use of the symbol there. Locals/upvalues are file-scoped (resolved
//! through the HIR to a single binding, then every name use pointing back at
//! it); genuinely cross-file symbols — bare/dotted globals and workspace-global
//! `---@class` field/method names — are matched by resolved name across every
//! `.lua` file the analysis knows about.

use lsp_types::Location;
use luabox_db::Analysis;
use luabox_hir::{BindingId, Resolution};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::lua::ast::{self, AstNode};
use luabox_syntax::luacats::FieldKey;
use rowan::TextRange;

use crate::sema::FileSema;
use crate::uri::path_to_uri;

/// What symbol the cursor identifies, and how far its references reach.
enum Target {
    /// A local / upvalue binding — references live only in its own file.
    Local(BindingId),
    /// A free name (bare global or a global `function`) — workspace-wide,
    /// matched by [`Resolution::Global`] name plus global definition sites.
    Global(String),
    /// A member name (`recv.field`, `recv:method`, a dotted/method `function`
    /// segment, or a `---@field`) — matched by name across the workspace.
    Member(String),
}

/// Compute every reference to the symbol at `offset` in `target`. `analysis`
/// supplies the other workspace files for cross-file symbols; `target` is the
/// already-built view of the file under the cursor and is reused for its own
/// file rather than rebuilt. `include_declaration` toggles the definition site.
#[must_use]
pub fn references(
    analysis: &Analysis,
    target: &FileSema,
    offset: usize,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let kind = classify(target, offset)?;
    let mut uses: Vec<Location> = Vec::new();
    let mut decls: Vec<Location> = Vec::new();
    match &kind {
        Target::Local(id) => collect_local(target, *id, &mut uses, &mut decls),
        Target::Global(_) | Target::Member(_) => {
            collect_cross(target, &kind, &mut uses, &mut decls);
            for path in analysis.files() {
                if path == target.path {
                    continue;
                }
                if let Some(file) = FileSema::new(analysis, path) {
                    collect_cross(&file, &kind, &mut uses, &mut decls);
                }
            }
        }
    }
    Some(finish(uses, decls, include_declaration))
}

/// Classify the cursor into the symbol it names. Member accesses win over name
/// resolution (the cursor is on the member, not the receiver); a bare use
/// resolves through the HIR; on a declaration site we fall back to the binding
/// or the `function` name being declared.
fn classify(sema: &FileSema, offset: usize) -> Option<Target> {
    let token = sema.ident_at(offset)?;
    if let Some(name) = member_name_at(&token) {
        return Some(Target::Member(name));
    }
    match sema.resolution_at(offset) {
        Some(Resolution::Local(id) | Resolution::Upvalue { binding: id, .. }) => {
            Some(Target::Local(id))
        }
        Some(Resolution::Global(name)) => Some(Target::Global(name)),
        None => {
            // On a declaration itself: a local binding, or a `function` name.
            if let Some(id) = sema.binding_decl_at(offset) {
                return Some(Target::Local(id));
            }
            decl_target_at(&token)
        }
    }
}

/// The member name the cursor is on: the field of a `recv.field` access or the
/// method of a `recv:method()` call (only when the cursor is on the member
/// token, not the receiver).
fn member_name_at(token: &luabox_syntax::lua::SyntaxToken) -> Option<String> {
    let parent = token.parent()?;
    match parent.kind() {
        SyntaxKind::FIELD_EXPR => {
            let name = ast::FieldExpr::cast(parent)?.field_name()?;
            (name.text_range() == token.text_range()).then(|| name.text().to_string())
        }
        SyntaxKind::METHOD_CALL_EXPR => {
            let name = ast::MethodCallExpr::cast(parent)?.method_name()?;
            (name.text_range() == token.text_range()).then(|| name.text().to_string())
        }
        _ => None,
    }
}

/// The target for a cursor sitting on a `function a.b:c` declaration name: the
/// last segment of a dotted/method name is a member, a single segment (or the
/// base table) is a global.
fn decl_target_at(token: &luabox_syntax::lua::SyntaxToken) -> Option<Target> {
    let decl = token
        .parent()?
        .ancestors()
        .find_map(ast::FunctionDeclStmt::cast)?;
    let segments: Vec<_> = decl.name()?.segments().collect();
    let idx = segments
        .iter()
        .position(|s| s.text_range() == token.text_range())?;
    if segments.len() >= 2 && idx == segments.len() - 1 {
        Some(Target::Member(token.text().to_string()))
    } else {
        Some(Target::Global(segments.first()?.text().to_string()))
    }
}

/// File-scoped references of a local/upvalue binding: every resolved name use
/// pointing at it, plus its declaration site.
fn collect_local(
    sema: &FileSema,
    id: BindingId,
    uses: &mut Vec<Location>,
    decls: &mut Vec<Location>,
) {
    for (range, res) in sema.name_resolutions() {
        let hits = match res {
            Resolution::Local(b) | Resolution::Upvalue { binding: b, .. } => b == id,
            Resolution::Global(_) => false,
        };
        if hits {
            uses.push(location(sema, range));
        }
    }
    decls.push(location(sema, sema.binding(id).range));
}

/// Cross-file references of a global or member name within one file.
fn collect_cross(
    sema: &FileSema,
    kind: &Target,
    uses: &mut Vec<Location>,
    decls: &mut Vec<Location>,
) {
    match kind {
        Target::Global(name) => {
            for (range, res) in sema.name_resolutions() {
                if matches!(&res, Resolution::Global(n) if n == name) {
                    uses.push(location(sema, range));
                }
            }
            // Global definition sites: declared `function name` and top-level
            // `name = ...` assignment targets (both are declarations, never
            // shadowing locals — those resolve to `Local`).
            for info in sema.functions() {
                if info.name == *name {
                    decls.push(location(sema, info.decl_range));
                }
            }
            for (defined, range) in sema.global_defs() {
                if defined == *name {
                    decls.push(location(sema, range));
                }
            }
        }
        Target::Member(name) => {
            for node in sema.root.descendants() {
                match node.kind() {
                    SyntaxKind::FIELD_EXPR => {
                        if let Some(field) = ast::FieldExpr::cast(node.clone())
                            && let Some(token) = field.field_name()
                            && token.text() == name
                        {
                            uses.push(location(sema, token.text_range()));
                        }
                    }
                    SyntaxKind::METHOD_CALL_EXPR => {
                        if let Some(call) = ast::MethodCallExpr::cast(node.clone())
                            && let Some(token) = call.method_name()
                            && token.text() == name
                        {
                            uses.push(location(sema, token.text_range()));
                        }
                    }
                    SyntaxKind::FUNCTION_DECL_STMT => {
                        // `function T.m` / `function T:m` declares the member.
                        if let Some(decl) = ast::FunctionDeclStmt::cast(node.clone())
                            && let Some(fname) = decl.name()
                        {
                            let segments: Vec<_> = fname.segments().collect();
                            if segments.len() >= 2
                                && let Some(last) = segments.last()
                                && last.text() == name
                            {
                                decls.push(location(sema, last.text_range()));
                            }
                        }
                    }
                    _ => {}
                }
            }
            // `---@field name` annotation sites (workspace-global classes).
            for info in sema.classes().values() {
                for field in &info.fields {
                    if let FieldKey::Name(field_name) = &field.key
                        && field_name == name
                    {
                        decls.push(location_bytes(sema, field.span.start, field.span.end));
                    }
                }
            }
        }
        Target::Local(_) => {}
    }
}

/// Combine uses and declarations per `include_declaration`, then order and
/// deduplicate (files are visited in unspecified order, and a definition can
/// coincide with a use — e.g. an assignment target that is also read).
fn finish(
    mut uses: Vec<Location>,
    decls: Vec<Location>,
    include_declaration: bool,
) -> Vec<Location> {
    if include_declaration {
        uses.extend(decls);
    } else {
        uses.retain(|loc| !decls.contains(loc));
    }
    uses.sort_by(|a, b| key(a).cmp(&key(b)));
    uses.dedup();
    uses
}

/// A total order over locations: file, then start, then end.
fn key(loc: &Location) -> (&str, u32, u32, u32, u32) {
    (
        loc.uri.as_str(),
        loc.range.start.line,
        loc.range.start.character,
        loc.range.end.line,
        loc.range.end.character,
    )
}

fn location(sema: &FileSema, range: TextRange) -> Location {
    location_bytes(sema, usize::from(range.start()), usize::from(range.end()))
}

fn location_bytes(sema: &FileSema, start: usize, end: usize) -> Location {
    Location {
        uri: path_to_uri(&sema.path),
        range: sema.index.range(start..end),
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    /// Build an analysis over `files`, returning the snapshot and the absolute
    /// path of the first file (the reference-request target).
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

    /// Byte offset just inside the `nth` (0-based) occurrence of `needle`.
    fn offset_of(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for _ in 0..nth {
            from = text[from..].find(needle).expect("occurrence") + from + 1;
        }
        text[from..].find(needle).expect("occurrence") + from
    }

    fn run(files: &[(&str, &str)], offset: usize, include: bool) -> Vec<Location> {
        let (analysis, path) = analyze(files);
        let target = FileSema::new(&analysis, &path).expect("target sema");
        references(&analysis, &target, offset, include).expect("references")
    }

    #[test]
    fn local_uses_are_file_scoped_with_declaration_toggle() {
        let src = "local value = 1\nprint(value)\nreturn value + value\n";
        // The cursor is on `value` inside `print(value)`.
        let offset = offset_of(src, "value", 1);
        let with = run(&[("main.lua", src)], offset, true);
        // The declaration plus three uses, all in the one file.
        assert_eq!(with.len(), 4, "{with:?}");
        assert!(
            with.iter()
                .any(|l| l.range.start.line == 0 && l.range.start.character == 6)
        );

        let without = run(&[("main.lua", src)], offset, false);
        assert_eq!(without.len(), 3, "{without:?}");
        assert!(
            !without
                .iter()
                .any(|l| l.range.start.line == 0 && l.range.start.character == 6)
        );
    }

    #[test]
    fn upvalue_uses_resolve_to_the_same_binding() {
        let src = "local n = 1\nlocal function f() return n end\n";
        let offset = offset_of(src, "n", 0); // the declaration
        let refs = run(&[("main.lua", src)], offset, true);
        // Declaration plus the upvalue read inside `f`.
        assert_eq!(refs.len(), 2, "{refs:?}");
    }

    #[test]
    fn global_function_references_span_files() {
        let files = &[
            ("a.lua", "function greet() return 1 end\n"),
            ("b.lua", "greet()\ngreet()\n"),
        ];
        // Cursor on the `greet` call in b.lua (the second file's first use).
        let (analysis, _) = analyze(files);
        let b = analysis
            .files()
            .find(|p| p.ends_with("b.lua"))
            .unwrap()
            .to_path_buf();
        let sema = FileSema::new(&analysis, &b).unwrap();
        let offset = offset_of("greet()\ngreet()\n", "greet", 0);

        let with = references(&analysis, &sema, offset, true).unwrap();
        // The declaration in a.lua plus two uses in b.lua.
        assert_eq!(with.len(), 3, "{with:?}");
        assert_eq!(
            with.iter()
                .filter(|l| l.uri.as_str().ends_with("a.lua"))
                .count(),
            1
        );
        assert_eq!(
            with.iter()
                .filter(|l| l.uri.as_str().ends_with("b.lua"))
                .count(),
            2
        );

        let without = references(&analysis, &sema, offset, false).unwrap();
        assert_eq!(without.len(), 2, "{without:?}");
        assert!(without.iter().all(|l| l.uri.as_str().ends_with("b.lua")));
    }

    #[test]
    fn class_field_references_match_by_name_across_files() {
        let files = &[
            ("point.lua", "---@class Point\n---@field x number\n"),
            (
                "use.lua",
                "---@type Point\nlocal p = nil\nprint(p.x)\nprint(p.x)\n",
            ),
        ];
        let (analysis, _) = analyze(files);
        let use_path = analysis
            .files()
            .find(|p| p.ends_with("use.lua"))
            .unwrap()
            .to_path_buf();
        let sema = FileSema::new(&analysis, &use_path).unwrap();
        let src = "---@type Point\nlocal p = nil\nprint(p.x)\nprint(p.x)\n";
        let offset = offset_of(src, ".x", 0) + 1; // on the `x` of the first `p.x`

        let with = references(&analysis, &sema, offset, true).unwrap();
        // Two member accesses plus the `---@field x` declaration.
        assert_eq!(with.len(), 3, "{with:?}");
        assert_eq!(
            with.iter()
                .filter(|l| l.uri.as_str().ends_with("point.lua"))
                .count(),
            1
        );

        let without = references(&analysis, &sema, offset, false).unwrap();
        assert_eq!(without.len(), 2, "{without:?}");
        assert!(without.iter().all(|l| l.uri.as_str().ends_with("use.lua")));
    }
}
