//! Canonical Lua formatter (SPEC.md §10): StyLua-compatible default style,
//! at most the six documented options, idempotent, and — above all — unable
//! to change what a program means.
//!
//! Safety ladder, enforced mechanically on every call:
//! 1. Inputs that do not parse cleanly are returned **unchanged** — broken
//!    code is never reformatted.
//! 2. The output is re-lexed and its non-trivia token stream must match the
//!    input's (kind + text), modulo exactly two neutral rewrites: short
//!    strings may change quotes only if their decoded value is identical
//!    ([`strings`]), and a trailing `,`/`;` directly before a table's `}`
//!    may appear or disappear.
//! 3. The output must itself parse cleanly, and every comment must survive.
//!
//! If any check fails the input comes back unchanged: a formatter never
//! destroys code.

mod emit;
mod strings;

use super::{Dialect, SyntaxKind, lex, parse};

/// Formatting options — the layout knobs from SPEC.md §10, nothing more.
/// `Options::default()` is the canonical luabox style.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Options {
    /// Target line width for layout decisions (table collapsing). Default 120.
    pub width: usize,
    /// Indentation unit. Default four spaces.
    pub indent: Indent,
    /// String-quote preference. Default: double quotes where the value allows.
    pub quotes: Quotes,
    /// Add a trailing comma to the last field of multi-line tables. Default true.
    pub trailing_table_comma: bool,
    /// Line ending for emitted line breaks. Default LF.
    pub line_ending: LineEnding,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            width: 120,
            indent: Indent::Spaces(4),
            quotes: Quotes::AutoPreferDouble,
            trailing_table_comma: true,
            line_ending: LineEnding::Lf,
        }
    }
}

/// Indentation unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Spaces(u8),
    /// Tabs (counted as four columns for width decisions).
    Tabs,
}

/// String-quote preference. "Auto": a literal only converts when the swap
/// is provably value-preserving; long strings never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quotes {
    AutoPreferDouble,
    AutoPreferSingle,
}

/// Line-ending style for emitted line breaks. Raw line breaks *inside*
/// long strings and long comments are never rewritten (they are content).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// Format `text` (parsed under `dialect`) into the canonical style.
///
/// Returns the input unchanged when it does not parse cleanly or when any
/// mechanical safety check on the output fails (see module docs).
#[must_use]
pub fn format(text: &str, dialect: Dialect) -> String {
    format_with(text, dialect, &Options::default())
}

/// [`format`] with explicit [`Options`].
#[must_use]
pub fn format_with(text: &str, dialect: Dialect, options: &Options) -> String {
    let parsed = parse(text, dialect);
    if !parsed.errors().is_empty() {
        return text.to_string();
    }
    let out = emit::emit(&parsed.syntax(), options);
    if out == text {
        return out;
    }
    let safe = comments_preserved(&parsed.syntax(), &out)
        && parse(&out, dialect).errors().is_empty()
        && same_program(text, &out, dialect);
    if safe { out } else { text.to_string() }
}

/// Conservative safety net: every comment in the input tree must appear in
/// the output verbatim (modulo trailing-whitespace trim).
fn comments_preserved(root: &super::SyntaxNode, out: &str) -> bool {
    root.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::COMMENT)
        .all(|t| out.contains(t.text().trim_end()))
}

/// The non-trivia `(kind, text)` stream of `text`.
#[expect(
    clippy::string_slice,
    reason = "offset/end accumulate lexer token lengths, which tile the input exactly on char boundaries"
)]
fn significant_tokens(text: &str, dialect: Dialect) -> Vec<(SyntaxKind, &str)> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for token in lex(text, dialect) {
        let end = offset + token.len as usize;
        if !token.kind.is_trivia() {
            out.push((token.kind, &text[offset..end]));
        }
        offset = end;
    }
    out
}

