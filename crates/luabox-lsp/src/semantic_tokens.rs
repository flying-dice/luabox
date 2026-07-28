//! Semantic tokens (`textDocument/semanticTokens/full`) for `.lua` files.
//!
//! The legend uses **only standard LSP token types and modifiers** so every
//! client's theme colours the result without custom scope mappings.
//!
//! # Classification rules
//!
//! **Lua** (token walk over the lossless tree, plus HIR name resolution):
//! - keywords, strings, numbers, operators from their lexer kinds;
//! - comments; a `---` doc comment (LuaCATS annotation block) additionally
//!   carries the `documentation` modifier so annotations read differently
//!   from prose comments;
//! - name *uses* via the HIR `lower()` resolution: locals/upvalues are
//!   `variable`, parameters `parameter`, `local function`s `function`, and
//!   globals are `variable` + `static` (+ `defaultLibrary` for the Lua
//!   standard globals) — the type-aware local/global distinction;
//! - declaration sites: `local` names (`readonly` when `<const>`/`<close>`),
//!   parameters, `for` variables, function/method names in `function a.b:c`,
//!   table constructor field names, and `::label::`s (labels have no
//!   standard token type, so they render as `variable`).
//!
//! Tokens are emitted in the LSP delta encoding with UTF-16 columns (via
//! [`LineIndex`]); multi-line tokens (long strings/comments) are split into
//! one token per line because clients need not support multiline tokens.

use std::collections::HashMap;

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxKind;
use rowan::{NodeOrToken, TextRange};

use crate::line_index::LineIndex;
use crate::sema::FileSema;

// === Legend ===============================================================
// The indices below are the positions in `legend()`; keep them in sync.

// Indices 0 (`namespace`) and 3 (`interface`) stay in the legend for
// client compatibility, but are no longer produced.
const PARAMETER: u32 = 5;
const VARIABLE: u32 = 6;
const PROPERTY: u32 = 7;
const FUNCTION: u32 = 8;
const METHOD: u32 = 9;
const KEYWORD: u32 = 10;
const COMMENT: u32 = 11;
const STRING: u32 = 12;
const NUMBER: u32 = 13;
const OPERATOR: u32 = 14;

const M_DECLARATION: u32 = 1;
const M_READONLY: u32 = 1 << 1;
const M_STATIC: u32 = 1 << 2;
const M_DEFAULT_LIBRARY: u32 = 1 << 3;
const M_DOCUMENTATION: u32 = 1 << 4;

/// The legend advertised at initialize: standard types/modifiers only.
#[must_use]
pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::TYPE,
            SemanticTokenType::CLASS,
            SemanticTokenType::INTERFACE,
            SemanticTokenType::TYPE_PARAMETER,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::METHOD,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::COMMENT,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::OPERATOR,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::STATIC,
            SemanticTokenModifier::DEFAULT_LIBRARY,
            SemanticTokenModifier::DOCUMENTATION,
        ],
    }
}

/// One classified token before delta encoding (byte range + legend indices).
struct RawToken {
    range: TextRange,
    token_type: u32,
    modifiers: u32,
}

// === Lua ==================================================================

/// Globals provided by the Lua standard library (any dialect), marked
/// `defaultLibrary` on top of the global `static` modifier.
const LUA_BUILTINS: &[&str] = &[
    "_G",
    "_VERSION",
    "arg",
    "assert",
    "collectgarbage",
    "coroutine",
    "debug",
    "dofile",
    "error",
    "getmetatable",
    "io",
    "ipairs",
    "load",
    "loadstring",
    "math",
    "next",
    "os",
    "package",
    "pairs",
    "pcall",
    "print",
    "rawequal",
    "rawget",
    "rawlen",
    "rawset",
    "require",
    "select",
    "setmetatable",
    "string",
    "table",
    "tonumber",
    "tostring",
    "type",
    "unpack",
    "utf8",
    "xpcall",
];

