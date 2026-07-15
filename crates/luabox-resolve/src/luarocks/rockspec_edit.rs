//! Comment-preserving rockspec dependency editing (SPEC.md §6).
//!
//! A rockspec is a Lua file, and `luabox add`/`luabox remove` edit its
//! `dependencies` / `test_dependencies` tables the way `pnpm add` edits
//! `package.json`: they touch **only** the entry being added or removed and
//! leave every other byte of the file untouched — comments, blank lines,
//! indentation, quote style, and the `lua >= X.Y` interpreter pin all survive.
//!
//! The edits are **CST-guided**. The lossless Lua parser ([`luabox_syntax`],
//! the same one the formatter uses) locates the target table and each entry's
//! exact byte span; the surrounding punctuation and layout (commas, newlines,
//! indentation, quote character) are then read back out of the file text so a
//! spliced entry matches the conventions already in the file. Nothing is
//! reformatted, and no entry the edit did not name is ever rewritten.
//!
//! Two public operations:
//!
//! * [`add_dependency`] inserts `<name> <constraint>` into the `dependencies`
//!   (or, for `--dev`, `test_dependencies`) table, matching the file's quote
//!   style and indentation and getting the trailing-comma handling right for
//!   the new last entry. A name already present has its constraint updated in
//!   place. A missing `test_dependencies` table is created, formatted like the
//!   `dependencies` table already in the file.
//! * [`remove_dependency`] deletes exactly the named entry's line/segment from
//!   whichever table holds it, with correct comma handling for first, middle,
//!   and last positions.

// This module is byte-precise text splicing: every string index is either a
// CST `TextRange` offset (rowan guarantees char boundaries) or the result of
// scanning for an ASCII byte (`\n`, `,`, `;`, `"`, `'`, space, tab), which can
// never land inside a multi-byte UTF-8 sequence. Slicing at those offsets is
// therefore always sound.
#![allow(
    clippy::string_slice,
    reason = "indices are CST char-boundary offsets or ASCII-byte scan positions"
)]

use luabox_syntax::lua::ast::{
    AssignStmt, AstNode, Expr, SourceFile, Stmt, TableExpr, TableField,
};
use luabox_syntax::lua::{Dialect, parse};

/// Adds (or updates) registry dependency `name` with `constraint` (a LuaRocks
/// constraint such as `>= 1.14.0` or `== 1.2`) in the rockspec `text`.
///
/// `dev` targets the `test_dependencies` table instead of `dependencies`. The
/// written entry is the Lua string `"<name> <constraint>"`. If `name` is
/// already listed, its entry's constraint is rewritten in place (its quote
/// style preserved). A missing `test_dependencies` table is created next to
/// `dependencies`, formatted consistently with it.
///
/// Every byte outside the single inserted/updated entry is preserved verbatim.
///
/// # Errors
/// Returns an error only when the target table cannot be located and cannot be
/// created — i.e. a non-`--dev` add against a rockspec whose `dependencies` is
/// not a statically visible table literal.
pub fn add_dependency(
    text: &str,
    dev: bool,
    name: &str,
    constraint: &str,
) -> Result<String, String> {
    let table_name = if dev {
        "test_dependencies"
    } else {
        "dependencies"
    };
    let entry_inner = format!("{name} {constraint}");

    let parsed = parse(text, Dialect::Lua54);
    let file = parsed.tree();

    if let Some(table) = find_table(&file, table_name) {
        return Ok(edit_existing_table(text, &table, name, &entry_inner));
    }

    // The table is absent. `test_dependencies` is optional and created on
    // demand; a rockspec with no visible `dependencies` table has nothing to
    // splice into and is reported rather than guessed at.
    if dev {
        Ok(create_table(text, &file, table_name, &entry_inner))
    } else {
        Err(format!(
            "the rockspec has no `dependencies` table to add `{name}` to — add one, \
             e.g.\n\ndependencies = {{\n   \"lua >= 5.1\",\n}}"
        ))
    }
}

