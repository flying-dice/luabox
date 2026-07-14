//! Completion: after `.`/`:` on a receiver with a known class type, its
//! fields and methods; otherwise scope-visible locals, file-declared
//! globals/functions, and keywords. In a plain position the scope items are
//! augmented with **auto-require imports** (tsc-style): names exported by
//! other workspace modules but not yet in scope, each carrying an
//! `additionalTextEdits` insert of `local <name> = require("<module>").<name>`.
//! Deduplicated and sorted.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Position, Range, TextEdit,
};
use luabox_db::Analysis;
use luabox_hir::BindingKind;
use luabox_syntax::luacats::FieldKey;
use luabox_types::ty::Ty;

use crate::sema::{self, FileSema};

/// Lua keywords offered in plain (non-member) positions.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Compute completions at `offset` (the cursor's byte offset). `analysis` and
/// `project_root` back the auto-require pass — enumerating other modules'
/// exports and reversing each target file to its `require` module path.
#[must_use]
pub fn completion(
    sema: &FileSema,
    offset: usize,
    analysis: &Analysis,
    project_root: &Path,
) -> Vec<CompletionItem> {
    let text = sema.index.text();
    let bytes = text.as_bytes();
    let offset = offset.min(bytes.len());

    // The identifier prefix being typed, and what precedes it.
    let mut start = offset;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let trigger = match start.checked_sub(1).map(|i| bytes[i]) {
        // `..` is concat, not member access.
        Some(b'.') if start < 2 || bytes[start - 2] != b'.' => Some(b'.'),
        Some(b':') => Some(b':'),
        _ => None,
    };

    let mut items: BTreeMap<String, CompletionItem> = BTreeMap::new();
    if let Some(trigger) = trigger {
        member_items(sema, text, start - 1, trigger, &mut items);
    } else {
        scope_items(sema, offset, &mut items);
        // Auto-require runs after scope items so names already in scope
        // (present in `items`) are left untouched.
        #[expect(
            clippy::string_slice,
            reason = "offset is a LineIndex byte offset (a char boundary); start walks back over ASCII identifier bytes, so it is one too"
        )]
        let prefix = &text[start..offset];
        auto_require_items(sema, offset, prefix, analysis, project_root, &mut items);
    }
    items.into_values().collect()
}

/// Fields/methods of the receiver identifier ending at `dot_offset`.
fn member_items(
    sema: &FileSema,
    text: &str,
    dot_offset: usize,
    trigger: u8,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    let bytes = text.as_bytes();
    let mut recv_start = dot_offset;
    while recv_start > 0 && is_ident_byte(bytes[recv_start - 1]) {
        recv_start -= 1;
    }
    if recv_start == dot_offset {
        return;
    }
    #[expect(
        clippy::string_slice,
        reason = "dot_offset indexes an ASCII `.`/`:` and recv_start walks back over ASCII identifier bytes, so both are char boundaries"
    )]
    let receiver = &text[recv_start..dot_offset];
    let Some(class) = sema.class_of_name(receiver, recv_start) else {
        return;
    };
    for (field, declaring) in sema.class_fields(&class) {
        let FieldKey::Name(name) = &field.key else {
            continue;
        };
        let is_fun = sema::is_function_type(&field.ty);
        // After `:` only methods make sense.
        if trigger == b':' && !is_fun {
            continue;
        }
        let kind = if is_fun {
            if trigger == b':' {
                CompletionItemKind::METHOD
            } else {
                CompletionItemKind::FUNCTION
            }
        } else {
            CompletionItemKind::FIELD
        };
        items.insert(
            name.clone(),
            CompletionItem {
                label: name.clone(),
                kind: Some(kind),
                detail: Some(format!(
                    "{declaring}.{name}: {}",
                    sema::render_type(&field.ty)
                )),
                ..CompletionItem::default()
            },
        );
    }
}

