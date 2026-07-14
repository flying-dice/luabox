//! Dialect-legality validation over parsed trees (SPEC.md §2).
//!
//! The lexer/parser accept the *union* of every dialect's grammar (see
//! [`super::parse`]); this pass walks the resulting tree and flags
//! constructs that are legal in the union grammar but illegal in the
//! `dialect` the caller configured. It never mutates the tree and never
//! rejects a construct the union grammar itself rejected — that is the
//! parser's job (`Parse::errors`).
//!
//! Each rule below corresponds to one delta row in SPEC.md §2.1 (the
//! lowering table); `luabox build` is the future fix for code that
//! legitimately wants the newer construct on an older `edition`.

use rowan::NodeOrToken;

use super::{Dialect, Parse, SyntaxKind};

/// One dialect-legality violation: a construct that parsed (it's part of
/// the union grammar) but is not legal under the configured `dialect`.
///
/// `code` is a plain `LBnnnn` string — `luabox-syntax` does not depend on
/// `luabox-diag` (SPEC.md §16 acyclic dep graph); the CLI maps these onto
/// registered [`luabox_diag`]-style diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialectError {
    pub code: &'static str,
    pub message: String,
    pub range: rowan::TextRange,
}

/// Walk `parse`'s tree and collect every dialect-legality violation for
/// `dialect`, in source order.
#[must_use]
pub fn validate(parse: &Parse, dialect: Dialect) -> Vec<DialectError> {
    let mut errors = Vec::new();
    for element in parse.syntax().descendants_with_tokens() {
        match element {
            NodeOrToken::Node(node) => check_node(&node, dialect, &mut errors),
            NodeOrToken::Token(token) => check_token(&token, dialect, &mut errors),
        }
    }
    errors
}

/// Human name for a dialect in prose (`SPEC.md §2` naming).
fn edition_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Lua51 => "Lua 5.1",
        Dialect::Lua52 => "Lua 5.2",
        Dialect::Lua53 => "Lua 5.3",
        Dialect::Lua54 => "Lua 5.4",
        Dialect::LuaJit => "LuaJIT",
    }
}

/// `//`, `&`, `|`, `~` (binary/unary), `<<`, `>>`: 5.3+ only, never on
/// LuaJIT (SPEC.md §2.1 — lowered via `math.floor`/`bit32`/`bit` shims).
fn supports_53_ops(dialect: Dialect) -> bool {
    matches!(dialect, Dialect::Lua53 | Dialect::Lua54)
}

/// Hex float literals and `\z`/`\x` string escapes: 5.2+ and LuaJIT: only
/// Lua 5.1 lacks them.
fn supports_52_lexis(dialect: Dialect) -> bool {
    dialect != Dialect::Lua51
}

fn check_node(node: &super::SyntaxNode, dialect: Dialect, errors: &mut Vec<DialectError>) {
    match node.kind() {
        SyntaxKind::LABEL_STMT | SyntaxKind::GOTO_STMT if !dialect.has_goto() => {
            let construct = if node.kind() == SyntaxKind::LABEL_STMT {
                "a label (`::name::`)"
            } else {
                "a `goto` statement"
            };
            errors.push(DialectError {
                code: "LB0010",
                message: format!(
                    "{construct} is not available in {}; goto/labels are supported from Lua 5.2 onward (also LuaJIT)",
                    edition_name(dialect)
                ),
                range: node.text_range(),
            });
        }
        SyntaxKind::NAME_ATTRIB if dialect != Dialect::Lua54 => {
            let attrib_text = node.text().to_string();
            errors.push(DialectError {
                code: "LB0013",
                message: format!(
                    "the `{attrib_text}` attribute is not available in {}; `<const>`/`<close>` attributes are supported from Lua 5.4 onward",
                    edition_name(dialect)
                ),
                range: node.text_range(),
            });
        }
        _ => {}
    }
}