/// Removes registry dependency `name` from whichever of `dependencies` /
/// `test_dependencies` holds it, returning the edited text and whether the
/// entry was a dev (`test_dependencies`) dependency. Returns `None` when the
/// name is in neither table.
///
/// Exactly the named entry's line (multi-line tables) or segment (single-line
/// tables) is deleted, with the trailing comma handled correctly for first,
/// middle, and last positions. The `lua` interpreter pin is never a target.
#[must_use]
pub fn remove_dependency(text: &str, name: &str) -> Option<(String, bool)> {
    if name == "lua" {
        return None; // the interpreter pin is metadata, never a removable dep
    }
    let parsed = parse(text, Dialect::Lua54);
    let file = parsed.tree();
    for (dev, table_name) in [(false, "dependencies"), (true, "test_dependencies")] {
        let Some(table) = find_table(&file, table_name) else {
            continue;
        };
        let items = collect_items(text, &table);
        if let Some(idx) = items.iter().position(|item| item.bare == name) {
            return Some((remove_item(text, &table, &items, idx), dev));
        }
    }
    None
}

// --- table & entry model ---------------------------------------------------

/// One positional entry (`"lpeg >= 1.0"`) of a dependency table: its byte span
/// in the file, its bare rock name (empty when unparseable), and the quote
/// character it is written with.
struct Item {
    start: usize,
    end: usize,
    bare: String,
    quote: char,
}

/// The top-level `name = { … }` table assignment, when `name` is assigned a
/// table literal at file scope.
fn find_table(file: &SourceFile, name: &str) -> Option<TableExpr> {
    let block = file.block()?;
    for stmt in block.stmts() {
        let Stmt::Assign(assign) = stmt else { continue };
        if single_name_target(&assign).as_deref() != Some(name) {
            continue;
        }
        if let Some(table) = value_table(&assign) {
            return Some(table);
        }
    }
    None
}

/// The sole assignment target's name, or `None` for a multi-target assignment
/// or a non-name (dotted/indexed) target.
fn single_name_target(assign: &AssignStmt) -> Option<String> {
    let list = assign.targets()?;
    let mut exprs = list.exprs();
    let first = exprs.next()?;
    if exprs.next().is_some() {
        return None;
    }
    match first {
        Expr::Name(name) => name.name().map(|t| t.text().to_owned()),
        _ => None,
    }
}

/// The assignment's first value expression, when it is a table literal.
fn value_table(assign: &AssignStmt) -> Option<TableExpr> {
    match assign.values()?.exprs().next()? {
        Expr::Table(table) => Some(table),
        _ => None,
    }
}

/// The `(after '{', at '}')` byte offsets bounding a table's interior.
fn table_interior(table: &TableExpr) -> (usize, usize) {
    let range = table.syntax().text_range();
    // `{` and `}` are single ASCII bytes, so the interior is `start+1 ..= end-1`.
    (
        usize::from(range.start()) + 1,
        usize::from(range.end()) - 1,
    )
}

/// The positional entries of a dependency table, in source order.
fn collect_items(text: &str, table: &TableExpr) -> Vec<Item> {
    let mut items = Vec::new();
    for field in table.fields() {
        let TableField::Item(item) = field else {
            continue;
        };
        let range = item.syntax().text_range();
        let start = usize::from(range.start());
        let end = usize::from(range.end());
        let raw = &text[start..end];
        let quote = raw
            .chars()
            .next()
            .filter(|c| *c == '"' || *c == '\'')
            .unwrap_or('"');
        let bare = entry_bare_name(raw).unwrap_or_default();
        items.push(Item {
            start,
            end,
            bare,
            quote,
        });
    }
    items
}

/// The bare rock name of a dependency-entry literal (`"user/lpeg >= 1.0"` →
/// `lpeg`), or `None` when the literal is not a simple quoted string.
fn entry_bare_name(raw: &str) -> Option<String> {
    let inner = strip_quotes(raw)?.trim_start();
    let end = inner
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '=' | '~' | '!'))
        .unwrap_or(inner.len());
    let name = &inner[..end];
    let bare = name.rsplit('/').next().unwrap_or(name);
    (!bare.is_empty()).then(|| bare.to_owned())
}

