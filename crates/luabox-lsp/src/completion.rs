//! Completion: after `.`/`:` on a receiver with a known class type, its
//! fields and methods; otherwise scope-visible locals, file-declared
//! globals/functions, and keywords. Deduplicated and sorted.

use std::collections::BTreeMap;

use lsp_types::{CompletionItem, CompletionItemKind};
use luabox_hir::BindingKind;
use luabox_syntax::luacats::FieldKey;

use crate::sema::{self, FileSema};

/// Lua keywords offered in plain (non-member) positions.
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Compute completions at `offset` (the cursor's byte offset).
#[must_use]
pub fn completion(sema: &FileSema, offset: usize) -> Vec<CompletionItem> {
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

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