/// Mechanical semantics check: token streams must be identical except for
/// value-preserving string requoting and trailing table separators.
fn same_program(input: &str, output: &str, dialect: Dialect) -> bool {
    let ins = significant_tokens(input, dialect);
    let outs = significant_tokens(output, dialect);
    let (mut at_in, mut at_out) = (0usize, 0usize);
    loop {
        match (ins.get(at_in), outs.get(at_out)) {
            (None, None) => return true,
            (Some(&(ka, ta)), Some(&(kb, tb))) if ka == kb && token_text_equal(ka, ta, tb) => {
                at_in += 1;
                at_out += 1;
            }
            (in_tok, out_tok) => {
                // A `,`/`;` directly before `}` is always a table's trailing
                // separator — semantically inert, so one side may lack it.
                if skippable_trailing_separator(&ins, at_in, out_tok) {
                    at_in += 1;
                } else if skippable_trailing_separator(&outs, at_out, in_tok) {
                    at_out += 1;
                } else {
                    return false;
                }
            }
        }
    }
}

fn skippable_trailing_separator(
    tokens: &[(SyntaxKind, &str)],
    idx: usize,
    other: Option<&(SyntaxKind, &str)>,
) -> bool {
    matches!(
        tokens.get(idx),
        Some((SyntaxKind::COMMA | SyntaxKind::SEMICOLON, _))
    ) && matches!(tokens.get(idx + 1), Some((SyntaxKind::R_BRACE, _)))
        && matches!(other, Some((SyntaxKind::R_BRACE, _)))
}

