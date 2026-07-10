//! Semantic tokens (`textDocument/semanticTokens/full`) for both languages.
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
//! **`.luab` shapes** (token walk over the shape tree):
//! - `struct`/`trait`/`impl`/`for`/`fn`/`type`/`use`/`self` keywords;
//! - `///` doc comments get `documentation`, `//`+`/* */` stay `comment`;
//! - declared names: structs `class`, traits `interface`, aliases `type`,
//!   generic parameters `typeParameter`, fields `property`, trait fns
//!   `method`, params `parameter`, `use` paths `namespace`;
//! - type references resolve against the file's declarations and the
//!   enclosing item's generic parameters: a trait name is `interface`, a
//!   struct name `class`, an in-scope generic `typeParameter`, else `type`.
//!
//! Tokens are emitted in the LSP delta encoding with UTF-16 columns (via
//! [`LineIndex`]); multi-line tokens (long strings/comments) are split into
//! one token per line because clients need not support multiline tokens.

use std::collections::{HashMap, HashSet};

use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend};
use luabox_hir::{BindingKind, Resolution};
use luabox_syntax::lua::SyntaxKind;
use luabox_syntax::shape::{
    self, ShapeSyntaxKind, ShapeSyntaxNode, ShapeSyntaxToken,
    ast::{AstNode, ShapeFile},
};
use rowan::{NodeOrToken, TextRange};

use crate::line_index::LineIndex;
use crate::sema::FileSema;

// === Legend ===============================================================
// The indices below are the positions in `legend()`; keep them in sync.

// Indices 0 (`namespace`) and 3 (`interface`) stay in the legend for
// LuaCATS-side classification and client compatibility, but the v2 shape
// grammar no longer produces them.
const TYPE: u32 = 1;
const CLASS: u32 = 2;
const TYPE_PARAMETER: u32 = 4;
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
pub fn lua_tokens(sema: &FileSema) -> Vec<SemanticToken> {
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

// === .luab shapes ===========================================================

/// Semantic tokens for one `.luab` shape file. Parse errors do not disable
/// highlighting — the tree is lossless, so whatever parsed still colours.
#[must_use]
pub fn lb_tokens(text: &str) -> Vec<SemanticToken> {
    let root = shape::parse(text).syntax();
    let decls = LbDecls::harvest(&root);
    let index = LineIndex::new(text);

    let mut raw = Vec::new();
    for element in root.descendants_with_tokens() {
        let NodeOrToken::Token(token) = element else {
            continue;
        };
        let classified = match token.kind() {
            ShapeSyntaxKind::DOC_COMMENT => Some((COMMENT, M_DOCUMENTATION)),
            ShapeSyntaxKind::COMMENT => Some((COMMENT, 0)),
            ShapeSyntaxKind::TYPE_KW | ShapeSyntaxKind::EXPORT_KW | ShapeSyntaxKind::SELF_KW => {
                Some((KEYWORD, 0))
            }
            ShapeSyntaxKind::FAT_ARROW
            | ShapeSyntaxKind::PIPE
            | ShapeSyntaxKind::AMP
            | ShapeSyntaxKind::QUESTION
            | ShapeSyntaxKind::EQ => Some((OPERATOR, 0)),
            ShapeSyntaxKind::IDENT => Some(classify_lb_ident(&token, &decls)),
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
    encode(&index, raw)
}

/// The file's declared names, for resolving type references.
struct LbDecls {
    types: HashSet<String>,
}

impl LbDecls {
    fn harvest(root: &ShapeSyntaxNode) -> Self {
        let mut types = HashSet::new();
        if let Some(file) = ShapeFile::cast(root.clone()) {
            for item in file.items() {
                types.extend(item.name());
            }
        }
        Self { types }
    }
}

fn classify_lb_ident(token: &ShapeSyntaxToken, decls: &LbDecls) -> (u32, u32) {
    let Some(parent) = token.parent() else {
        return (TYPE, 0);
    };
    match parent.kind() {
        ShapeSyntaxKind::TYPE_DEF => (TYPE, M_DECLARATION),
        ShapeSyntaxKind::GENERIC_PARAM => (TYPE_PARAMETER, M_DECLARATION),
        ShapeSyntaxKind::FIELD => (PROPERTY, M_DECLARATION),
        ShapeSyntaxKind::METHOD => (METHOD, M_DECLARATION),
        ShapeSyntaxKind::PARAM => (PARAMETER, M_DECLARATION),
        ShapeSyntaxKind::TYPE_REF => classify_lb_type_ref(token, decls),
        _ => (TYPE, 0),
    }
}

/// A type reference: an in-scope generic parameter, a sibling declaration,
/// or a plain type (primitives, qualified names, unknowns).
fn classify_lb_type_ref(token: &ShapeSyntaxToken, decls: &LbDecls) -> (u32, u32) {
    let name = token.text();
    if generic_param_in_scope(token, name) {
        return (TYPE_PARAMETER, 0);
    }
    if decls.types.contains(name) {
        return (CLASS, 0);
    }
    (TYPE, 0)
}

/// Whether an enclosing item declares a generic parameter named `name`.
fn generic_param_in_scope(token: &ShapeSyntaxToken, name: &str) -> bool {
    token.parent_ancestors().any(|ancestor| {
        ancestor
            .children()
            .filter(|n| n.kind() == ShapeSyntaxKind::GENERIC_PARAMS)
            .flat_map(|n| n.children())
            .filter(|n| n.kind() == ShapeSyntaxKind::GENERIC_PARAM)
            .filter_map(|n| {
                n.children_with_tokens()
                    .filter_map(NodeOrToken::into_token)
                    .find(|t| t.kind() == ShapeSyntaxKind::IDENT)
            })
            .any(|t| t.text() == name)
    })
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
