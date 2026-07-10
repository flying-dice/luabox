//! The rule-facing diagnostic type and its optional [`Fix`].
//!
//! Rules speak in byte ranges (`std::ops::Range<usize>`) and plain messages;
//! the orchestrator ([`crate::lint_source`]) stamps the rule id, `LB05xx`
//! code, and effective severity, and lowers the result into a
//! [`luabox_diag::Diagnostic`] for rendering.

use std::ops::Range;

/// A machine- (or human-) applicable edit: replace `range` with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    /// The byte range to replace.
    pub range: Range<usize>,
    /// The replacement text.
    pub replacement: String,
    /// Whether `--fix` may apply this automatically. Non-machine-applicable
    /// fixes are shown as suggestions but never written to disk.
    pub is_machine_applicable: bool,
}

impl Fix {
    /// A machine-applicable fix.
    #[must_use]
    pub fn machine(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
            is_machine_applicable: true,
        }
    }
}

/// A single finding produced by a [`crate::rule::Rule`].
#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    /// The primary span (byte range in the linted file).
    pub range: Range<usize>,
    /// The headline message.
    pub message: String,
    /// Secondary spans with their own messages (e.g. "shadows this binding").
    pub secondary: Vec<(Range<usize>, String)>,
    /// Free-form trailing notes.
    pub notes: Vec<String>,
    /// An optional fix.
    pub fix: Option<Fix>,
}

impl LintDiagnostic {
    /// A finding at `range` with `message` and no fix.
    #[must_use]
    pub fn new(range: Range<usize>, message: impl Into<String>) -> Self {
        Self {
            range,
            message: message.into(),
            secondary: Vec::new(),
            notes: Vec::new(),
            fix: None,
        }
    }

    /// Attach a secondary label.
    #[must_use]
    pub fn with_secondary(mut self, range: Range<usize>, message: impl Into<String>) -> Self {
        self.secondary.push((range, message.into()));
        self
    }

    /// Attach a trailing note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Attach a fix.
    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }
}
