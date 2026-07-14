//! `--minify`: scope-aware identifier mangling plus whitespace collapse
//! (SPEC.md §7).
//!
//! # What is renamed — and what never is
//!
//! Renaming is driven by the HIR resolution tables (`luabox-hir`), not
//! text: every **local binding** (locals, parameters, loop variables,
//! `local function` names) and every **label** gets a fresh short name;
//! every name expression the resolver tied to a renamed binding is
//! rewritten to match. Everything else is untouched by construction:
//!
//! - **globals** — `Resolution::Global` names are never edited;
//! - **table fields / properties** — `a.b`, `o:m()`, `{ k = v }` carry
//!   their keys as synthesized string literals or method names in the HIR,
//!   not name expressions, so the renamer cannot even see them;
//! - the implicit **`self`** parameter and every reference to it;
//! - string and number literals (constant folding is a follow-up — it
//!   needs dialect-faithful constant evaluation, not textual rewriting).
//!
//! Fresh names are drawn from `a, b, …, z, a0, a1, …` skipping Lua
//! keywords and **every identifier that occurs anywhere in the module**
//! (cheap and capture-proof: a fresh name can never collide with a global,
//! a property, or anything left unrenamed; distinct bindings always get
//! distinct fresh names, so renamed bindings cannot capture each other).
//!
//! Labels are renamed only when every `goto` site is textually plain
//! (`goto name` with nothing but whitespace between); a label with any
//! exotic site (comments inside the statement) keeps its original name at
//! the definition and every use — consistency over aggressiveness.
//!
//! # Whitespace collapse
//!
//! The token stream is re-emitted with comments dropped and exactly the
//! separators Lua's lexer needs (identifier/keyword/number adjacency,
//! `- -`, `[ [`, `. .`, operator-merge pairs like `< <`). Long strings are
//! kept verbatim. The result is one line per module.
//!
//! # Mechanical guarantee
//!
//! [`minify`] reparses its own output under the same dialect and fails
//! (rather than emitting) if the result no longer parses — the same
//! property-style check the formatter uses.

use std::collections::{HashMap, HashSet};

use luabox_hir::{BindingKind, Expr, HirId, LabelId, LoweredFile, Resolution, Stmt};
use luabox_syntax::{Dialect, SyntaxKind, lua};
use rowan::TextRange;

/// Minify one module's text. `Err` carries an internal-invariant message
/// (input not parseable, or output failed the reparse check) — callers
/// treat it as a bundler bug, never a user diagnostic.
pub(crate) fn minify(text: &str, dialect: Dialect) -> Result<String, String> {
    let renamed = rename(text, dialect)?;
    let collapsed = collapse(&renamed, dialect);
    let reparse = lua::parse(&collapsed, dialect);
    if let Some(err) = reparse.errors().first() {
        return Err(format!(
            "minified output no longer parses (bundler bug): {}",
            err.message
        ));
    }
    Ok(collapsed)
}

/// Every Lua keyword across the supported dialects — never generated as a
/// fresh name (`goto` is a keyword only in 5.2+/LuaJIT, but skipping it
/// everywhere costs one name).
const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// Scope-aware binding + label renaming via the HIR resolution tables.
fn rename(text: &str, dialect: Dialect) -> Result<String, String> {
    let parse = lua::parse(text, dialect);
    if let Some(err) = parse.errors().first() {
        return Err(format!("minify input does not parse: {}", err.message));
    }
    let file = luabox_hir::lower(&parse);

    let mut names = NameGen::new(text, dialect);
    let mut edits: Vec<(TextRange, String)> = Vec::new();

    // Bindings: one fresh name per binding, edited at the declaration site.
    let mut fresh: HashMap<luabox_hir::BindingId, String> = HashMap::new();
    for (id, binding) in file.bindings() {
        if binding.kind == BindingKind::SelfParam {
            continue; // implicit `self` has no spellable declaration
        }
        let name = names.fresh();
        edits.push((binding.range, name.clone()));
        fresh.insert(id, name);
    }

    // References: every name expression resolved to a renamed binding.
    for (body_id, body) in file.bodies() {
        for (expr_id, expr) in body.exprs() {
            if !matches!(expr, Expr::Name(_)) {
                continue;
            }
            let hir_id = HirId::expr(body_id, expr_id);
            let binding = match file.resolution(hir_id) {
                Some(Resolution::Local(b)) => *b,
                Some(Resolution::Upvalue { binding, .. }) => *binding,
                _ => continue,
            };
            if let Some(name) = fresh.get(&binding)
                && let Some(range) = file.source_map().range(hir_id)
            {
                edits.push((range, name.clone()));
            }
        }
    }

    edits.extend(label_edits(text, &file, &mut names));

    // Apply back-to-front; ranges are disjoint identifier tokens.
    edits.sort_by_key(|(range, _)| (range.start(), range.end()));
    edits.dedup_by_key(|(range, _)| *range);
    let mut out = text.to_owned();
    for (range, replacement) in edits.into_iter().rev() {
        out.replace_range(
            usize::from(range.start())..usize::from(range.end()),
            &replacement,
        );
    }
    Ok(out)
}

