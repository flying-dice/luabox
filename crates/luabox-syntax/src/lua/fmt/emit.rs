//! The canonical emitter behind [`super::format`]: walks the lossless tree
//! and regenerates every piece of whitespace.
//!
//! Every non-whitespace token in the tree is emitted exactly once (strings
//! possibly re-quoted), so token preservation holds by construction — and is
//! re-verified mechanically by the caller regardless.
//!
//! Layout model:
//! - Statements go one per line ([`Emitter::walk_block_like`]); blank-line
//!   runs between them collapse to a single blank line.
//! - Block-bodied constructs indent their bodies and put the closing
//!   keyword on its own line ([`Emitter::walk_structured`]); functions with
//!   empty bodies collapse to one line (`function f() end`).
//! - Tables render inline (`{ 1, 2, 3 }`) when they fit within the width and
//!   contain no comments, one field per line with trailing commas otherwise
//!   ([`Emitter::walk_table`]).
//! - Comments keep their line association: a comment on the same line as
//!   code stays trailing; a comment on its own line stays on its own line.

use rowan::NodeOrToken;

use super::strings::normalize_quotes;
use super::{Indent, LineEnding, Options};
#[allow(clippy::enum_glob_use)]
use crate::lua::SyntaxKind::{self, *};
use crate::lua::{SyntaxElement, SyntaxNode, SyntaxToken};

/// Render `root` (a `SOURCE_FILE`) canonically. Infallible; the caller
/// validates the result and falls back to the input if anything is off.
pub(super) fn emit(root: &SyntaxNode, opts: &Options) -> String {
    let mut emitter = Emitter::new(opts, false);
    emitter.walk_block_like(root);
    let mut out = emitter.out;
    if !out.is_empty() {
        out.push_str(opts.line_ending.as_str());
    }
    out
}

impl LineEnding {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// What the spacing rules need to know about the previously emitted token.
#[derive(Clone, Copy)]
struct Prev {
    kind: SyntaxKind,
    parent: SyntaxKind,
    last_char: char,
}

// The flags model orthogonal line state; folding them into an enum would
// obscure, not clarify.
#[allow(clippy::struct_excessive_bools)]
struct Emitter<'a> {
    opts: &'a Options,
    out: String,
    indent: usize,
    /// Width column of the current output line (tabs counted as 4).
    col: usize,
    line_has_content: bool,
    /// An emitted `--` line comment owns the rest of the line; the next
    /// token must break first.
    line_comment_open: bool,
    /// Blank-line requests are ignored right after an opening construct
    /// (start of file, start of body, start of table).
    suppress_blank: bool,
    /// Newlines seen in input whitespace since the last emission — the
    /// signal for blank-line and trailing-comment decisions.
    pending_newlines: usize,
    /// Probe mode: render single-line or fail (used to measure tables).
    probe: bool,
    failed: bool,
    prev: Option<Prev>,
}

impl<'a> Emitter<'a> {
    fn new(opts: &'a Options, probe: bool) -> Self {
        Emitter {
            opts,
            out: String::new(),
            indent: 0,
            col: 0,
            line_has_content: false,
            line_comment_open: false,
            suppress_blank: true,
            pending_newlines: 0,
            probe,
            failed: false,
            prev: None,
        }
    }

    // === Low-level printing ===

    fn force_newline(&mut self) {
        if self.probe {
            self.failed = true;
            return;
        }
        if self.line_has_content {
            self.out.push_str(self.opts.line_ending.as_str());
            self.col = 0;
            self.line_has_content = false;
            self.line_comment_open = false;
            self.prev = None;
        }
    }

    /// Break before a statement / table field / own-line comment: newline,
    /// plus one blank line when the input had any (collapse runs to one).
    fn item_break(&mut self) {
        self.force_newline();
        if self.pending_newlines >= 2 && !self.suppress_blank && !self.out.is_empty() {
            self.out.push_str(self.opts.line_ending.as_str());
        }
        self.pending_newlines = 0;
    }

    fn write_indent(&mut self) {
        match self.opts.indent {
            Indent::Spaces(n) => {
                let total = self.indent * usize::from(n);
                self.out.extend(std::iter::repeat_n(' ', total));
                self.col += total;
            }
            Indent::Tabs => {
                self.out.extend(std::iter::repeat_n('\t', self.indent));
                self.col += self.indent * 4;
            }
        }
    }

