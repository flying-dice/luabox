//! LSP formatting over the shipped canonical formatter (SPEC.md §10):
//! `.lua` through [`luabox_syntax::lua::fmt::format`] with the project's
//! edition.
//!
//! The formatter guarantees it never destroys code — inputs that do not
//! parse cleanly come back unchanged — so a formatting request on a broken
//! document yields **no edits**, never an error response.
//!
//! # Range formatting (MVP semantics)
//!
//! The canonical formatters are whole-file by design (idempotent, no range
//! API), so `textDocument/rangeFormatting` formats the **whole document**
//! and returns the same single edit as a full format. This is the
//! least-surprising pragmatic behaviour: the result is always the canonical
//! form the user would get on save, and a second request is a no-op.

use lsp_types::TextEdit;

use crate::line_index::LineIndex;

/// The edits that turn `original` into `formatted`: a single whole-document
/// replacement when the text changed, and **no edits** when it did not
/// (including the formatter's parse-error "return input unchanged" case).
#[must_use]
pub fn full_document_edits(original: &str, formatted: &str) -> Vec<TextEdit> {
    if original == formatted {
        return Vec::new();
    }
    let index = LineIndex::new(original);
    vec![TextEdit {
        range: index.range(0..original.len()),
        new_text: formatted.to_string(),
    }]
}
