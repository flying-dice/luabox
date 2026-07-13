//! Rename: find-all-references re-expressed as a [`WorkspaceEdit`]. The
//! renameable symbol and its full reference set (declaration included) come
//! straight from [`references`], so the local / global / member classification
//! is not re-derived here — a rename is exactly "find every reference, replace
//! each with the new name".
//!
//! One correction is applied on the way out. [`references`] reports a
//! `---@field name Type desc` declaration as the *whole annotation-line* span
//! (right for a highlight, wrong for a rename — it would overwrite the type and
//! description too). Every other reference it returns is already the bare name
//! token. So each edit range is narrowed to the exact `old_name` occurrence
//! within it before it becomes a [`TextEdit`]: a rename must never replace more
//! than the name.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use lsp_types::{Location, Range, TextEdit, Uri, WorkspaceEdit};
use luabox_db::Analysis;

use crate::line_index::LineIndex;
use crate::references::references;
use crate::sema::FileSema;
use crate::uri::uri_to_path;

/// The identifier range to pre-select for a rename at `offset`, or `None` when
/// the position is not a renameable symbol. Renameability reuses
/// [`references`]' classification: a position it cannot resolve to a symbol is
/// not renameable.
#[must_use]
pub fn prepare_rename(analysis: &Analysis, target: &FileSema, offset: usize) -> Option<Range> {
    let token = target.ident_at(offset)?;
    // Only offer a rename where a symbol is actually identified.
    references(analysis, target, offset, true)?;
    let range = token.text_range();
    Some(
        target
            .index
            .range(usize::from(range.start())..usize::from(range.end())),
    )
}

/// A [`WorkspaceEdit`] renaming the symbol at `offset` to `new_name` at every
/// reference and its declaration. `None` when the position is not a renameable
/// symbol. Each edit replaces exactly the identifier token — the `---@field`
/// line span [`references`] returns for a member declaration is narrowed to its
/// name token first.
#[must_use]
#[allow(
    clippy::mutable_key_type,
    reason = "WorkspaceEdit keys its edits by Uri; the lint's interior-mutability concern does not affect Uri's hash"
)]
pub fn rename(
    analysis: &Analysis,
    target: &FileSema,
    offset: usize,
    new_name: &str,
) -> Option<WorkspaceEdit> {
    let old_name = target.ident_at(offset)?.text().to_string();
    let locations = references(analysis, target, offset, true)?;

    // One line index per referenced file, reused across that file's edits.
    let mut indices: HashMap<Uri, LineIndex> = HashMap::new();
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for loc in locations {
        let index = match indices.entry(loc.uri.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let path = uri_to_path(&loc.uri)?;
                let text = analysis.file_text(&path)?;
                entry.insert(LineIndex::new(text))
            }
        };
        let Some(range) = name_range(index, &loc, &old_name) else {
            continue;
        };
        changes.entry(loc.uri).or_default().push(TextEdit {
            range,
            new_text: new_name.to_string(),
        });
    }

    // Dedupe coincident edits per file (a definition can also be a use);
    // narrowed ranges are otherwise already distinct.
    for edits in changes.values_mut() {
        edits.sort_by_key(range_key);
        edits.dedup();
    }

    Some(WorkspaceEdit {
        changes: Some(changes),
        ..WorkspaceEdit::default()
    })
}

/// The precise name-token range for one reference: the location's range as-is
/// when it already spans exactly `old_name`, otherwise the `old_name` token
/// found within it (the wide `---@field` line span).
fn name_range(index: &LineIndex, loc: &Location, old_name: &str) -> Option<Range> {
    let start = index.offset(loc.range.start);
    let end = index.offset(loc.range.end);
    let slice = index.text().get(start..end)?;
    if slice == old_name {
        return Some(loc.range);
    }
    let (name_start, name_end) = narrow(slice, old_name)?;
    Some(index.range(start + name_start..start + name_end))
}