/// Semantic tokens for one Lua file.
#[must_use]
pub fn semantic_tokens(sema: &FileSema) -> Vec<SemanticToken> {
    let names: HashMap<TextRange, (u32, u32)> = sema
        .name_resolutions()
        .into_iter()
        .map(|(range, res)| (range, classify_resolution(sema, &res)))
        .collect();

    let mut raw = Vec::new();
    for element in sema.root.descendants_with_tokens() {
        let NodeOrToken::Token(token) = element else {
            continue;
        };
        let classified = match token.kind() {
            SyntaxKind::COMMENT => Some(classify_lua_comment(token.text())),
            SyntaxKind::STRING => Some((STRING, 0)),
            SyntaxKind::NUMBER => Some((NUMBER, 0)),
            SyntaxKind::IDENT => Some(classify_lua_ident(&token, &names)),
            kind if is_lua_keyword(kind) => Some((KEYWORD, 0)),
            kind if is_lua_operator(kind) => Some((OPERATOR, 0)),
            _ => None,
        };
        if let Some((token_type, modifiers)) = classified {
            raw.push(RawToken {
                range: token.text_range(),
                token_type,
                modifiers,
            });
        }
    }
    encode(&sema.index, raw)
}

/// A `---` comment opens (or continues) a LuaCATS doc block; mark it as
/// documentation so annotations read differently from prose comments.
fn classify_lua_comment(text: &str) -> (u32, u32) {
    if text.starts_with("---") {
        (COMMENT, M_DOCUMENTATION)
    } else {
        (COMMENT, 0)
    }
}

/// Classify a resolved name use: the HIR payoff — locals, upvalues,
/// parameters and `local function`s each get their own type, and globals are
/// `static` (plus `defaultLibrary` for the stdlib).
fn classify_resolution(sema: &FileSema, res: &Resolution) -> (u32, u32) {
    match res {
        Resolution::Local(id) | Resolution::Upvalue { binding: id, .. } => {
            match sema.binding(*id).kind {
                BindingKind::Param | BindingKind::SelfParam => (PARAMETER, 0),
                BindingKind::LocalFunction => (FUNCTION, 0),
                BindingKind::Local | BindingKind::ForVar => (VARIABLE, 0),
            }
        }
        Resolution::Global(name) => {
            let mut modifiers = M_STATIC;
            if LUA_BUILTINS.contains(&name.as_str()) {
                modifiers |= M_DEFAULT_LIBRARY;
            }
            (VARIABLE, modifiers)
        }
    }
}

/// Classify an identifier token by its syntactic context, falling back to
/// the resolution map for plain name expressions.
fn classify_lua_ident(
    token: &luabox_syntax::lua::SyntaxToken,
    names: &HashMap<TextRange, (u32, u32)>,
) -> (u32, u32) {
    let Some(parent) = token.parent() else {
        return (VARIABLE, 0);
    };
    match parent.kind() {
        SyntaxKind::LOCAL_NAME => {
            let readonly = parent
                .children()
                .any(|n| n.kind() == SyntaxKind::NAME_ATTRIB);
            let modifiers = M_DECLARATION | if readonly { M_READONLY } else { 0 };
            (VARIABLE, modifiers)
        }
        // The `const` / `close` inside `<...>` reads as a keyword.
        SyntaxKind::NAME_ATTRIB => (KEYWORD, 0),
        SyntaxKind::FUNCTION_NAME => classify_function_name_segment(token, &parent),
        SyntaxKind::LOCAL_FUNCTION_STMT => (FUNCTION, M_DECLARATION),
        SyntaxKind::PARAM => (PARAMETER, M_DECLARATION),
        SyntaxKind::METHOD_CALL_EXPR => (METHOD, 0),
        SyntaxKind::FIELD_EXPR => (PROPERTY, 0),
        SyntaxKind::TABLE_NAME_FIELD => (PROPERTY, M_DECLARATION),
        // Labels have no standard token type; `variable` keeps themes happy.
        // For variables declare at the loop header.
        SyntaxKind::LABEL_STMT | SyntaxKind::NUMERIC_FOR_STMT | SyntaxKind::GENERIC_FOR_STMT => {
            (VARIABLE, M_DECLARATION)
        }
        SyntaxKind::NAME_EXPR => names
            .get(&token.text_range())
            .copied()
            .unwrap_or((VARIABLE, 0)),
        _ => (VARIABLE, 0),
    }
}