fn check_token(token: &super::SyntaxToken, dialect: Dialect, errors: &mut Vec<DialectError>) {
    match token.kind() {
        SyntaxKind::SLASH_SLASH if !supports_53_ops(dialect) => {
            errors.push(DialectError {
                code: "LB0011",
                message: format!(
                    "integer division `//` is not available in {}; supported from Lua 5.3 onward (not supported on LuaJIT)",
                    edition_name(dialect)
                ),
                range: token.text_range(),
            });
        }
        SyntaxKind::AMP
        | SyntaxKind::PIPE
        | SyntaxKind::TILDE
        | SyntaxKind::LT_LT
        | SyntaxKind::GT_GT
            if !supports_53_ops(dialect) =>
        {
            errors.push(DialectError {
                code: "LB0012",
                message: format!(
                    "the bitwise operator `{}` is not available in {}; bitwise operators are supported from Lua 5.3 onward (not supported on LuaJIT)",
                    token.text(),
                    edition_name(dialect)
                ),
                range: token.text_range(),
            });
        }
        SyntaxKind::NUMBER => check_number_literal(token, dialect, errors),
        SyntaxKind::STRING => check_string_escapes(token, dialect, errors),
        _ => {}
    }
}

/// Hex float literals (`0x1.8p3`, `0x1p4`, …) are 5.2+/LuaJIT; flag them
/// under 5.1. Plain hex integers (`0xBEBADA`) are legal everywhere and must
/// not fire.
fn check_number_literal(
    token: &super::SyntaxToken,
    dialect: Dialect,
    errors: &mut Vec<DialectError>,
) {
    if supports_52_lexis(dialect) {
        return;
    }
    let text = token.text();
    let lower = text.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("0x") else {
        return;
    };
    if rest.contains('.') || rest.contains('p') {
        errors.push(DialectError {
            code: "LB0014",
            message: format!(
                "the hex float literal `{text}` is not available in {}; hex float literals are supported from Lua 5.2 onward (also LuaJIT)",
                edition_name(dialect)
            ),
            range: token.text_range(),
        });
    }
}

/// Scan a `STRING` token's raw text for `\z`, `\x`, and `\u{...}` escape
/// sequences, mindful of doubled backslashes (`\\z` is an escaped
/// backslash followed by a literal `z`, not a `\z` escape). Long-bracket
/// strings (`[[...]]`) have no escapes and are skipped.
fn check_string_escapes(
    token: &super::SyntaxToken,
    dialect: Dialect,
    errors: &mut Vec<DialectError>,
) {
    let text = token.text();
    if !(text.starts_with('\'') || text.starts_with('"')) {
        return; // long-bracket string: no escapes to scan
    }
    let bytes = text.as_bytes();
    let start = token.text_range().start();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        let esc = bytes[i + 1];
        match esc {
            b'z' | b'Z' if !supports_52_lexis(dialect) => {
                push_escape_error(errors, "LB0015", "\\z", "Lua 5.2", i, 2, start);
            }
            b'x' if !supports_52_lexis(dialect) => {
                push_escape_error(errors, "LB0015", "\\x", "Lua 5.2", i, 2, start);
            }
            b'u' if bytes.get(i + 2) == Some(&b'{') && !supports_53_ops(dialect) => {
                // Skip to the closing `}` (or end of the token) so the
                // range covers the whole escape and we don't re-scan its
                // interior.
                let mut j = i + 3;
                while j < bytes.len() && bytes[j] != b'}' {
                    j += 1;
                }
                let end = (j + 1).min(bytes.len());
                push_escape_error(errors, "LB0016", "\\u{...}", "Lua 5.3", i, end - i, start);
                i = end;
                continue;
            }
            _ => {}
        }
        i += 2;
    }
}