/// Locate `name` as a standalone identifier within `slice`, returning its byte
/// offsets relative to `slice`. A leading `@tag` word is skipped so a field
/// whose name equals its own tag word is not matched on the tag itself.
fn narrow(slice: &str, name: &str) -> Option<(usize, usize)> {
    let bytes = slice.as_bytes();
    let mut from = 0;
    if let Some(rest) = slice.strip_prefix('@') {
        from = rest
            .find(|c: char| !is_ident_char(c))
            .map_or(slice.len(), |i| i + 1);
    }
    while let Some(rel) = slice[from..].find(name) {
        let s = from + rel;
        let e = s + name.len();
        let before = s == 0 || !is_ident_byte(bytes[s - 1]);
        let after = e == slice.len() || !is_ident_byte(bytes[e]);
        if before && after {
            return Some((s, e));
        }
        from = s + 1;
    }
    None
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// A total order over an edit's range: start then end.
fn range_key(edit: &TextEdit) -> (u32, u32, u32, u32) {
    (
        edit.range.start.line,
        edit.range.start.character,
        edit.range.end.line,
        edit.range.end.character,
    )
}

#[cfg(test)]
#[allow(
    clippy::mutable_key_type,
    reason = "WorkspaceEdit keys its edits by Uri throughout these tests"
)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{AnalysisHost, Change, Dialect, Strictness};

    /// Build an analysis over `files`, returning the snapshot and the absolute
    /// path of the first file (the rename-request target).
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

    /// The full `WorkspaceEdit.changes` for a rename request on the first file.
    fn rename_changes(
        files: &[(&str, &str)],
        offset: usize,
        new_name: &str,
    ) -> HashMap<Uri, Vec<TextEdit>> {
        let (analysis, path) = analyze(files);
        let target = FileSema::new(&analysis, &path).expect("target sema");
        rename(&analysis, &target, offset, new_name)
            .expect("rename")
            .changes
            .expect("changes")
    }

    /// Assert every edit in `changes` replaces exactly `old_name` — i.e. the
    /// range's text in that file equals the old name, never a wider span. This
    /// is the correctness invariant: a rename that overwrites more than the
    /// name is a bug (notably the `---@field` line span).
    fn assert_name_precise(
        files: &[(&str, &str)],
        changes: &HashMap<Uri, Vec<TextEdit>>,
        old: &str,
    ) {
        for (uri, edits) in changes {
            // Match the file by its trailing path component(s).
            let text = files
                .iter()
                .find(|(rel, _)| uri.as_str().ends_with(&rel.replace('\\', "/")))
                .map_or_else(|| panic!("no source for {uri:?}"), |(_, text)| *text);
            let index = LineIndex::new(text);
            for edit in edits {
                let start = index.offset(edit.range.start);
                let end = index.offset(edit.range.end);
                assert_eq!(
                    &text[start..end],
                    old,
                    "edit range must be exactly `{old}`, got `{}` in {uri:?}",
                    &text[start..end]
                );
            }
        }
    }

    #[test]
    fn rename_local_updates_every_use_and_the_declaration() {
        let src = "local value = 1\nprint(value)\nreturn value + value\n";
        let files = &[("main.lua", src)];
        // Cursor on `value` inside `print(value)`.
        let offset = offset_of(src, "value", 1);
        let changes = rename_changes(files, offset, "amount");
        assert_eq!(changes.len(), 1, "single file: {changes:?}");
        let edits = changes.values().next().unwrap();
        // Declaration plus three uses.
        assert_eq!(edits.len(), 4, "{edits:?}");
        assert!(edits.iter().all(|e| e.new_text == "amount"));
        assert_name_precise(files, &changes, "value");
    }

    #[test]
    fn rename_global_function_spans_files() {
        let files = &[
            ("a.lua", "function greet() return 1 end\n"),
            ("b.lua", "greet()\ngreet()\n"),
        ];
        let (analysis, _) = analyze(files);
        let b = analysis
            .files()
            .find(|p| p.ends_with("b.lua"))
            .unwrap()
            .to_path_buf();
        let sema = FileSema::new(&analysis, &b).unwrap();
        let offset = offset_of("greet()\ngreet()\n", "greet", 0);
        let changes = rename(&analysis, &sema, offset, "hello")
            .expect("rename")
            .changes
            .expect("changes");
        assert_eq!(changes.len(), 2, "two files: {changes:?}");
        let total: usize = changes.values().map(Vec::len).sum();
        // The declaration in a.lua plus two calls in b.lua.
        assert_eq!(total, 3, "{changes:?}");
        assert_name_precise(files, &changes, "greet");
    }

    #[test]
    fn rename_class_field_is_name_precise_across_files() {
        // The `---@field x number` declaration must be narrowed to `x`, not
        // the whole annotation line.
        let files = &[
            ("point.lua", "---@class Point\n---@field x number\n"),
            (
                "use.lua",
                "---@type Point\nlocal p = nil\nprint(p.x)\nprint(p.x)\n",
            ),
        ];
        let src = "---@type Point\nlocal p = nil\nprint(p.x)\nprint(p.x)\n";
        let offset = offset_of(src, ".x", 0) + 1; // on the `x` of the first `p.x`
        // The request targets use.lua (where the cursor is), not the first file.
        let (analysis, _) = analyze(files);
        let use_path = analysis
            .files()
            .find(|p| p.ends_with("use.lua"))
            .unwrap()
            .to_path_buf();
        let sema = FileSema::new(&analysis, &use_path).unwrap();
        let changes = rename(&analysis, &sema, offset, "col")
            .expect("rename")
            .changes
            .expect("changes");
        assert_eq!(changes.len(), 2, "{changes:?}");
        let total: usize = changes.values().map(Vec::len).sum();
        // Two member accesses plus the `---@field x` declaration.
        assert_eq!(total, 3, "{changes:?}");
        assert_name_precise(files, &changes, "x");
    }

    #[test]
    fn prepare_rename_returns_the_identifier_range() {
        let src = "local value = 1\nprint(value)\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let offset = offset_of(src, "value", 1); // inside `print(value)`
        let range = prepare_rename(&analysis, &sema, offset).expect("prepare");
        // `value` in `print(value)` is on line 1, columns 6..11.
        assert_eq!(range, sema.index.range(offset..offset + "value".len()));
        assert_eq!(range.start.line, 1);
        assert_eq!(range.start.character, 6);
        assert_eq!(range.end.character, 11);
    }

    #[test]
    fn prepare_rename_on_a_non_symbol_is_none() {
        // The cursor sits on a numeric literal, not an identifier.
        let src = "local value = 1\n";
        let (analysis, path) = analyze(&[("main.lua", src)]);
        let sema = FileSema::new(&analysis, &path).unwrap();
        let offset = offset_of(src, "1", 0);
        assert!(prepare_rename(&analysis, &sema, offset).is_none());
    }
}