/// One segment of `function a.b:c`: the last segment is the declared
/// function (`method` after `:`), the first is the base variable, and any
/// middle segments are properties.
fn classify_function_name_segment(
    token: &luabox_syntax::lua::SyntaxToken,
    name_node: &luabox_syntax::lua::SyntaxNode,
) -> (u32, u32) {
    let idents: Vec<_> = name_node
        .children_with_tokens()
        .filter_map(NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .collect();
    let is_last = idents
        .last()
        .is_some_and(|last| last.text_range() == token.text_range());
    if is_last {
        let is_method = name_node
            .children_with_tokens()
            .filter_map(NodeOrToken::into_token)
            .any(|t| t.kind() == SyntaxKind::COLON);
        let token_type = if is_method { METHOD } else { FUNCTION };
        return (token_type, M_DECLARATION);
    }
    let is_first = idents
        .first()
        .is_some_and(|first| first.text_range() == token.text_range());
    if is_first {
        (VARIABLE, 0)
    } else {
        (PROPERTY, 0)
    }
}

fn is_lua_keyword(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::AND_KW
            | SyntaxKind::BREAK_KW
            | SyntaxKind::DO_KW
            | SyntaxKind::ELSE_KW
            | SyntaxKind::ELSEIF_KW
            | SyntaxKind::END_KW
            | SyntaxKind::FALSE_KW
            | SyntaxKind::FOR_KW
            | SyntaxKind::FUNCTION_KW
            | SyntaxKind::GOTO_KW
            | SyntaxKind::IF_KW
            | SyntaxKind::IN_KW
            | SyntaxKind::LOCAL_KW
            | SyntaxKind::NIL_KW
            | SyntaxKind::NOT_KW
            | SyntaxKind::OR_KW
            | SyntaxKind::REPEAT_KW
            | SyntaxKind::RETURN_KW
            | SyntaxKind::THEN_KW
            | SyntaxKind::TRUE_KW
            | SyntaxKind::UNTIL_KW
            | SyntaxKind::WHILE_KW
    )
}

/// Real operators only — delimiters (parens, braces, commas, `.`, `:`)
/// stay unhighlighted, as themes expect.
fn is_lua_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::PLUS
            | SyntaxKind::MINUS
            | SyntaxKind::STAR
            | SyntaxKind::SLASH
            | SyntaxKind::PERCENT
            | SyntaxKind::CARET
            | SyntaxKind::HASH
            | SyntaxKind::AMP
            | SyntaxKind::TILDE
            | SyntaxKind::PIPE
            | SyntaxKind::LT_LT
            | SyntaxKind::GT_GT
            | SyntaxKind::SLASH_SLASH
            | SyntaxKind::EQ
            | SyntaxKind::EQ_EQ
            | SyntaxKind::TILDE_EQ
            | SyntaxKind::LT_EQ
            | SyntaxKind::GT_EQ
            | SyntaxKind::LT
            | SyntaxKind::GT
            | SyntaxKind::DOT_DOT
    )
}

// === Delta encoding =======================================================