fn token_text_equal(kind: SyntaxKind, a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    kind == SyntaxKind::STRING
        && matches!(
            (
                strings::decode_short_string(a),
                strings::decode_short_string(b)
            ),
            (Some(va), Some(vb)) if va == vb
        )
}

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;

    fn fmt(text: &str) -> String {
        format(text, Dialect::Lua54)
    }

    #[track_caller]
    fn check(input: &str, expected: &str) {
        let out = fmt(input);
        assert_eq!(out, expected, "for input {input:?}");
        assert_eq!(fmt(&out), out, "not idempotent for {input:?}");
    }

    // === Statements & spacing ===

    #[test]
    fn local_and_assignment_spacing() {
        check("local x=1", "local x = 1\n");
        check("local  a,b  =  1,'two'", "local a, b = 1, \"two\"\n");
        check("a,b.c,d[1]=1,2,3", "a, b.c, d[1] = 1, 2, 3\n");
        check(
            "local  x<const>,y<close>  =  1,f()",
            "local x <const>, y <close> = 1, f()\n",
        );
        // `<close>=` lexes as `close` `>=` (greedy, as in real Lua): a parse
        // error, so the input must come back unchanged.
        let glued = "local y<close>=1";
        assert_eq!(fmt(glued), glued);
    }

    #[test]
    fn one_statement_per_line() {
        check(
            "local a = 1 local b = 2 f(a,b)",
            "local a = 1\nlocal b = 2\nf(a, b)\n",
        );
    }

    #[test]
    fn semicolons_stay_glued_to_their_statement() {
        check("f() ; g()  ;", "f();\ng();\n");
        check("return 1 ;", "return 1;\n");
    }

    #[test]
    fn binary_and_unary_operator_spacing() {
        check("x=a+b*c-d/e%f^g", "x = a + b * c - d / e % f ^ g\n");
        check("x=a..b..'s'", "x = a .. b .. \"s\"\n");
        check("x=-1+ -y", "x = -1 + -y\n");
        check("x=#t+1", "x = #t + 1\n");
        check("x=not not a", "x = not not a\n");
        check("x=-2^2", "x = -2 ^ 2\n");
        check(
            "x=a==b and c~=d or e<=f",
            "x = a == b and c ~= d or e <= f\n",
        );
        check("x=a|b~c&d<<e>>f//g", "x = a | b ~ c & d << e >> f // g\n");
        check("x=~a", "x = ~a\n");
    }

    #[test]
    fn unary_minus_pairs_never_merge_into_comments() {
        check("x = - -y", "x = - -y\n");
        check("x=-(-y)", "x = -(-y)\n");
    }

    #[test]
    fn calls_and_suffix_chains() {
        check("f ( 1 , 2 )", "f(1, 2)\n");
        check("t . a : b ( )", "t.a:b()\n");
        check("a.b:c(1)(2)[3].d=1", "a.b:c(1)(2)[3].d = 1\n");
        check("( f ) ( )", "(f)()\n");
        check("x=f(g(h(1)))", "x = f(g(h(1)))\n");
    }

    #[test]
    fn paren_free_call_arguments_keep_their_shape() {
        check("f'x'", "f \"x\"\n");
        check("f[[x]]", "f [[x]]\n");
        check("f{1,2}", "f { 1, 2 }\n");
        check("obj:method'lit'", "obj:method \"lit\"\n");
        check("require'lib.mod'", "require \"lib.mod\"\n");
    }

    #[test]
    fn index_with_long_string_does_not_merge_brackets() {
        check("x=t[ [[k]] ]", "x = t[ [[k]]]\n");
    }

    // === Block constructs ===

    #[test]
    fn if_elseif_else_layout() {
        check(
            "if a then x=1 elseif b then x=2 else x=3 end",
            "if a then\n    x = 1\nelseif b then\n    x = 2\nelse\n    x = 3\nend\n",
        );
    }

    #[test]
    fn nested_blocks_indent() {
        check(
            "if a then if b then f() end end",
            "if a then\n    if b then\n        f()\n    end\nend\n",
        );
    }

    #[test]
    fn loops_layout() {
        check(
            "while x<10 do x=x+1 end",
            "while x < 10 do\n    x = x + 1\nend\n",
        );
        check("repeat f() until done", "repeat\n    f()\nuntil done\n");
        check(
            "for i=1,10,2 do print(i) end",
            "for i = 1, 10, 2 do\n    print(i)\nend\n",
        );
        check(
            "for k,v in pairs(t) do print(k,v) end",
            "for k, v in pairs(t) do\n    print(k, v)\nend\n",
        );
        check("do break end", "do\n    break\nend\n");
    }

    #[test]
    fn empty_non_function_blocks_still_expand() {
        check("while true do end", "while true do\nend\n");
        check("do end", "do\nend\n");
        check("if x then end", "if x then\nend\n");
    }

    #[test]
    fn function_layouts() {
        check(
            "function a.b:c(x,y,...) return x end",
            "function a.b:c(x, y, ...)\n    return x\nend\n",
        );
        check(
            "local function f(a) return a end",
            "local function f(a)\n    return a\nend\n",
        );
        check(
            "local f=function(x) return x end",
            "local f = function(x)\n    return x\nend\n",
        );
    }

    #[test]
    fn empty_function_bodies_collapse() {
        check("function noop() end", "function noop() end\n");
        check("local f = function()  end", "local f = function() end\n");
        check("local f = function()\nend", "local f = function() end\n");
    }

    #[test]
    fn function_with_comment_in_body_does_not_collapse() {
        check("function f() -- todo\nend", "function f() -- todo\nend\n");
    }

    #[test]
    fn goto_and_labels() {
        check("::top:: goto top", "::top::\ngoto top\n");
    }

    // === Strings ===

    #[test]
    fn quote_normalization() {
        check("x='hi'", "x = \"hi\"\n");
        check(r"x='it\'s'", "x = \"it's\"\n");
        check(r#"x='say "hi"'"#, "x = 'say \"hi\"'\n");
        check(r"x='a\nb'", "x = \"a\\nb\"\n");
        check("x=[[keep ' and \" alone]]", "x = [[keep ' and \" alone]]\n");
        check("x=[==[level]==]", "x = [==[level]==]\n");
    }

    #[test]
    fn prefer_single_quotes_option() {
        let opts = Options {
            quotes: Quotes::AutoPreferSingle,
            ..Options::default()
        };
        assert_eq!(format_with("x=\"hi\"", Dialect::Lua54, &opts), "x = 'hi'\n");
    }

    #[test]
    fn multiline_long_string_content_untouched() {
        check("x=[[a\n  b\n]]", "x = [[a\n  b\n]]\n");
    }

    // === Tables ===

    #[test]
    fn short_tables_inline() {
        check("x={1,2,3}", "x = { 1, 2, 3 }\n");
        check("x={a=1,b=2}", "x = { a = 1, b = 2 }\n");
        check("x={['k']=v,f(),'s'}", "x = { [\"k\"] = v, f(), \"s\" }\n");
        check("x={}", "x = {}\n");
        check("x={ }", "x = {}\n");
        check("x={{1},{2}}", "x = { { 1 }, { 2 } }\n");
    }

    #[test]
    fn inline_tables_drop_trailing_separators() {
        check("x={1,2,}", "x = { 1, 2 }\n");
        check("x={1;2;}", "x = { 1; 2 }\n");
    }

    #[test]
    fn wide_tables_expand_one_field_per_line_with_trailing_comma() {
        let input = format!("x={{{}}}", ("long_field_name = 1, ").repeat(8));
        let out = fmt(&input);
        assert_eq!(
            out,
            "x = {\n".to_string() + &"    long_field_name = 1,\n".repeat(8) + "}\n"
        );
    }

    #[test]
    fn width_option_controls_expansion() {
        let opts = Options {
            width: 12,
            ..Options::default()
        };
        assert_eq!(
            format_with("x={1,2,3}", Dialect::Lua54, &opts),
            "x = {\n    1,\n    2,\n    3,\n}\n"
        );
        let no_trailing = Options {
            width: 12,
            trailing_table_comma: false,
            ..Options::default()
        };
        assert_eq!(
            format_with("x={1,2,3}", Dialect::Lua54, &no_trailing),
            "x = {\n    1,\n    2,\n    3\n}\n"
        );
    }

    #[test]
    fn table_with_comment_expands() {
        check("x={1, -- one\n2}", "x = {\n    1, -- one\n    2,\n}\n");
    }

    #[test]
    fn table_with_function_value_expands() {
        check(
            "t={f=function(x) return x end}",
            "t = {\n    f = function(x)\n        return x\n    end,\n}\n",
        );
    }

    #[test]
    fn nested_table_in_expanded_table_can_stay_inline() {
        let opts = Options {
            width: 30,
            ..Options::default()
        };
        assert_eq!(
            format_with(
                "x={aaaaaaaaaa=1,bbbbbbbbbb={1,2},cccccccccc=3}",
                Dialect::Lua54,
                &opts
            ),
            "x = {\n    aaaaaaaaaa = 1,\n    bbbbbbbbbb = { 1, 2 },\n    cccccccccc = 3,\n}\n"
        );
    }

    // === Comments ===

    #[test]
    fn trailing_comments_stay_trailing() {
        check("local x = 1 -- the answer", "local x = 1 -- the answer\n");
        check("f() -- do it\ng()", "f() -- do it\ng()\n");
    }

    #[test]
    fn own_line_comments_stay_on_their_line() {
        check("-- header\nlocal x = 1", "-- header\nlocal x = 1\n");
        check(
            "local a = 1\n-- middle\nlocal b = 2",
            "local a = 1\n-- middle\nlocal b = 2\n",
        );
    }

    #[test]
    fn comment_before_end_keeps_body_indent() {
        check(
            "do\nf() -- t\n-- before end\nend",
            "do\n    f() -- t\n    -- before end\nend\n",
        );
    }

    #[test]
    fn comment_after_then_stays_trailing() {
        check(
            "if x then -- why\nf()\nend",
            "if x then -- why\n    f()\nend\n",
        );
    }

    #[test]
    fn long_comments_survive_inline_and_multiline() {
        check("local x = --[[mid]] 1", "local x = --[[mid]] 1\n");
        check("--[[a\nb]]\nf()", "--[[a\nb]]\nf()\n");
    }

    #[test]
    fn comment_only_file() {
        check("-- just a note", "-- just a note\n");
        check("   -- indented note   ", "-- indented note\n");
    }

    #[test]
    fn file_trailing_comment_survives() {
        check("f()\n-- bye", "f()\n-- bye\n");
    }

    // === Blank lines ===

    #[test]
    fn blank_line_runs_collapse_to_one() {
        check(
            "local a = 1\n\n\n\nlocal b = 2",
            "local a = 1\n\nlocal b = 2\n",
        );
        check("local a = 1\n\nlocal b = 2", "local a = 1\n\nlocal b = 2\n");
    }

    #[test]
    fn no_blank_lines_at_block_edges() {
        check("if x then\n\n\nf()\nend", "if x then\n    f()\nend\n");
    }

    // === Whitespace & endings ===

    #[test]
    fn output_ends_with_exactly_one_newline() {
        check("f()", "f()\n");
        check("f()\n\n\n", "f()\n");
    }

    #[test]
    fn crlf_input_normalizes_to_lf() {
        check("local x=1\r\nf(x)\r\n", "local x = 1\nf(x)\n");
    }

    #[test]
    fn crlf_option_emits_crlf_but_not_inside_long_strings() {
        let opts = Options {
            line_ending: LineEnding::Crlf,
            ..Options::default()
        };
        assert_eq!(
            format_with("local x=1\nx=[[a\nb]]", Dialect::Lua54, &opts),
            "local x = 1\r\nx = [[a\nb]]\r\n"
        );
    }

    #[test]
    fn tab_indent_option() {
        let opts = Options {
            indent: Indent::Tabs,
            ..Options::default()
        };
        assert_eq!(
            format_with("if x then f() end", Dialect::Lua54, &opts),
            "if x then\n\tf()\nend\n"
        );
    }

    #[test]
    fn empty_and_whitespace_only_inputs() {
        assert_eq!(fmt(""), "");
        assert_eq!(fmt("   \n\n  "), "");
    }

    // === Safety ===

    #[test]
    fn broken_inputs_come_back_unchanged() {
        for src in [
            "local = 5",
            "if x then",
            "f(",
            "x = ",
            "function f( end",
            "x = 'unterminated",
            "#!/usr/bin/env lua\nprint(1)",
        ] {
            assert_eq!(fmt(src), src, "broken input must be untouched");
        }
    }

    #[test]
    fn dialect_matters_for_goto() {
        // `goto top` is two identifiers (a parse error) in 5.1: untouched.
        let src = "::top::  goto top";
        assert_eq!(format(src, Dialect::Lua51), src);
        assert_eq!(format(src, Dialect::Lua54), "::top::\ngoto top\n");
    }

    #[test]
    fn same_program_accepts_neutral_rewrites_only() {
        let d = Dialect::Lua54;
        assert!(same_program("x={1,}", "x = { 1 }", d));
        assert!(same_program("x={1}", "x = {\n    1,\n}", d));
        assert!(same_program("x='a'", "x = \"a\"", d));
        assert!(!same_program("x=1", "x = 2", d));
        assert!(!same_program("f(a)", "f(a, b)", d));
        assert!(!same_program("x='a'", "x = \"b\"", d));
        assert!(!same_program("f(a,b)", "f(a b)", d));
    }

    #[test]
    fn parser_corpus_formats_cleanly_and_idempotently() {
        // The parser test corpus programs, reused as a formatting smoke set.
        let corpus = [
            "local function fib(n)\n  if n < 2 then return n end\n  return fib(n - 1) + fib(n - 2)\nend\nprint(fib(10))\n",
            "local t <const> = { 1, 2, x = 'y', ['z'] = [[w]], f(); }\nfor i = 1, #t, 2 do t[i] = t[i] * 2 ^ i end\nfor k, v in pairs(t) do io.write(k, '=', tostring(v), '\\n') end\n",
            "::top::\nlocal i = 0\nwhile true do\n  i = i + 1\n  if i & 3 == 0 then goto top end\n  repeat i = i // 2 until i < 1 or i ~ 5 == 0\n  break\nend\n",
            "function obj.ns:method(a, b, ...)\n  local args = { ... }\n  return self, select('#', ...)\nend\nobj = setmetatable({}, { __index = function(_, k) return k end })\nobj:method 'lit' -- string call\nobj:method { 1, 2 }\ndo local x <close> = open() end\nreturn obj\n",
        ];
        for src in corpus {
            let once = fmt(src);
            assert_ne!(once, *src, "corpus programs are messy on purpose");
            assert!(
                parse(&once, Dialect::Lua54).errors().is_empty(),
                "output must re-parse: {once}"
            );
            assert_eq!(fmt(&once), once, "not idempotent for corpus program");
            assert!(same_program(src, &once, Dialect::Lua54));
        }
    }
}