/// The contents of a single- or double-quoted string literal, or `None` for a
/// long-bracket / non-string literal.
fn strip_quotes(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    let (&first, &last) = (bytes.first()?, bytes.last()?);
    if (first == b'"' || first == b'\'') && first == last && raw.len() >= 2 {
        // Both quotes are ASCII, so `1 .. len-1` lands on char boundaries.
        Some(&raw[1..raw.len() - 1])
    } else {
        None
    }
}

// --- adding ----------------------------------------------------------------

/// Insert `name`/`entry_inner` into an existing table, or update the entry in
/// place when `name` is already present.
fn edit_existing_table(text: &str, table: &TableExpr, name: &str, entry_inner: &str) -> String {
    let items = collect_items(text, table);
    if let Some(item) = items.iter().find(|item| item.bare == name) {
        let replacement = format!("{q}{entry_inner}{q}", q = item.quote);
        return splice(text, item.start, item.end, &replacement);
    }
    insert_entry(text, table, &items, entry_inner)
}

/// Append `entry_inner` as a new last entry, matching the table's quote style,
/// indentation, and trailing-comma convention.
fn insert_entry(text: &str, table: &TableExpr, items: &[Item], entry_inner: &str) -> String {
    let (lbrace, rbrace) = table_interior(table);
    let multiline = text[lbrace..rbrace].contains('\n');
    let quote = items.first().map_or('"', |item| item.quote);
    let quoted = format!("{quote}{entry_inner}{quote}");

    let Some(last) = items.last() else {
        return insert_into_empty(text, lbrace, rbrace, &quoted);
    };

    let has_comma = next_separator(text, last.end, rbrace).is_some();
    if multiline {
        let indent = line_indent(text, last.start);
        let eol = line_end(text, last.end, rbrace);
        if has_comma {
            // Trailing-comma style: add the new line after the last entry's
            // line (past any inline comment), itself comma-terminated.
            splice(text, eol, eol, &format!("\n{indent}{quoted},"))
        } else {
            // No-trailing-comma style: comma the previous last entry (before
            // its inline trivia) and add the new, comma-less, last entry.
            let between = &text[last.end..eol];
            splice(
                text,
                last.end,
                eol,
                &format!(",{between}\n{indent}{quoted}"),
            )
        }
    } else if let Some(comma_end) = next_separator(text, last.end, rbrace) {
        splice(text, comma_end, comma_end, &format!(" {quoted},"))
    } else {
        splice(text, last.end, last.end, &format!(", {quoted}"))
    }
}

/// Insert the first entry into an empty (`{}` / `{ }` / `{\n}`) table.
fn insert_into_empty(text: &str, lbrace: usize, rbrace: usize, quoted: &str) -> String {
    if text[lbrace..rbrace].contains('\n') {
        let brace_indent = line_indent(text, lbrace);
        let indent = format!("{brace_indent}   ");
        splice(
            text,
            lbrace,
            rbrace,
            &format!("\n{indent}{quoted},\n{brace_indent}"),
        )
    } else {
        splice(text, lbrace, rbrace, &format!(" {quoted} "))
    }
}

/// Create a `<table_name> = { … }` table, placed after the `dependencies`
/// table when one exists (else appended), formatted like it (quote style and
/// indentation) or with the 3-space rockspec default.
fn create_table(text: &str, file: &SourceFile, table_name: &str, entry_inner: &str) -> String {
    let (quote, indent) = existing_style(text, file);
    let block = format!("\n{table_name} = {{\n{indent}{quote}{entry_inner}{quote},\n}}\n");
    let anchor = table_insert_anchor(text, file);
    splice(text, anchor, anchor, &block)
}

/// The quote character and per-entry indentation of the `dependencies` table
/// (falling back to `test_dependencies`), or the 3-space `"` default.
fn existing_style(text: &str, file: &SourceFile) -> (char, String) {
    for name in ["dependencies", "test_dependencies"] {
        if let Some(table) = find_table(file, name) {
            let items = collect_items(text, &table);
            if let Some(first) = items.first() {
                return (first.quote, line_indent(text, first.start));
            }
        }
    }
    ('"', "   ".to_owned())
}

