//! The `Diagnostic` value and its span-rich parts.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::code::{Code, Severity};

/// A byte-offset region within a source file.
///
/// Ranges are half-open byte offsets (`start..end`); line/column are computed
/// at render time from the source text, so a diagnostic carries no rendering
/// state and can be produced without the source in hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The file the range refers to (path or logical name).
    pub file: String,
    /// Half-open byte range within `file`.
    pub range: Range<usize>,
}

impl Span {
    /// Construct a span from a file name and byte range.
    pub fn new(file: impl Into<String>, range: Range<usize>) -> Self {
        Self {
            file: file.into(),
            range,
        }
    }
}

/// A labelled span attached to a diagnostic.
///
/// Exactly one label is usually `primary` (the site of the error, rendered
/// with `^`); the rest are secondary context (rendered with `-`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    /// The region this label points at.
    pub span: Span,
    /// The message rendered under the span.
    pub message: String,
    /// Whether this is the primary (error-site) label.
    pub primary: bool,
}

impl Label {
    /// A primary label — the site the diagnostic is really about.
    pub fn primary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            primary: true,
        }
    }

    /// A secondary label — supporting context elsewhere.
    pub fn secondary(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
            primary: false,
        }
    }
}

/// A machine-applicable suggestion: replace `span` with `replacement`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// The region to replace.
    pub span: Span,
    /// The text to substitute for the region's current contents.
    pub replacement: String,
    /// A human description of the fix.
    pub message: String,
}

impl Suggestion {
    /// Construct a suggestion.
    pub fn new(span: Span, replacement: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            span,
            replacement: replacement.into(),
            message: message.into(),
        }
    }
}

/// A single diagnostic: a coded, severity-tagged message with optional
/// labels, suggestions, and free-form notes.
///
/// Built fluently:
///
/// ```
/// use luabox_diag::{Code, Diagnostic, Label, Span};
///
/// let code: Code = "LB0001".parse().unwrap();
/// let diag = Diagnostic::error(code, "unexpected token")
///     .with_label(Label::primary(Span::new("a.lua", 4..9), "expected an identifier"))
///     .with_note("Lua identifiers may not start with a digit.");
/// assert_eq!(diag.labels.len(), 1);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// The `LBnnnn` code.
    pub code: Code,
    /// How loud this diagnostic is.
    pub severity: Severity,
    /// The one-line headline message.
    pub message: String,
    /// Labelled spans (primary + secondary).
    pub labels: Vec<Label>,
    /// Machine-applicable fixes.
    pub suggestions: Vec<Suggestion>,
    /// Free-form trailing notes.
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Construct a diagnostic with an explicit severity.
    pub fn new(code: Code, severity: Severity, message: impl Into<String>) -> Self {
        Self {
            code,
            severity,
            message: message.into(),
            labels: Vec::new(),
            suggestions: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// An error-severity diagnostic.
    pub fn error(code: Code, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, message)
    }

    /// A warning-severity diagnostic.
    pub fn warning(code: Code, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Warning, message)
    }

    /// A note-severity diagnostic.
    pub fn note(code: Code, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Note, message)
    }

    /// A help-severity diagnostic.
    pub fn help(code: Code, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Help, message)
    }

    /// Attach a label.
    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    /// Attach a suggestion.
    #[must_use]
    pub fn with_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Attach a trailing note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// The primary label, if one was set (first one wins).
    #[must_use]
    pub fn primary_label(&self) -> Option<&Label> {
        self.labels.iter().find(|l| l.primary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_accumulates() {
        let code: Code = "LB0001".parse().unwrap();
        let diag = Diagnostic::error(code, "boom")
            .with_label(Label::primary(Span::new("a.lua", 0..3), "here"))
            .with_label(Label::secondary(Span::new("a.lua", 5..6), "and here"))
            .with_suggestion(Suggestion::new(Span::new("a.lua", 0..3), "fix", "try this"))
            .with_note("a note");

        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.labels.len(), 2);
        assert_eq!(diag.suggestions.len(), 1);
        assert_eq!(diag.notes, vec!["a note".to_string()]);
        assert_eq!(diag.primary_label().unwrap().message, "here");
    }
}