#[cfg(test)]
// test code — panics document assumptions
#[allow(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn ident() -> impl Strategy<Value = String> {
        prop::sample::select(vec!["a", "b", "foo", "bar_2", "x", "obj", "t"]).prop_map(String::from)
    }

    fn literal() -> impl Strategy<Value = String> {
        prop::sample::select(vec![
            "1",
            "2.5",
            "0x1F",
            "1e3",
            "nil",
            "true",
            "false",
            "'hi'",
            "\"ok\"",
            r"'it\'s'",
            r#"'say "hi"'"#,
            r"'tab\tend'",
            r"'\65\x42\u{48}'",
            "[[long \n string]]",
            "[==[lvl]==]",
        ])
        .prop_map(String::from)
    }

    /// Always-valid expression text. Operands are parenthesized where
    /// needed so generated programs parse cleanly by construction.
    fn expr() -> impl Strategy<Value = String> {
        let leaf = prop_oneof![ident(), literal()];
        leaf.prop_recursive(4, 24, 4, |inner| {
            prop_oneof![
                (inner.clone(), inner.clone())
                    .prop_map(|(a, b)| format!("({a} + {b} * {a} .. {b})")),
                inner.clone().prop_map(|e| format!("not ({e})")),
                inner.clone().prop_map(|e| format!("-({e})")),
                inner.clone().prop_map(|e| format!("#({e})")),
                (ident(), prop::collection::vec(inner.clone(), 0..3))
                    .prop_map(|(f, args)| format!("{f}({})", args.join(","))),
                (ident(), ident(), inner.clone()).prop_map(|(o, m, a)| format!("{o}:{m}({a})")),
                ident().prop_map(|f| format!("{f} 'lit'")),
                // Spaces inside the brackets: `k` may be a long string, and
                // `[[[` would lex as a long-bracket opener.
                (inner.clone(), inner.clone()).prop_map(|(t, k)| format!("({t})[ {k} ]")),
                (inner.clone(), ident()).prop_map(|(t, f)| format!("({t}).{f}")),
                table(inner.clone()),
                (ident(), inner).prop_map(|(p, e)| format!("function({p}) return {e} end")),
            ]
        })
    }

    fn table(inner: impl Strategy<Value = String> + Clone) -> impl Strategy<Value = String> {
        prop::collection::vec(
            prop_oneof![
                inner.clone(),
                (ident(), inner.clone()).prop_map(|(k, v)| format!("{k} = {v}")),
                (inner.clone(), inner).prop_map(|(k, v)| format!("[ {k} ] = {v}")),
            ],
            0..4,
        )
        .prop_map(|fields| format!("{{{}}}", fields.join(", ")))
    }

    fn stmt() -> impl Strategy<Value = String> {
        prop_oneof![
            (ident(), expr()).prop_map(|(n, v)| format!("local {n} = {v}")),
            (ident(), expr()).prop_map(|(n, v)| format!("local {n} <const> = {v}")),
            (ident(), expr()).prop_map(|(n, v)| format!("{n} = {v}")),
            (ident(), expr()).prop_map(|(f, a)| format!("{f}({a})")),
            (expr(), expr(), expr()).prop_map(|(c, a, b)| format!(
                "if {c} then f({a}) elseif {b} then g() else h() end"
            )),
            (expr(), expr()).prop_map(|(c, v)| format!("while {c} do x = {v} end")),
            (expr(), expr()).prop_map(|(a, b)| format!("for i = {a}, {b} do print(i) end")),
            (ident(), expr()).prop_map(|(k, t)| format!("for {k}, v in pairs({t}) do end")),
            (expr(), expr()).prop_map(|(v, c)| format!("repeat x = {v} until {c}")),
            (ident(), ident(), expr())
                .prop_map(|(f, p, v)| format!("local function {f}({p}) return {v} end")),
            (ident(), expr()).prop_map(|(f, v)| format!("function ns.{f}(a, ...) return {v} end")),
            Just("do break end".to_string()),
            Just("::top:: goto top".to_string()),
            // The trailing newline keeps a following `--[[`/statement out of
            // the line comment's tail when the joiner is a bare space.
            Just("-- a note\n".to_string()),
            Just("--[[ block\ncomment ]]".to_string()),
        ]
    }

    fn program() -> impl Strategy<Value = String> {
        (
            prop::collection::vec(stmt(), 1..8),
            prop::collection::vec(
                prop::sample::select(vec!["\n", "\n\n", " ", "\n\n\n\n"]),
                1..8,
            ),
        )
            .prop_map(|(stmts, seps)| {
                let mut out = String::new();
                for (i, s) in stmts.iter().enumerate() {
                    if i > 0 {
                        out.push_str(seps[(i - 1) % seps.len()]);
                    }
                    out.push_str(s);
                }
                out
            })
    }

    /// Non-trivia token kinds with trailing table separators dropped and
    /// strings decoded — the invariant surface `format` must preserve.
    fn semantic_fingerprint(text: &str, dialect: Dialect) -> Vec<(SyntaxKind, Vec<u8>)> {
        let tokens = significant_tokens(text, dialect);
        let mut out = Vec::with_capacity(tokens.len());
        for (i, &(kind, text)) in tokens.iter().enumerate() {
            if matches!(kind, SyntaxKind::COMMA | SyntaxKind::SEMICOLON)
                && matches!(tokens.get(i + 1), Some((SyntaxKind::R_BRACE, _)))
            {
                continue;
            }
            let value = if kind == SyntaxKind::STRING {
                strings::decode_short_string(text).unwrap_or_else(|| text.as_bytes().to_vec())
            } else {
                text.as_bytes().to_vec()
            };
            out.push((kind, value));
        }
        out
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Generated programs are valid; formatting them is idempotent,
        /// re-parses cleanly, and preserves the semantic token stream —
        /// verified here independently of `format`'s internal check.
        #[test]
        fn fmt_is_idempotent_and_meaning_preserving(src in program()) {
            let dialect = Dialect::Lua54;
            prop_assert!(
                parse(&src, dialect).errors().is_empty(),
                "generator produced invalid source: {src:?}"
            );
            let once = format(&src, dialect);
            prop_assert!(
                parse(&once, dialect).errors().is_empty(),
                "output must re-parse: {once:?}"
            );
            prop_assert_eq!(
                semantic_fingerprint(&src, dialect),
                semantic_fingerprint(&once, dialect),
                "token stream changed for {:?} -> {:?}", src, once
            );
            let twice = format(&once, dialect);
            prop_assert_eq!(&twice, &once, "not idempotent for {:?}", src);
        }

        /// Arbitrary junk never panics; anything that fails to parse comes
        /// back unchanged, and format(format(x)) == format(x) regardless.
        #[test]
        fn arbitrary_input_never_panics(text in any::<String>()) {
            for dialect in Dialect::ALL {
                let once = format(&text, dialect);
                if !parse(&text, dialect).errors().is_empty() {
                    prop_assert_eq!(&once, &text);
                }
                prop_assert_eq!(format(&once, dialect), once);
            }
        }

        /// Mutated corpus programs (parse errors likely): never panic,
        /// never touch unparseable input, stay idempotent.
        #[test]
        fn corpus_mutations_are_safe(
            index in 0..4usize,
            at in any::<prop::sample::Index>(),
            action in 0u8..3,
            snippet in prop::sample::select(vec![
                "end", "then", "(", ")", "==", "local", "..", "[[", "]]", "'", "\"",
                "...", "::", "<const>", "--", "\n", ";", ",", "{", "}", "=", "function",
            ]),
        ) {
            let corpus: [&str; 4] = [
                "local function fib(n)\n  if n < 2 then return n end\n  return fib(n - 1) + fib(n - 2)\nend\n",
                "local t <const> = { 1, 2, x = 'y', ['z'] = [[w]], f(); }\nfor i = 1, #t, 2 do t[i] = t[i] * 2 ^ i end\n",
                "::top::\nwhile true do\n  if i & 3 == 0 then goto top end\n  break\nend\n",
                "obj = setmetatable({}, { __index = function(_, k) return k end })\nobj:method 'lit' -- string call\n",
            ];
            let source = corpus[index];
            let mut pos = at.index(source.len() + 1).min(source.len());
            while pos > 0 && !source.is_char_boundary(pos) {
                pos -= 1;
            }
            let mutated = match action {
                0 => format!("{}{}{}", &source[..pos], snippet, &source[pos..]),
                1 => {
                    let mut end = (pos + 6).min(source.len());
                    while end < source.len() && !source.is_char_boundary(end) {
                        end += 1;
                    }
                    format!("{}{}", &source[..pos], &source[end..])
                }
                _ => source[..pos].to_string(),
            };
            let once = format(&mutated, Dialect::Lua54);
            if !parse(&mutated, Dialect::Lua54).errors().is_empty() {
                prop_assert_eq!(&once, &mutated);
            }
            prop_assert_eq!(format(&once, Dialect::Lua54), once);
        }
    }
}