/// The byte offset to splice a freshly created table at: the end of the line
/// holding the `dependencies` (else `version`, else `package`) statement, so
/// the new table lands just below it with a blank separating line. Falls back
/// to end-of-file.
fn table_insert_anchor(text: &str, file: &SourceFile) -> usize {
    let Some(block) = file.block() else {
        return text.len();
    };
    let mut anchor: Option<usize> = None;
    for stmt in block.stmts() {
        let Stmt::Assign(assign) = &stmt else { continue };
        let Some(name) = single_name_target(assign) else {
            continue;
        };
        if matches!(name.as_str(), "dependencies" | "version" | "package") {
            let end = usize::from(assign.syntax().text_range().end());
            // Prefer `dependencies`; otherwise keep the latest of the fallbacks.
            if name == "dependencies" {
                anchor = Some(end);
                break;
            }
            anchor = Some(anchor.map_or(end, |current| current.max(end)));
        }
    }
    match anchor {
        Some(end) => text[end..]
            .find('\n')
            .map_or(text.len(), |offset| end + offset + 1),
        None => text.len(),
    }
}

// --- removing --------------------------------------------------------------

/// Delete entry `idx` from the table, handling comma/newline layout for its
/// position (first, middle, last) and for single- vs multi-line tables.
fn remove_item(text: &str, table: &TableExpr, items: &[Item], idx: usize) -> String {
    let (lbrace, rbrace) = table_interior(table);
    let item = &items[idx];
    let multiline = text[lbrace..rbrace].contains('\n');
    let trailing = next_separator(text, item.end, rbrace);

    if multiline {
        if trailing.is_some() {
            // First/middle (or last-with-trailing-comma): drop the whole
            // physical line, its terminating newline included.
            let start = line_start(text, item.start);
            let eol = line_end(text, item.end, rbrace);
            let del_end = if eol < rbrace { eol + 1 } else { eol };
            splice(text, start, del_end, "")
        } else {
            // Genuine last entry, no trailing comma: also drop the newline
            // that ended the previous entry's line, so no blank line remains.
            let prev_nl = text[..item.start].rfind('\n').unwrap_or(lbrace);
            splice(text, prev_nl, item.end, "")
        }
    } else if let Some(comma_end) = trailing {
        // Single-line, not last: drop the entry, its comma, and the space
        // separating it from the following entry.
        let mut end = comma_end;
        let bytes = text.as_bytes();
        while end < rbrace && matches!(bytes[end], b' ' | b'\t') {
            end += 1;
        }
        splice(text, item.start, end, "")
    } else {
        // Single-line last entry: drop the preceding comma and spaces too.
        let start = prev_separator_start(text, lbrace, item.start);
        splice(text, start, item.end, "")
    }
}

// --- byte-level helpers ----------------------------------------------------

/// Replace `text[start..end]` with `replacement`.
fn splice(text: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len() + replacement.len());
    out.push_str(&text[..start]);
    out.push_str(replacement);
    out.push_str(&text[end..]);
    out
}

/// The end offset just past a `,`/`;` separator immediately following `from`
/// (skipping spaces/tabs), or `None` when the next non-blank byte before
/// `limit` is not a separator.
fn next_separator(text: &str, from: usize, limit: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < limit && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    (i < limit && matches!(bytes[i], b',' | b';')).then_some(i + 1)
}

/// The start offset of a `,`/`;` separator immediately preceding `from`
/// (skipping spaces/tabs back to `floor`), or `from` when there is none.
fn prev_separator_start(text: &str, floor: usize, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = from;
    while i > floor && matches!(bytes[i - 1], b' ' | b'\t') {
        i -= 1;
    }
    if i > floor && matches!(bytes[i - 1], b',' | b';') {
        i - 1
    } else {
        from
    }
}

/// The leading whitespace of the line containing `pos`.
fn line_indent(text: &str, pos: usize) -> String {
    let start = line_start(text, pos);
    let bytes = text.as_bytes();
    let mut i = start;
    while i < pos && matches!(bytes[i], b' ' | b'\t') {
        i += 1;
    }
    text[start..i].to_owned()
}