    /// Append `text` to the current line, breaking first if a line comment
    /// owns it, indenting if the line is fresh, and spacing per `space`
    /// (plus a token-merge guard: never glue `- -`, `[ [`, etc.).
    fn push_text(&mut self, text: &str, space: bool) {
        if self.probe && text.contains('\n') {
            self.failed = true;
            return;
        }
        if self.line_comment_open {
            self.force_newline();
        }
        if self.line_has_content {
            let glue_risk = must_separate(self.prev.map(|p| p.last_char), text.chars().next());
            if space || glue_risk {
                self.out.push(' ');
                self.col += 1;
            }
        } else {
            self.write_indent();
        }
        self.out.push_str(text);
        match text.rfind('\n') {
            Some(i) => self.col = text.len() - i - 1,
            None => self.col += text.len(),
        }
        self.line_has_content = true;
        self.suppress_blank = false;
        self.pending_newlines = 0;
    }

    /// Emit a non-trivia token with canonical spacing.
    fn token(&mut self, t: &SyntaxToken) {
        let parent = t.parent().map_or(ERROR, |p| p.kind());
        let normalized;
        let text = if t.kind() == STRING {
            match normalize_quotes(t.text(), self.opts.quotes) {
                Some(converted) => {
                    normalized = converted;
                    normalized.as_str()
                }
                None => t.text(),
            }
        } else {
            t.text()
        };
        let space = self
            .prev
            .is_some_and(|p| space_between(p, t.kind(), parent));
        self.push_text(text, space);
        self.prev = Some(Prev {
            kind: t.kind(),
            parent,
            last_char: text.chars().last().unwrap_or(' '),
        });
    }

    /// Emit a comment: trailing (same line) when the input had it on the
    /// same line as preceding code, on its own line otherwise.
    fn comment(&mut self, t: &SyntaxToken) {
        if self.probe {
            self.failed = true;
            return;
        }
        let text = t.text().trim_end();
        if !(self.line_has_content && self.pending_newlines == 0 && !self.line_comment_open) {
            self.item_break();
        }
        self.push_text(text, true);
        if !is_long_comment(text) {
            self.line_comment_open = true;
        }
        self.prev = Some(Prev {
            kind: COMMENT,
            parent: t.parent().map_or(ERROR, |p| p.kind()),
            last_char: text.chars().last().unwrap_or(' '),
        });
    }

    /// Append a formatter-inserted character (trailing table comma).
    fn insert_char(&mut self, c: char) {
        self.out.push(c);
        self.col += 1;
        self.prev = Some(Prev {
            kind: COMMA,
            parent: TABLE_EXPR,
            last_char: c,
        });
    }

    // === Tree walking ===

    fn walk(&mut self, node: &SyntaxNode) {
        if self.failed {
            return;
        }
        match node.kind() {
            SOURCE_FILE | BLOCK => self.walk_block_like(node),
            TABLE_EXPR => self.walk_table(node),
            IF_STMT | ELSEIF_CLAUSE | ELSE_CLAUSE | WHILE_STMT | DO_STMT | NUMERIC_FOR_STMT
            | GENERIC_FOR_STMT | REPEAT_STMT | FUNCTION_DECL_STMT | LOCAL_FUNCTION_STMT
            | FUNCTION_EXPR => self.walk_structured(node),
            _ => self.walk_inline(node),
        }
    }

    /// `SOURCE_FILE` / `BLOCK`: one statement per line; semicolons stay
    /// glued to the statement they follow.
    fn walk_block_like(&mut self, node: &SyntaxNode) {
        for el in node.children_with_tokens() {
            if self.failed {
                return;
            }
            match el {
                NodeOrToken::Token(t) => match t.kind() {
                    WHITESPACE => self.pending_newlines += count_newlines(t.text()),
                    COMMENT => self.comment(&t),
                    SEMICOLON => {
                        if self.pending_newlines > 0 || !self.line_has_content {
                            self.item_break();
                        }
                        self.token(&t);
                    }
                    _ => self.token(&t),
                },
                NodeOrToken::Node(n) => {
                    if n.kind() == BLOCK {
                        // SOURCE_FILE's single BLOCK: statements break
                        // themselves.
                        self.walk(&n);
                    } else {
                        self.item_break();
                        self.walk(&n);
                    }
                }
            }
        }
    }

    /// Generic in-order emission with canonical spacing; no layout of its
    /// own. Nested nodes may still break lines internally.
    fn walk_inline(&mut self, node: &SyntaxNode) {
        for el in node.children_with_tokens() {
            if self.failed {
                return;
            }
            match el {
                NodeOrToken::Token(t) => match t.kind() {
                    WHITESPACE => self.pending_newlines += count_newlines(t.text()),
                    COMMENT => self.comment(&t),
                    _ => self.token(&t),
                },
                NodeOrToken::Node(n) => self.walk(&n),
            }
        }
    }