/// Locals visible at `offset`, file-declared functions/globals, and keywords.
fn scope_items(sema: &FileSema, offset: usize, items: &mut BTreeMap<String, CompletionItem>) {
    for keyword in KEYWORDS {
        items.insert(
            (*keyword).to_string(),
            CompletionItem {
                label: (*keyword).to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                ..CompletionItem::default()
            },
        );
    }
    for (name, _) in sema.global_defs() {
        items.insert(
            name.clone(),
            CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::VARIABLE),
                ..CompletionItem::default()
            },
        );
    }
    for info in sema.functions() {
        // Methods complete after `:`, not in plain scope.
        if info.name.contains(':') {
            continue;
        }
        items.insert(
            info.name.clone(),
            CompletionItem {
                label: info.name.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(info.sig),
                ..CompletionItem::default()
            },
        );
    }
    // Locals last: they override same-named globals/keywords in the map.
    for binding in sema.bindings_before(offset) {
        let kind = if binding.kind == BindingKind::LocalFunction {
            CompletionItemKind::FUNCTION
        } else {
            CompletionItemKind::VARIABLE
        };
        let detail = sema.binding_type(binding).map(|ty| sema::render_type(&ty));
        items.insert(
            binding.name.clone(),
            CompletionItem {
                label: binding.name.clone(),
                kind: Some(kind),
                detail,
                ..CompletionItem::default()
            },
        );
    }
}

/// Append auto-require import completions: for every other workspace module,
/// each exported name that matches `prefix`, is not already in scope, and
/// whose module is not already required here, offered with an
/// `additionalTextEdits` insert of `local <name> = require("<module>").<name>`.
///
/// Runs on every plain-position completion; the per-file `module_export` is
/// salsa-memoized, so the cost is one cached lookup per workspace file. An
/// empty prefix is skipped — auto-require is a targeted, prefix-driven suggest,
/// not a dump of the whole workspace surface on every keystroke.
fn auto_require_items(
    sema: &FileSema,
    offset: usize,
    prefix: &str,
    analysis: &Analysis,
    project_root: &Path,
    items: &mut BTreeMap<String, CompletionItem>,
) {
    if prefix.is_empty() {
        return;
    }

    // Modules this file already `require`s, under any local name.
    let required: HashSet<&str> = sema.requires().iter().map(|e| e.module.as_str()).collect();

    // The require insert lands on its own line; placement is shared, the
    // statement text differs per candidate (the editor applies only the
    // accepted item's edit).
    let (anchor, leading) = require_anchor(sema);

    // Deterministic first-wins when two modules export the same name.
    let mut files: Vec<&Path> = analysis.files().filter(|&f| f != sema.path).collect();
    files.sort_unstable();

    for file in files {
        let Some(module) = module_path(project_root, file) else {
            continue;
        };
        if required.contains(module.as_str()) {
            continue;
        }
        let Some(export) = analysis.module_export(file) else {
            continue;
        };
        let Some(Ty::Table(table)) = export.ty() else {
            continue;
        };
        for (name, field) in &table.fields {
            if !name.starts_with(prefix) {
                continue;
            }
            // Already in scope (a local/upvalue) or already offered by scope
            // completion (a global/function/keyword) — leave it be.
            if items.contains_key(name) || sema.visible_binding_named(name, offset).is_some() {
                continue;
            }
            let kind = if matches!(field.ty, Ty::Function(_)) {
                CompletionItemKind::FUNCTION
            } else {
                CompletionItemKind::VARIABLE
            };
            // `name` is a field of the module's exported table, so bind the
            // field itself (`require("m").name`) — binding the whole module to
            // a field-named local would make `name(...)` call the table.
            let stmt = format!("local {name} = require(\"{module}\").{name}");
            let new_text = if leading {
                format!("\n{stmt}")
            } else {
                format!("{stmt}\n")
            };
            items.insert(
                name.clone(),
                CompletionItem {
                    label: name.clone(),
                    kind: Some(kind),
                    detail: Some(format!("Auto import from \"{module}\"")),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: None,
                        description: Some(module.clone()),
                    }),
                    additional_text_edits: Some(vec![TextEdit {
                        range: Range::new(anchor, anchor),
                        new_text,
                    }]),
                    ..CompletionItem::default()
                },
            );
        }
    }
}