/// Encode classified byte-range tokens as the LSP delta stream: sorted by
/// position, split at line breaks (multiline client support is optional),
/// with UTF-16 start columns and lengths.
fn encode(index: &LineIndex, mut raw: Vec<RawToken>) -> Vec<SemanticToken> {
    raw.sort_by_key(|t| t.range.start());
    let text = index.text();
    let mut data = Vec::new();
    let (mut prev_line, mut prev_start) = (0u32, 0u32);
    for tok in raw {
        let (start, end) = (usize::from(tok.range.start()), usize::from(tok.range.end()));
        let Some(token_text) = text.get(start..end) else {
            continue;
        };
        let mut seg_start = start;
        for segment in token_text.split_inclusive('\n') {
            let seg_end = seg_start + segment.len();
            let visible = segment.trim_end_matches(['\n', '\r']);
            if !visible.is_empty() {
                let pos = index.position(seg_start);
                let length: usize = visible.chars().map(char::len_utf16).sum();
                let delta_line = pos.line - prev_line;
                let delta_start = if delta_line == 0 {
                    pos.character.saturating_sub(prev_start)
                } else {
                    pos.character
                };
                data.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length: u32::try_from(length).unwrap_or(u32::MAX),
                    token_type: tok.token_type,
                    token_modifiers_bitset: tok.modifiers,
                });
                prev_line = pos.line;
                prev_start = pos.character;
            }
            seg_start = seg_end;
        }
    }
    data
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test code — panics document assumptions"
)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use luabox_db::{Analysis, AnalysisHost, Change, Dialect, Strictness};

    fn analyze(text: &str) -> (Analysis, PathBuf) {
        let mut host = AnalysisHost::new(Dialect::Lua54, Strictness::Warn);
        let path = Path::new(if cfg!(windows) {
            r"C:\ws\main.lua"
        } else {
            "/ws/main.lua"
        })
        .to_path_buf();
        host.apply_change(Change::SetFileText {
            path: path.clone(),
            dialect: Dialect::Lua54,
            text: text.to_string(),
        });
        (host.snapshot(), path)
    }

    /// One token with its delta encoding resolved back to absolute position.
    #[derive(Debug)]
    struct Decoded {
        line: u32,
        start: u32,
        length: u32,
        token_type: u32,
        modifiers: u32,
    }

    fn decode(src: &str) -> Vec<Decoded> {
        let (analysis, path) = analyze(src);
        let sema = FileSema::new(&analysis, &path).expect("sema");
        let (mut line, mut start) = (0, 0);
        semantic_tokens(&sema)
            .into_iter()
            .map(|token| {
                if token.delta_line == 0 {
                    start += token.delta_start;
                } else {
                    line += token.delta_line;
                    start = token.delta_start;
                }
                Decoded {
                    line,
                    start,
                    length: token.length,
                    token_type: token.token_type,
                    modifiers: token.token_modifiers_bitset,
                }
            })
            .collect()
    }

    /// The token starting exactly at `(line, start)`.
    fn at(tokens: &[Decoded], line: u32, start: u32) -> &Decoded {
        tokens
            .iter()
            .find(|t| t.line == line && t.start == start)
            .unwrap_or_else(|| panic!("no token at ({line}, {start}) in {tokens:?}"))
    }

    #[test]
    fn string_and_number_literals_get_their_own_types() {
        let tokens = decode("local s = \"hi\"\nlocal n = 12\n");
        assert_eq!(at(&tokens, 0, 10).token_type, STRING);
        assert_eq!(at(&tokens, 0, 10).length, 4);
        assert_eq!(at(&tokens, 1, 10).token_type, NUMBER);
    }

    #[test]
    fn a_const_local_is_readonly_and_the_attribute_reads_as_a_keyword() {
        let tokens = decode("local x <const> = 1\n");
        let name = at(&tokens, 0, 6);
        assert_eq!(name.token_type, VARIABLE);
        assert_eq!(name.modifiers, M_DECLARATION | M_READONLY);
        // `const` inside `<...>`.
        assert_eq!(at(&tokens, 0, 9).token_type, KEYWORD);
    }

    #[test]
    fn a_plain_local_declaration_is_not_readonly() {
        let tokens = decode("local y = 1\n");
        assert_eq!(at(&tokens, 0, 6).modifiers, M_DECLARATION);
    }

    #[test]
    fn a_dotted_method_declaration_splits_into_base_property_and_method() {
        let tokens = decode("function M.sub:go() end\n");
        assert_eq!(at(&tokens, 0, 9).token_type, VARIABLE);
        assert_eq!(at(&tokens, 0, 9).modifiers, 0);
        assert_eq!(at(&tokens, 0, 11).token_type, PROPERTY);
        let method = at(&tokens, 0, 15);
        assert_eq!(method.token_type, METHOD);
        assert_eq!(method.modifiers, M_DECLARATION);
    }

    #[test]
    fn a_plain_function_declaration_name_is_a_function() {
        let tokens = decode("function f() end\n");
        let name = at(&tokens, 0, 9);
        assert_eq!(name.token_type, FUNCTION);
        assert_eq!(name.modifiers, M_DECLARATION);
    }

    #[test]
    fn a_local_function_name_and_its_params_are_declarations() {
        let tokens = decode("local function g(p) return p end\n");
        let name = at(&tokens, 0, 15);
        assert_eq!(name.token_type, FUNCTION);
        assert_eq!(name.modifiers, M_DECLARATION);
        let param = at(&tokens, 0, 17);
        assert_eq!(param.token_type, PARAMETER);
        assert_eq!(param.modifiers, M_DECLARATION);
        // The *use* of the parameter carries no declaration modifier.
        let use_site = at(&tokens, 0, 27);
        assert_eq!(use_site.token_type, PARAMETER);
        assert_eq!(use_site.modifiers, 0);
    }

    #[test]
    fn field_and_method_accesses_are_properties_and_methods() {
        let tokens = decode("local o = {}\nprint(o.field)\no:method()\n");
        assert_eq!(at(&tokens, 1, 8).token_type, PROPERTY);
        assert_eq!(at(&tokens, 2, 2).token_type, METHOD);
    }

    #[test]
    fn a_table_constructor_key_is_a_property_declaration() {
        let tokens = decode("local t = { key = 1 }\n");
        let key = at(&tokens, 0, 12);
        assert_eq!(key.token_type, PROPERTY);
        assert_eq!(key.modifiers, M_DECLARATION);
    }

    #[test]
    fn a_label_renders_as_a_variable_declaration() {
        let tokens = decode("::top::\ngoto top\n");
        let label = at(&tokens, 0, 2);
        assert_eq!(label.token_type, VARIABLE);
        assert_eq!(label.modifiers, M_DECLARATION);
    }

    #[test]
    fn for_loop_variables_are_declarations_in_both_loop_forms() {
        let numeric = decode("for i = 1, 3 do end\n");
        assert_eq!(at(&numeric, 0, 4).modifiers, M_DECLARATION);
        assert_eq!(at(&numeric, 0, 4).token_type, VARIABLE);

        let generic = decode("for k, v in pairs({}) do end\n");
        assert_eq!(at(&generic, 0, 4).modifiers, M_DECLARATION);
        assert_eq!(at(&generic, 0, 7).modifiers, M_DECLARATION);
    }

    #[test]
    fn stdlib_globals_carry_the_default_library_modifier() {
        let tokens = decode("print(other)\n");
        assert_eq!(at(&tokens, 0, 0).modifiers, M_STATIC | M_DEFAULT_LIBRARY);
        assert_eq!(at(&tokens, 0, 6).modifiers, M_STATIC);
    }

    #[test]
    fn doc_comments_are_documentation_and_prose_comments_are_not() {
        let tokens = decode("---@type number\n-- prose\nlocal x = 1\n");
        assert_eq!(at(&tokens, 0, 0).token_type, COMMENT);
        assert_eq!(at(&tokens, 0, 0).modifiers, M_DOCUMENTATION);
        assert_eq!(at(&tokens, 1, 0).token_type, COMMENT);
        assert_eq!(at(&tokens, 1, 0).modifiers, 0);
    }

    #[test]
    fn keywords_and_operators_are_classified_but_delimiters_are_not() {
        let tokens = decode("local a = 1 + 2\n");
        assert_eq!(at(&tokens, 0, 0).token_type, KEYWORD);
        assert_eq!(at(&tokens, 0, 8).token_type, OPERATOR);
        assert_eq!(at(&tokens, 0, 12).token_type, OPERATOR);
        // The parentheses of a call are delimiters, not operators.
        let call = decode("f()\n");
        assert!(call.iter().all(|t| t.token_type != OPERATOR), "{call:?}");
    }

    #[test]
    fn a_multiline_comment_is_split_into_one_token_per_line() {
        let tokens = decode("--[[first\nsecond\nthird]]\nlocal x = 1\n");
        let comment_lines: Vec<&Decoded> =
            tokens.iter().filter(|t| t.token_type == COMMENT).collect();
        assert_eq!(comment_lines.len(), 3, "{comment_lines:?}");
        assert_eq!(comment_lines[0].line, 0);
        assert_eq!(comment_lines[1].line, 1);
        assert_eq!(comment_lines[1].start, 0);
        assert_eq!(
            comment_lines[1].length,
            u32::try_from("second".len()).unwrap()
        );
        assert_eq!(comment_lines[2].line, 2);
    }

    #[test]
    fn the_legend_indices_match_the_constants() {
        let legend = legend();
        assert_eq!(
            legend.token_types[PARAMETER as usize],
            SemanticTokenType::PARAMETER
        );
        assert_eq!(
            legend.token_types[OPERATOR as usize],
            SemanticTokenType::OPERATOR
        );
        assert_eq!(
            legend.token_modifiers[M_DOCUMENTATION.trailing_zeros() as usize],
            SemanticTokenModifier::DOCUMENTATION
        );
    }
}