#[expect(
    clippy::expect_used,
    reason = "offset/len index within a lexed string token; all offsets fit rowan's u32 TextSize, so >4 GiB input is out of scope"
)]
fn push_escape_error(
    errors: &mut Vec<DialectError>,
    code: &'static str,
    escape: &str,
    earliest: &str,
    offset: usize,
    len: usize,
    token_start: rowan::TextSize,
) {
    let start = token_start + rowan::TextSize::try_from(offset).expect("string token too long");
    let end = start + rowan::TextSize::try_from(len).expect("string token too long");
    errors.push(DialectError {
        code,
        message: format!(
            "the `{escape}` string escape is not available before {earliest} (supported from {earliest} onward, also LuaJIT for `\\z`/`\\x`)"
        ),
        range: rowan::TextRange::new(start, end),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::parse;

    fn errors_for(text: &str, dialect: Dialect) -> Vec<DialectError> {
        let parse = parse(text, dialect);
        validate(&parse, dialect)
    }

    fn codes_for(text: &str, dialect: Dialect) -> Vec<&'static str> {
        errors_for(text, dialect).iter().map(|e| e.code).collect()
    }

    // === LB0010: goto / labels ===

    #[test]
    fn goto_and_label_error_in_51_only() {
        for dialect in [
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::Lua54,
            Dialect::LuaJit,
        ] {
            assert_eq!(
                codes_for("::top:: goto top", dialect),
                Vec::<&str>::new(),
                "{dialect:?} should allow goto/labels"
            );
        }
    }

    #[test]
    fn label_errors_in_51() {
        // `::top::` still parses under 5.1 (union grammar); the validator
        // flags it. `goto top` under 5.1 lexes as two idents (an
        // assignment target and a call target) and is ordinary — legal —
        // 5.1 code, so it must not fire here.
        let errors = errors_for("::top::", Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "LB0010");
        assert_eq!(errors[0].range, rowan::TextRange::new(0.into(), 7.into()));
    }

    #[test]
    fn bare_goto_call_in_51_is_legal_51_code() {
        // In 5.1, `goto` is an ordinary identifier: `goto(top)` is a call,
        // not a goto statement, and must not trip LB0010.
        assert_eq!(codes_for("goto(top)", Dialect::Lua51), Vec::<&str>::new());
    }

    #[test]
    fn goto_and_label_clean_on_luajit() {
        // `goto`/`::label::` only exist as real GOTO_STMT/LABEL_STMT nodes
        // once the lexer treats `goto` as a keyword (5.2+, LuaJIT) — see
        // `goto_is_an_identifier_in_51` in parser.rs for the 5.1 case,
        // which this validator never sees as a GOTO_STMT at all.
        let errors = errors_for("::top:: goto top", Dialect::LuaJit);
        assert!(errors.is_empty());
    }

    // === LB0011: integer division `//` ===

    #[test]
    fn integer_division_errors_before_53_and_on_luajit() {
        for dialect in [Dialect::Lua51, Dialect::Lua52, Dialect::LuaJit] {
            assert_eq!(codes_for("x = a // b", dialect), vec!["LB0011"]);
        }
    }

    #[test]
    fn integer_division_clean_on_53_and_54() {
        for dialect in [Dialect::Lua53, Dialect::Lua54] {
            assert_eq!(codes_for("x = a // b", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn integer_division_range_is_the_operator() {
        let errors = errors_for("x = a // b", Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].range, rowan::TextRange::new(6.into(), 8.into()));
    }

    // === LB0012: bitwise operators ===

    #[test]
    fn bitops_error_before_53_and_on_luajit() {
        for dialect in [Dialect::Lua51, Dialect::Lua52, Dialect::LuaJit] {
            assert_eq!(codes_for("x = a & b", dialect), vec!["LB0012"]);
            assert_eq!(codes_for("x = a | b", dialect), vec!["LB0012"]);
            assert_eq!(codes_for("x = a << b", dialect), vec!["LB0012"]);
            assert_eq!(codes_for("x = a >> b", dialect), vec!["LB0012"]);
            assert_eq!(
                codes_for("x = a ~ b", dialect),
                vec!["LB0012"],
                "binary xor"
            );
            assert_eq!(codes_for("x = ~a", dialect), vec!["LB0012"], "unary bnot");
        }
    }

    #[test]
    fn bitops_clean_on_53_and_54() {
        for dialect in [Dialect::Lua53, Dialect::Lua54] {
            assert_eq!(
                codes_for("x = a & b | c ~ d << e >> f", dialect),
                Vec::<&str>::new()
            );
        }
    }

    #[test]
    fn tilde_eq_never_fires() {
        for dialect in Dialect::ALL {
            assert_eq!(codes_for("x = a ~= b", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn hash_length_operator_never_fires() {
        for dialect in Dialect::ALL {
            assert_eq!(codes_for("x = #t", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn bitops_range_is_the_operator_token() {
        let errors = errors_for("x = a & b", Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].range, rowan::TextRange::new(6.into(), 7.into()));
    }

    // === LB0013: `<const>` / `<close>` attribs ===

    #[test]
    fn attribs_error_before_54() {
        for dialect in [
            Dialect::Lua51,
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::LuaJit,
        ] {
            assert_eq!(codes_for("local x <const> = 1", dialect), vec!["LB0013"]);
            assert_eq!(codes_for("local x <close> = 1", dialect), vec!["LB0013"]);
        }
    }

    #[test]
    fn attribs_clean_only_on_54() {
        assert_eq!(
            codes_for("local x <const>, y <close> = 1, 2", Dialect::Lua54),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn attrib_range_is_the_attrib_not_the_whole_local() {
        let errors = errors_for("local x <const> = 1", Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].range, rowan::TextRange::new(8.into(), 15.into()));
    }

    // === LB0014: hex float literals ===

    #[test]
    fn hex_float_errors_in_51_only() {
        assert_eq!(codes_for("x = 0x1p4", Dialect::Lua51), vec!["LB0014"]);
        assert_eq!(codes_for("x = 0x1.8p3", Dialect::Lua51), vec!["LB0014"]);
        for dialect in [
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::Lua54,
            Dialect::LuaJit,
        ] {
            assert_eq!(codes_for("x = 0x1p4", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn plain_hex_integer_never_fires() {
        for dialect in Dialect::ALL {
            assert_eq!(codes_for("x = 0xBEBADA", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn plain_decimal_float_never_fires() {
        for dialect in Dialect::ALL {
            assert_eq!(codes_for("x = 3.1416e-2", dialect), Vec::<&str>::new());
        }
    }

    // === LB0015: `\z` / `\x` string escapes ===

    #[test]
    fn z_and_x_escapes_error_in_51_only() {
        assert_eq!(
            codes_for("x = \"a\\z\n  b\"", Dialect::Lua51),
            vec!["LB0015"]
        );
        assert_eq!(codes_for("x = \"a\\x41b\"", Dialect::Lua51), vec!["LB0015"]);
        for dialect in [
            Dialect::Lua52,
            Dialect::Lua53,
            Dialect::Lua54,
            Dialect::LuaJit,
        ] {
            assert_eq!(codes_for("x = \"a\\z\n  b\"", dialect), Vec::<&str>::new());
            assert_eq!(codes_for("x = \"a\\x41b\"", dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn doubled_backslash_before_z_does_not_fire() {
        // `"a\\z"` is an escaped backslash followed by a literal `z`, not a
        // `\z` escape.
        assert_eq!(
            codes_for(r#"x = "a\\z""#, Dialect::Lua51),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn ordinary_escapes_never_fire() {
        for dialect in Dialect::ALL {
            assert_eq!(
                codes_for(r#"x = "a\nb\tc\\d\"e""#, dialect),
                Vec::<&str>::new()
            );
        }
    }

    #[test]
    fn escape_range_covers_backslash_and_char() {
        let errors = errors_for(r#"x = "a\x41""#, Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        // "a\x41" starts at byte 4 (opening quote); \x is at offset 2 within it.
        assert_eq!(errors[0].range, rowan::TextRange::new(6.into(), 8.into()));
    }

    // === LB0016: `\u{...}` escapes ===

    #[test]
    fn unicode_escape_errors_before_53() {
        for dialect in [Dialect::Lua51, Dialect::Lua52, Dialect::LuaJit] {
            assert_eq!(codes_for(r#"x = "\u{48}""#, dialect), vec!["LB0016"]);
        }
    }

    #[test]
    fn unicode_escape_clean_on_53_and_54() {
        for dialect in [Dialect::Lua53, Dialect::Lua54] {
            assert_eq!(codes_for(r#"x = "\u{48}""#, dialect), Vec::<&str>::new());
        }
    }

    #[test]
    fn unicode_escape_does_not_double_report_with_z_or_x_rule() {
        // \u{...} must be reported once (LB0016), not also matched as a
        // stray \x-ish sequence.
        let errors = errors_for(r#"x = "\u{48}""#, Dialect::Lua51);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "LB0016");
    }

    // === Multiple violations collected in one pass ===

    #[test]
    fn multiple_violations_all_collected() {
        let src = "local x <const> = a // b & c\n::top:: goto top\nlocal y = 0x1p4";
        let errors = errors_for(src, Dialect::Lua51);
        let mut codes: Vec<_> = errors.iter().map(|e| e.code).collect();
        codes.sort_unstable();
        assert_eq!(
            codes,
            vec!["LB0010", "LB0011", "LB0012", "LB0013", "LB0014"]
        );
        // Reported in source order.
        for pair in errors.windows(2) {
            assert!(pair[0].range.start() <= pair[1].range.start());
        }
    }

    // === Valid-corpus-per-dialect sweeps: no false positives ===

    #[test]
    fn idiomatic_51_corpus_is_clean() {
        let src = "\
local function fib(n)\n\
  if n < 2 then return n end\n\
  return fib(n - 1) + fib(n - 2)\n\
end\n\
local t = { 1, 2, x = 'y' }\n\
for i = 1, #t do t[i] = t[i] * 2 end\n\
for k, v in pairs(t) do print(k, v) end\n\
local s = \"line1\\nline2\\ttab\"\n\
print(fib(10), s)\n";
        assert_eq!(codes_for(src, Dialect::Lua51), Vec::<&str>::new());
    }

    #[test]
    fn idiomatic_52_corpus_is_clean() {
        let src = "\
local _ENV = _ENV\n\
::top::\n\
local i = 0\n\
i = i + 1\n\
if i < 3 then goto top end\n\
local hex = 0x1p4\n\
local s = \"a\\z\n  b\\x41\"\n\
print(i, hex, s)\n";
        assert_eq!(codes_for(src, Dialect::Lua52), Vec::<&str>::new());
    }

    #[test]
    fn idiomatic_53_corpus_is_clean() {
        let src = "\
local a, b = 7, 2\n\
local q = a // b\n\
local bits = a & b | 1 ~ 2\n\
bits = bits << 1 >> 1\n\
local s = \"\\u{48}\\u{49}\"\n\
print(q, bits, s)\n";
        assert_eq!(codes_for(src, Dialect::Lua53), Vec::<&str>::new());
    }

    #[test]
    fn idiomatic_54_corpus_is_clean() {
        let src = "\
local x <const> = 10\n\
do\n\
  local h <close> = setmetatable({}, { __close = function() end })\n\
  local q = x // 3\n\
  local bits = x & 3 | 1 ~ 2\n\
  local s = \"\\u{2603}\"\n\
  print(q, bits, s)\n\
end\n";
        assert_eq!(codes_for(src, Dialect::Lua54), Vec::<&str>::new());
    }

    #[test]
    fn idiomatic_luajit_corpus_is_clean() {
        let src = "\
::top::\n\
local i = 0\n\
i = i + 1\n\
if i < 3 then goto top end\n\
local hex = 0x1p4\n\
local n = 42LL\n\
local s = \"a\\z\n  b\\x41\"\n\
print(i, hex, n, s)\n";
        assert_eq!(codes_for(src, Dialect::LuaJit), Vec::<&str>::new());
    }
}