/// Where a new `require` line should be inserted: the LSP [`Position`] anchor
/// (a zero-width point) and whether the statement text must be prefixed with a
/// newline (`leading`) rather than suffixed. Placed after the file's last
/// existing `require` statement, else after any leading comment/`---@meta`
/// header, else at the very top.
fn require_anchor(sema: &FileSema) -> (Position, bool) {
    let text = sema.index.text();
    let offset = require_insertion_offset(sema, text);
    // Prefix a newline when the anchor is not itself a line start (an
    // unterminated final line); otherwise suffix one so the statement occupies
    // its own line and pushes any following code down.
    let leading = offset > 0 && text.as_bytes().get(offset - 1) != Some(&b'\n');
    (sema.index.position(offset), leading)
}

/// The byte offset of the line start where a new `require` should be inserted.
fn require_insertion_offset(sema: &FileSema, text: &str) -> usize {
    // 1. Just after the last existing `require(...)` statement's line.
    if let Some(end) = sema
        .requires()
        .iter()
        .map(|e| usize::from(e.range.end()))
        .max()
    {
        #[expect(
            clippy::string_slice,
            reason = "end is a rowan range end, which always lands on a char boundary"
        )]
        return match text[end..].find('\n') {
            Some(rel) => end + rel + 1,
            None => text.len(),
        };
    }
    // 2. After the leading comment / `---@meta` header (and blank lines).
    header_end(text)
}

/// The byte offset of the first line that is neither blank nor a `--` comment
/// — where a require belongs when the file has no existing requires.
fn header_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            offset += line.len();
        } else {
            break;
        }
    }
    offset
}

/// Reverse of `require` resolution: a workspace `.lua` file under `root` to its
/// module path (`<root>/a/b/c.lua` → `"a.b.c"`, `<root>/a/b/init.lua` →
/// `"a.b"`). `None` for a file outside `root` or a bare root `init.lua`.
fn module_path(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let mut segments: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let last = segments.last_mut()?;
    if last == "init.lua" {
        segments.pop();
    } else {
        *last = last.strip_suffix(".lua")?.to_string();
    }
    (!segments.is_empty()).then(|| segments.join("."))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// test code — panics document assumptions
#[allow(
    clippy::string_slice,
    reason = "test code — panics document assumptions"
)]
#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{header_end, module_path};

    #[test]
    fn module_path_reverses_require_resolution() {
        let root = Path::new("/proj");
        assert_eq!(
            module_path(root, Path::new("/proj/a/b/c.lua")).as_deref(),
            Some("a.b.c")
        );
        assert_eq!(
            module_path(root, Path::new("/proj/geometry.lua")).as_deref(),
            Some("geometry")
        );
    }

    #[test]
    fn module_path_handles_init_lua() {
        let root = Path::new("/proj");
        // `<dir>/init.lua` is the module `<dir>`, not `<dir>.init`.
        assert_eq!(
            module_path(root, Path::new("/proj/foo/init.lua")).as_deref(),
            Some("foo")
        );
        assert_eq!(
            module_path(root, Path::new("/proj/a/b/init.lua")).as_deref(),
            Some("a.b")
        );
        // A bare root `init.lua` reverses to an empty module: not offerable.
        assert_eq!(module_path(root, Path::new("/proj/init.lua")), None);
    }

    #[test]
    fn module_path_rejects_files_outside_root_and_non_lua() {
        let root = Path::new("/proj");
        assert_eq!(module_path(root, Path::new("/other/x.lua")), None);
        assert_eq!(module_path(root, Path::new("/proj/x.txt")), None);
    }

    #[test]
    fn header_end_skips_leading_comments_and_blanks() {
        // Insert after a `---@meta`/comment header and its blank lines.
        let text = "---@meta\n-- a note\n\nlocal x = 1\n";
        let offset = header_end(text);
        assert_eq!(&text[..offset], "---@meta\n-- a note\n\n");
        // No header: insertion is the very top.
        assert_eq!(header_end("local x = 1\n"), 0);
        // All comments: insertion is the end of the file.
        let all = "-- one\n-- two\n";
        assert_eq!(header_end(all), all.len());
    }
}