    /// Block-bodied constructs: header inline, body indented one level,
    /// closing keyword on its own line. Functions with genuinely empty
    /// bodies collapse to a single line.
    fn walk_structured(&mut self, node: &SyntaxNode) {
        let base = self.indent;
        let collapse = is_function(node.kind()) && function_body_is_empty(node);
        let mut opened = false;
        for el in node.children_with_tokens() {
            if self.failed {
                return;
            }
            match el {
                NodeOrToken::Token(t) => match t.kind() {
                    WHITESPACE => self.pending_newlines += count_newlines(t.text()),
                    COMMENT => self.comment(&t),
                    END_KW | UNTIL_KW => {
                        self.indent = base;
                        if opened {
                            self.force_newline();
                            self.pending_newlines = 0;
                        }
                        self.token(&t);
                    }
                    kind => {
                        self.token(&t);
                        if !collapse && is_header_end(node.kind(), kind) {
                            self.indent += 1;
                            self.suppress_blank = true;
                            opened = true;
                        }
                    }
                },
                NodeOrToken::Node(n) => match n.kind() {
                    ELSEIF_CLAUSE | ELSE_CLAUSE => {
                        self.indent = base;
                        self.force_newline();
                        self.pending_newlines = 0;
                        self.walk_structured(&n);
                    }
                    PARAM_LIST => {
                        self.walk_inline(&n);
                        if !collapse && is_function(node.kind()) {
                            self.indent += 1;
                            self.suppress_blank = true;
                            opened = true;
                        }
                    }
                    _ => self.walk(&n),
                },
            }
        }
        self.indent = base;
    }

    /// Table constructor: inline when it fits and holds no comments,
    /// one field per line with trailing separators otherwise.
    fn walk_table(&mut self, node: &SyntaxNode) {
        if self.probe {
            self.walk_table_inline(node);
            return;
        }
        if let Some(inline) = self.try_inline(node) {
            // +1 for the potential space in front of `{`.
            if self.col + 1 + inline.len() <= self.opts.width {
                let space = self
                    .prev
                    .is_some_and(|p| space_between(p, L_BRACE, TABLE_EXPR));
                self.push_text(&inline, space);
                self.prev = Some(Prev {
                    kind: R_BRACE,
                    parent: TABLE_EXPR,
                    last_char: '}',
                });
                return;
            }
        }
        let base = self.indent;
        for el in node.children_with_tokens() {
            if self.failed {
                return;
            }
            match el {
                NodeOrToken::Token(t) => match t.kind() {
                    WHITESPACE => self.pending_newlines += count_newlines(t.text()),
                    COMMENT => self.comment(&t),
                    L_BRACE => {
                        self.token(&t);
                        self.indent += 1;
                        self.suppress_blank = true;
                    }
                    R_BRACE => {
                        self.indent = base;
                        self.force_newline();
                        self.pending_newlines = 0;
                        self.token(&t);
                    }
                    _ => self.token(&t),
                },
                NodeOrToken::Node(n) => {
                    self.item_break();
                    self.walk(&n);
                    if self.opts.trailing_table_comma
                        && next_significant(&NodeOrToken::Node(n.clone()))
                            .is_some_and(|next| element_kind(&next) == R_BRACE)
                    {
                        self.insert_char(',');
                    }
                }
            }
        }
    }

    /// Probe/inline rendering of a table: `{ a, b }` with existing
    /// separators kept (minus a trailing one) — or failure when a comment
    /// or a line break makes inlining impossible.
    fn walk_table_inline(&mut self, node: &SyntaxNode) {
        for el in node.children_with_tokens() {
            if self.failed {
                return;
            }
            match el {
                NodeOrToken::Token(t) => match t.kind() {
                    WHITESPACE => {}
                    COMMENT => self.failed = true,
                    COMMA | SEMICOLON => {
                        if !separator_is_trailing(&t) {
                            self.token(&t);
                        }
                    }
                    _ => self.token(&t),
                },
                NodeOrToken::Node(n) => self.walk(&n),
            }
        }
    }

    fn try_inline(&self, node: &SyntaxNode) -> Option<String> {
        let mut probe = Emitter::new(self.opts, true);
        probe.walk_table_inline(node);
        (!probe.failed).then_some(probe.out)
    }
}

// === Spacing rules ===

/// Canonical single-space decisions between adjacent tokens on one line.
fn space_between(prev: Prev, next: SyntaxKind, next_parent: SyntaxKind) -> bool {
    // After opening delimiters and glue punctuation: never.
    match prev.kind {
        L_PAREN | L_BRACKET | DOT | COLON | COLON_COLON => return false,
        // Inline tables carry spaces inside the braces (`{ 1 }`), except
        // when empty (`{}`).
        L_BRACE => return next != R_BRACE,
        _ => {}
    }
    // Unary `-`, `~`, `#` glue to their operand (`not` keeps its space).
    if prev.parent == PREFIX_EXPR && matches!(prev.kind, MINUS | TILDE | HASH) {
        return false;
    }
    // `<const>` / `<close>` glue inside the angle brackets.
    if prev.parent == NAME_ATTRIB && prev.kind == LT {
        return false;
    }
    match next {
        COMMA | SEMICOLON | R_PAREN | R_BRACKET | DOT | COLON | COLON_COLON => false,
        // Call/parameter parens hug the callee/name; grouping parens space
        // like any operand.
        L_PAREN => !matches!(next_parent, ARG_LIST | PARAM_LIST),
        // Indexing hugs (`t[k]`); table keys (`[k] = v`) space normally.
        L_BRACKET => next_parent != INDEX_EXPR,
        GT if next_parent == NAME_ATTRIB => false,
        _ => true,
    }
}