/// The offset just after the newline preceding `pos` (start of `pos`'s line).
fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// The offset of the newline ending the line containing `from`, capped at
/// `limit` (used so a single-line table stops at its `}`).
fn line_end(text: &str, from: usize, limit: usize) -> usize {
    text[from..limit].find('\n').map_or(limit, |i| from + i)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, reason = "tests document assumptions")]
mod tests {
    use super::*;

    fn add(text: &str, name: &str, constraint: &str) -> String {
        add_dependency(text, false, name, constraint).unwrap()
    }

    fn add_dev(text: &str, name: &str, constraint: &str) -> String {
        add_dependency(text, true, name, constraint).unwrap()
    }

    #[test]
    fn add_inserts_into_a_multiline_trailing_comma_table() {
        let src = "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = {\n   \"lua >= 5.1\",\n}\n";
        let out = add(src, "penlight", ">= 1.14.0");
        assert_eq!(
            out,
            "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = {\n   \"lua >= 5.1\",\n   \"penlight >= 1.14.0\",\n}\n"
        );
    }

    #[test]
    fn add_matches_indentation_and_quote_style() {
        let src = "dependencies = {\n\t'lua >= 5.1',\n}\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(out, "dependencies = {\n\t'lua >= 5.1',\n\t'lpeg >= 1.0',\n}\n");
    }

    #[test]
    fn add_no_trailing_comma_table_commas_previous_and_omits_own() {
        let src = "dependencies = {\n   \"lua >= 5.1\"\n}\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(out, "dependencies = {\n   \"lua >= 5.1\",\n   \"lpeg >= 1.0\"\n}\n");
    }

    #[test]
    fn add_single_line_table() {
        let src = "dependencies = { \"lua >= 5.1\" }\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(out, "dependencies = { \"lua >= 5.1\", \"lpeg >= 1.0\" }\n");
    }

    #[test]
    fn add_single_line_trailing_comma_table() {
        let src = "dependencies = { \"lua >= 5.1\", }\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(out, "dependencies = { \"lua >= 5.1\", \"lpeg >= 1.0\", }\n");
    }

