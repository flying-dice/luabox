/**
 * @file Tree-sitter grammar for the luabox `.luab` shape DSL (SHAPES-V2.md).
 * @author luabox
 * @license MIT
 *
 * TypeScript-adjacent type declarations in separate `.luab` files,
 * analyser-only. This grammar is intentionally a mirror of the hand-written
 * rowan grammar in `crates/luabox-syntax/src/shape/` (kind.rs / lexer.rs /
 * parser.rs) — it exists to give Zed (and any other tree-sitter host) syntax
 * highlighting and structural selection for `.luab`. It is NOT the source of
 * truth for the language; the Rust parser is. There are no string or numeric
 * literals in the grammar, and no item terminators — declarations are
 * self-delimiting.
 */

/* eslint-disable arrow-parens */
/* eslint-disable-next-line spaced-comment */
/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

/** A non-empty list of `rule` separated by `sep`. */
function sep1(sep, rule) {
  return seq(rule, repeat(seq(sep, rule)));
}

module.exports = grammar({
  name: "luab",

  word: ($) => $.identifier,

  // Whitespace and comments are trivia everywhere. Doc comments (`///`) are
  // kept as their own node so highlights can distinguish them (they surface
  // in hover and `luabox doc`).
  extras: ($) => [/\s/, $.line_comment, $.block_comment, $.doc_comment],

  supertypes: ($) => [$._type],

  // After `(`, an identifier may open a named function-type parameter
  // (`(x: T) => R`, `(x?: T) => R`) or a parenthesized type (`(T)`, `(T?)`,
  // a multi-return list `(A, B)`); GLR resolves at the `:` / `=>`.
  conflicts: ($) => [[$.parameter, $._type_identifier]],

  rules: {
    source_file: ($) => repeat($.type_definition),

    // === the single item form ===========================================
    // export? type IDENT generics? "=" type
    type_definition: ($) =>
      seq(
        optional($.export_modifier),
        "type",
        field("name", $._type_identifier),
        optional($.generic_parameters),
        "=",
        field("value", $._type),
      ),

    export_modifier: (_) => "export",

    // === generics ========================================================
    // "<" IDENT ("," IDENT)* ">" — v2 generics carry no bounds.
    generic_parameters: ($) =>
      seq("<", sep1(",", $.generic_parameter), ">"),

    generic_parameter: ($) => field("name", $._type_identifier),

    // Use-site args: "<T>", "<K, V>"
    generic_arguments: ($) => seq("<", sep1(",", $._type), ">"),

    // === types ===========================================================
    // Precedence, tightest first: `?` postfix, generic application, `&`, `|`,
    // `=>` loosest.
    _type: ($) =>
      choice(
        $.optional_type,
        $.union_type,
        $.intersection_type,
        $.function_type,
        $.parenthesized_type,
        $.object_type,
        $.generic_type,
        $._type_path,
      ),

    // `Point`, `love.graphics.Canvas` — a (possibly dotted) type reference.
    // References outside the declaring module are fully qualified.
    _type_path: ($) =>
      choice($._type_identifier, $.qualified_type),

    qualified_type: ($) =>
      prec.right(seq($._type_identifier, repeat1(seq(".", $._type_identifier)))),

    // `Pair<T>`, `geometry.Pair<number>` — generic application.
    generic_type: ($) =>
      prec(5, seq(field("name", $._type_path), $.generic_arguments)),

    // `T?` — nil-union postfix (binds tightest).
    optional_type: ($) => prec(6, seq($._type, "?")),

    // `A & B` — intersection; binds tighter than union.
    intersection_type: ($) => prec.left(3, seq($._type, "&", $._type)),

    // `A | B` — union.
    union_type: ($) => prec.left(2, seq($._type, "|", $._type)),

    // `(a: A) => R` — function type; parameters are named.
    function_type: ($) =>
      prec(1, seq($.parameters, "=>", field("return", $._type))),

    // `(T)` — a group; with commas a multi-return list `(A, B)` (legal only
    // in return position; the Rust checker enforces that, not this grammar).
    parenthesized_type: ($) => seq("(", sep1(",", $._type), ")"),

    // === object types & members ==========================================
    // "{" (field | method)* "}"
    object_type: ($) =>
      seq("{", repeat(choice($.field, $.method)), "}"),

    // `name?: type ,?`
    field: ($) =>
      seq(
        field("name", $.identifier),
        optional("?"),
        ":",
        field("type", $._type),
        optional(","),
      ),

    // `name(params) (":" ret)? ,?` — a `self` first parameter marks the
    // member as receiver-taking (`:` on the Lua side).
    method: ($) =>
      seq(
        field("name", $.identifier),
        $.parameters,
        optional(seq(":", field("return", $._type))),
        optional(","),
      ),

    parameters: ($) => seq("(", optional(sep1(",", $.parameter)), ")"),

    parameter: ($) =>
      choice(
        $.self,
        seq(
          field("name", $.identifier),
          optional("?"),
          ":",
          field("type", $._type),
        ),
      ),

    self: (_) => "self",

    // === leaves ==========================================================
    // A type-position identifier (`Point`, `number`, `Pair`). Aliased so
    // highlights can target @type without a separate lexer token.
    _type_identifier: ($) => alias($.identifier, $.type_identifier),

    identifier: (_) => /[A-Za-z_][A-Za-z0-9_]*/,

    // `// ...` line comment (matches the whole line).
    line_comment: (_) => token(/\/\/[^\n]*/),

    // `/// ...` doc comment — three slashes, incl. the empty `///` line. Given
    // higher precedence so `///` wins over `//`. (`////+` is highlighted as a
    // doc comment here; the rowan lexer demotes it to a plain comment — a
    // cosmetic-only divergence, documented in the README.)
    doc_comment: (_) => token(prec(1, /\/\/\/[^\n]*/)),

    // `/* ... */` — tree-sitter's regex engine can't nest; this matches a
    // single (non-nested) block. The Rust lexer nests; highlighting-wise the
    // difference is invisible.
    block_comment: (_) =>
      token(seq("/*", /[^*]*\*+([^/*][^*]*\*+)*/, "/")),
  },
});