/// Rename labels: the definition site range is the name token (from the
/// HIR label table); each `goto` site's name token is located textually
/// inside the statement range. A label with any non-plain `goto` site is
/// left alone entirely.
fn label_edits(text: &str, file: &LoweredFile, names: &mut NameGen) -> Vec<(TextRange, String)> {
    // Group sites per label: the definition plus every goto naming it.
    let mut sites: HashMap<LabelId, Option<Vec<TextRange>>> = HashMap::new();
    for (body_id, body) in file.bodies() {
        for (stmt_id, stmt) in body.stmts() {
            match stmt {
                Stmt::Label { label, .. } => {
                    let range = file.label(*label).range;
                    if let Some(list) = sites.entry(*label).or_insert_with(|| Some(Vec::new())) {
                        list.push(range);
                    }
                }
                Stmt::Goto {
                    name,
                    target: Some(label),
                } => {
                    let entry = sites.entry(*label).or_insert_with(|| Some(Vec::new()));
                    let stmt_range = file.source_map().range(HirId::stmt(body_id, stmt_id));
                    match stmt_range.and_then(|r| goto_name_range(text, r, name)) {
                        Some(range) => {
                            if let Some(list) = entry {
                                list.push(range);
                            }
                        }
                        None => *entry = None, // exotic site: skip this label
                    }
                }
                _ => {}
            }
        }
    }
    let mut labels: Vec<_> = sites.into_iter().collect();
    labels.sort_by_key(|(id, _)| *id);
    let mut edits = Vec::new();
    for (_, ranges) in labels {
        let Some(ranges) = ranges else { continue };
        let name = names.fresh();
        edits.extend(ranges.into_iter().map(|r| (r, name.clone())));
    }
    edits
}

/// The range of `name` inside a plain `goto name` statement, or `None`
/// when the statement is not textually plain (e.g. a comment between the
/// keyword and the label).
fn goto_name_range(text: &str, stmt: TextRange, name: &str) -> Option<TextRange> {
    let slice = text.get(usize::from(stmt.start())..usize::from(stmt.end()))?;
    let after_kw = slice.strip_prefix("goto")?;
    let trimmed = after_kw.trim_start();
    if trimmed != name || after_kw.len() == trimmed.len() {
        return None; // trailing junk, or no separator after `goto`
    }
    let offset = usize::from(stmt.start()) + (slice.len() - trimmed.len());
    let start = u32::try_from(offset).ok()?;
    let len = u32::try_from(name.len()).ok()?;
    Some(TextRange::at(start.into(), len.into()))
}

/// Fresh short names: `a…z`, then `a0…z9, aa…zz`, … — skipping keywords
/// and every identifier occurring in the module (capture-proof by
/// construction; see module docs).
struct NameGen {
    next: usize,
    forbidden: HashSet<String>,
}

impl NameGen {
    fn new(text: &str, dialect: Dialect) -> Self {
        let mut forbidden: HashSet<String> = KEYWORDS.iter().map(ToString::to_string).collect();
        forbidden.insert("self".to_owned());
        let mut offset = 0usize;
        for token in lua::lex(text, dialect) {
            let end = offset + token.len as usize;
            if token.kind == SyntaxKind::IDENT {
                #[expect(
                    clippy::string_slice,
                    reason = "offset..end are consecutive lexer token byte spans that tile `text`, so they land on char boundaries"
                )]
                forbidden.insert(text[offset..end].to_owned());
            }
            offset = end;
        }
        Self { next: 0, forbidden }
    }

    fn fresh(&mut self) -> String {
        loop {
            let candidate = encode(self.next);
            self.next += 1;
            if !self.forbidden.contains(&candidate) {
                return candidate;
            }
        }
    }
}

/// Bijective encoding of an index into a valid identifier: a leading
/// letter, then base-36 alphanumeric tail digits.
fn encode(index: usize) -> String {
    const HEAD: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    const TAIL: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = String::new();
    out.push(HEAD[index % HEAD.len()] as char);
    let mut rest = index / HEAD.len();
    while rest > 0 {
        rest -= 1;
        out.push(TAIL[rest % TAIL.len()] as char);
        rest /= TAIL.len();
    }
    out
}