    #[test]
    fn add_preserves_comments_above_and_inline() {
        let src = "dependencies = {\n   -- the interpreter\n   \"lua >= 5.1\",  -- pinned\n}\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(
            out,
            "dependencies = {\n   -- the interpreter\n   \"lua >= 5.1\",  -- pinned\n   \"lpeg >= 1.0\",\n}\n"
        );
    }

    #[test]
    fn add_no_comma_with_inline_comment_keeps_comment_after_new_comma() {
        let src = "dependencies = {\n   \"lua >= 5.1\"  -- pinned\n}\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",  -- pinned\n   \"lpeg >= 1.0\"\n}\n"
        );
    }

    #[test]
    fn duplicate_add_updates_constraint_in_place() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   \"penlight >= 1.0\",\n}\n";
        let out = add(src, "penlight", ">= 1.14.0");
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",\n   \"penlight >= 1.14.0\",\n}\n"
        );
    }

    #[test]
    fn duplicate_add_preserves_quote_style_on_update() {
        let src = "dependencies = {\n   'penlight >= 1.0',\n}\n";
        let out = add(src, "penlight", "== 1.5");
        assert_eq!(out, "dependencies = {\n   'penlight == 1.5',\n}\n");
    }

    #[test]
    fn lua_entry_is_never_touched_by_add() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n}\n";
        let out = add(src, "lpeg", ">= 1.0");
        assert!(out.contains("\"lua >= 5.1\","), "lua pin survives: {out}");
    }

    #[test]
    fn dev_add_creates_test_dependencies_when_absent() {
        let src = "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = {\n   \"lua >= 5.1\",\n}\n";
        let out = add_dev(src, "busted", ">= 2.0");
        assert_eq!(
            out,
            "package = \"app\"\nversion = \"0.1.0-1\"\ndependencies = {\n   \"lua >= 5.1\",\n}\n\ntest_dependencies = {\n   \"busted >= 2.0\",\n}\n"
        );
    }

    #[test]
    fn dev_add_appends_to_existing_test_dependencies() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n}\ntest_dependencies = {\n   \"busted >= 2.0\",\n}\n";
        let out = add_dev(src, "luassert", ">= 1.9");
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",\n}\ntest_dependencies = {\n   \"busted >= 2.0\",\n   \"luassert >= 1.9\",\n}\n"
        );
    }

    #[test]
    fn remove_middle_entry_multiline() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   \"lpeg >= 1.0\",\n   \"penlight >= 1.14\",\n}\n";
        let (out, dev) = remove_dependency(src, "lpeg").unwrap();
        assert!(!dev);
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",\n   \"penlight >= 1.14\",\n}\n"
        );
    }

    #[test]
    fn remove_last_entry_with_trailing_comma() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   \"lpeg >= 1.0\",\n}\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(out, "dependencies = {\n   \"lua >= 5.1\",\n}\n");
    }

    #[test]
    fn remove_last_entry_without_trailing_comma() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   \"lpeg >= 1.0\"\n}\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(out, "dependencies = {\n   \"lua >= 5.1\",\n}\n");
    }

    #[test]
    fn remove_first_entry_multiline() {
        let src = "dependencies = {\n   \"lpeg >= 1.0\",\n   \"lua >= 5.1\",\n}\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(out, "dependencies = {\n   \"lua >= 5.1\",\n}\n");
    }

    #[test]
    fn remove_preserves_surrounding_comments() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   -- json\n   \"lpeg >= 1.0\",  -- parser\n   \"penlight >= 1.14\",\n}\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",\n   -- json\n   \"penlight >= 1.14\",\n}\n"
        );
    }

    #[test]
    fn remove_middle_entry_single_line() {
        let src = "dependencies = { \"lua >= 5.1\", \"lpeg >= 1.0\", \"penlight\" }\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(out, "dependencies = { \"lua >= 5.1\", \"penlight\" }\n");
    }

    #[test]
    fn remove_last_entry_single_line() {
        let src = "dependencies = { \"lua >= 5.1\", \"lpeg >= 1.0\" }\n";
        let (out, _) = remove_dependency(src, "lpeg").unwrap();
        assert_eq!(out, "dependencies = { \"lua >= 5.1\" }\n");
    }

    #[test]
    fn remove_from_test_dependencies_reports_dev() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n}\ntest_dependencies = {\n   \"busted >= 2.0\",\n}\n";
        let (out, dev) = remove_dependency(src, "busted").unwrap();
        assert!(dev);
        assert_eq!(
            out,
            "dependencies = {\n   \"lua >= 5.1\",\n}\ntest_dependencies = {\n}\n"
        );
    }

    #[test]
    fn remove_absent_name_is_none() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n}\n";
        assert!(remove_dependency(src, "nope").is_none());
    }

    #[test]
    fn lua_is_never_removed() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n}\n";
        assert!(remove_dependency(src, "lua").is_none());
    }

    #[test]
    fn namespaced_entry_matches_bare_name() {
        let src = "dependencies = {\n   \"lua >= 5.1\",\n   \"hisham/luaposix >= 35\",\n}\n";
        let (out, _) = remove_dependency(src, "luaposix").unwrap();
        assert_eq!(out, "dependencies = {\n   \"lua >= 5.1\",\n}\n");
    }

    #[test]
    fn round_trip_untouched_bytes_are_preserved() {
        // A rockspec with computed fields, comments, and odd spacing: every
        // byte outside the single edited entry must survive an add + remove.
        let src = "-- my rock\nrockspec_format = \"3.0\"\npackage = \"app\"\nlocal v = \"1.2.3\"\nversion = v .. \"-1\"\n\nsource = {\n   url = \"git+https://example/app.git\",  -- upstream\n}\n\ndependencies = {\n   \"lua >= 5.1\",     -- interpreter\n   \"lpeg ~> 1.0\",\n}\n\nbuild = { type = \"builtin\" }\n";
        let added = add(src, "penlight", ">= 1.14.0");
        let (removed, _) = remove_dependency(&added, "penlight").unwrap();
        assert_eq!(removed, src, "add then remove is a no-op on the file bytes");
    }
}