/// Merge guard: adjacent token texts that would lex as one token (or as a
/// comment) must be separated even where the style says "no space".
fn must_separate(prev_last: Option<char>, next_first: Option<char>) -> bool {
    let (Some(a), Some(b)) = (prev_last, next_first) else {
        return false;
    };
    let word = |c: char| c.is_alphanumeric() || c == '_';
    (word(a) && word(b))
        || (a == '-' && b == '-')
        || (a == '[' && (b == '[' || b == '='))
        || (a == '.' && (b == '.' || b.is_ascii_digit()))
        || (a == '<' && (b == '<' || b == '='))
        || (a == '>' && (b == '>' || b == '='))
        || (a == '=' && b == '=')
        || (a == '~' && b == '=')
        || (a == '/' && b == '/')
}

// === Structure predicates ===

fn is_function(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        FUNCTION_DECL_STMT | LOCAL_FUNCTION_STMT | FUNCTION_EXPR
    )
}

/// The token that ends a construct's header line and opens its body.
fn is_header_end(node: SyntaxKind, token: SyntaxKind) -> bool {
    matches!(
        (node, token),
        (IF_STMT | ELSEIF_CLAUSE, THEN_KW)
            | (
                WHILE_STMT | DO_STMT | NUMERIC_FOR_STMT | GENERIC_FOR_STMT,
                DO_KW
            )
            | (REPEAT_STMT, REPEAT_KW)
            | (ELSE_CLAUSE, ELSE_KW)
    )
}

/// True when a function's body block holds nothing at all (no statements,
/// no semicolons, no comments anywhere after the parameter list) — the
/// `function f() end` collapse case.
fn function_body_is_empty(node: &SyntaxNode) -> bool {
    let mut past_params = false;
    for el in node.children_with_tokens() {
        match el {
            NodeOrToken::Node(n) if n.kind() == PARAM_LIST => past_params = true,
            NodeOrToken::Node(n) if n.kind() == BLOCK => {
                let empty = n
                    .children_with_tokens()
                    .all(|el| matches!(&el, NodeOrToken::Token(t) if t.kind() == WHITESPACE));
                if !empty {
                    return false;
                }
            }
            NodeOrToken::Token(t) if t.kind() == COMMENT && past_params => return false,
            _ => {}
        }
    }
    true
}

// === Element helpers ===

fn element_kind(el: &SyntaxElement) -> SyntaxKind {
    match el {
        NodeOrToken::Node(n) => n.kind(),
        NodeOrToken::Token(t) => t.kind(),
    }
}

/// A table separator with no field after it (only more separators, then
/// `}`) is a trailing separator: dropped when the table renders inline.
/// Runs of separators must all count as trailing, or a second format pass
/// would drop what the first one kept (idempotence).
fn separator_is_trailing(t: &SyntaxToken) -> bool {
    let mut cur = next_significant(&NodeOrToken::Token(t.clone()));
    while let Some(el) = cur {
        match element_kind(&el) {
            R_BRACE => return true,
            COMMA | SEMICOLON => cur = next_significant(&el),
            _ => return false,
        }
    }
    true
}

/// The next sibling element skipping trivia (whitespace and comments).
fn next_significant(el: &SyntaxElement) -> Option<SyntaxElement> {
    let mut cur = match el {
        NodeOrToken::Node(n) => n.next_sibling_or_token(),
        NodeOrToken::Token(t) => t.next_sibling_or_token(),
    };
    while let Some(next) = cur {
        if !element_kind(&next).is_trivia() {
            return Some(next);
        }
        cur = match &next {
            NodeOrToken::Node(n) => n.next_sibling_or_token(),
            NodeOrToken::Token(t) => t.next_sibling_or_token(),
        };
    }
    None
}

fn count_newlines(text: &str) -> usize {
    text.bytes().filter(|&b| b == b'\n').count()
}

/// `--[[ … ]]` (any level) vs `-- …`; only line comments own their line.
fn is_long_comment(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("--") else {
        return false;
    };
    let Some(after_bracket) = rest.strip_prefix('[') else {
        return false;
    };
    after_bracket.trim_start_matches('=').starts_with('[')
}