/// Re-emit the token stream with comments dropped and minimal separators.
fn collapse(text: &str, dialect: Dialect) -> String {
    let mut out = String::with_capacity(text.len() / 2);
    let mut offset = 0usize;
    let mut prev: Option<(SyntaxKind, &str)> = None;
    for token in lua::lex(text, dialect) {
        let end = offset + token.len as usize;
        #[expect(
            clippy::string_slice,
            reason = "offset..end are consecutive lexer token byte spans that tile `text`, so they land on char boundaries"
        )]
        let slice = &text[offset..end];
        offset = end;
        if token.kind.is_trivia() {
            continue;
        }
        if let Some((prev_kind, prev_text)) = prev
            && needs_separator(prev_kind, prev_text, slice)
        {
            out.push(' ');
        }
        out.push_str(slice);
        prev = Some((token.kind, slice));
    }
    out
}

/// Whether Lua's lexer needs a separator between two adjacent tokens to
/// keep tokenizing them the same way. Conservative: every pair whose
/// concatenation could lex as one longer token gets a space.
fn needs_separator(prev_kind: SyntaxKind, prev: &str, next: &str) -> bool {
    let ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let (Some(a), Some(b)) = (prev.chars().last(), next.chars().next()) else {
        return false;
    };
    // `local x`, `and not`, `0x1 and`, `n1 n2` (never grammatical, but safe).
    if ident_char(a) && ident_char(b) {
        return true;
    }
    // `1 ..`, `1 .5`: a number followed by a dot would extend the number.
    if prev_kind == SyntaxKind::NUMBER && b == '.' {
        return true;
    }
    // `. .` / `.. .` would merge into `..` / `...`; `- -` into a comment;
    // `[ [` / `[ =` into a long-bracket opener.
    if (a == '.' && b == '.') || (a == '-' && b == '-') || (a == '[' && (b == '[' || b == '=')) {
        return true;
    }
    // Operator-merge guards: `= =`, `~ =`, `< =`, `> =`, `< <`, `> >`,
    // `/ /`, `: :` — mostly ungrammatical as separate tokens, all cheap.
    matches!(
        (a, b),
        ('=' | '~' | '<' | '>', '=') | ('<', '<') | ('>', '>') | ('/', '/') | (':', ':')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_is_injective_and_identifier_shaped() {
        let mut seen = HashSet::new();
        for i in 0..2000 {
            let name = encode(i);
            assert!(name.chars().next().unwrap().is_ascii_lowercase());
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric()));
            assert!(seen.insert(name), "duplicate at {i}");
        }
    }

    #[test]
    fn locals_renamed_globals_and_fields_untouched() {
        let out = minify(
            "local counter = 0\nfunction tick(step)\n  counter = counter + step\n  \
             registry.total = counter\nend\ntick(2)\nprint(registry.total)\n",
            Dialect::Lua54,
        )
        .expect("minify");
        assert!(!out.contains("counter"), "local renamed: {out}");
        assert!(!out.contains("step"), "param renamed: {out}");
        assert!(out.contains("tick"), "global function name kept: {out}");
        assert!(out.contains("registry.total"), "property chain kept: {out}");
        assert!(out.contains("print"), "global kept: {out}");
    }

    #[test]
    fn self_and_method_names_survive() {
        let out = minify(
            "local M = {}\nfunction M:area(scale)\n  return self.width * scale\nend\nreturn M\n",
            Dialect::Lua54,
        )
        .expect("minify");
        assert!(out.contains("self.width"), "{out}");
        assert!(out.contains(":area("), "{out}");
        assert!(!out.contains("scale"), "{out}");
    }

    #[test]
    fn labels_renamed_with_their_gotos() {
        let out = minify(
            "local i = 0\n::top::\ni = i + 1\nif i < 3 then goto top end\nprint(i)\n",
            Dialect::Lua54,
        )
        .expect("minify");
        assert!(!out.contains("top"), "label renamed: {out}");
        assert!(out.contains("goto "), "goto still present: {out}");
        assert!(out.contains("::"), "label still present: {out}");
    }

    #[test]
    fn collapse_keeps_lexical_boundaries() {
        for (input, must_contain) in [
            ("local x = 1 .. 2\n", "1 .."),
            ("local y = - -1\n", "- -"),
            ("local t = {}\nlocal v = t[ [[key]] ]\n", "[ [[key]]"),
        ] {
            let out = minify(input, Dialect::Lua54).expect("minify");
            assert!(out.contains(must_contain), "{input:?} -> {out}");
        }
    }

    #[test]
    fn shadowed_locals_get_distinct_names() {
        let out = minify(
            "local v = 1\ndo\n  local v = 2\n  print(v)\nend\nprint(v)\n",
            Dialect::Lua54,
        )
        .expect("minify");
        // Two bindings, two fresh names — the inner print must not see the
        // outer binding's fresh name.
        assert!(out.starts_with("local a=1"), "{out}");
        assert!(out.contains("local b=2"), "{out}");
        assert!(out.contains("print(b)"), "{out}");
        assert!(out.ends_with("print(a)"), "{out}");
    }
}
