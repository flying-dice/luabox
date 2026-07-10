/**
 * @file Tree-sitter grammar for the luabox `.luab` shape DSL (SHAPES.md §3).
 * @author luabox
 * @license MIT
 *
 * Rust struct/trait syntax in separate `.luab` files, analyser-only. This
 * grammar is intentionally a mirror of the hand-written rowan grammar in
 * `crates/luabox-syntax/src/shape/` (kind.rs / lexer.rs / parser.rs) — it
 * exists to give Zed (and any other tree-sitter host) syntax highlighting and
 * structural selection for `.luab`. It is NOT the source of truth for the
 * language; the Rust parser is. There are no string or numeric literals in the
 * grammar.
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
  // kept as their own node so highlights can distinguish them (they surface in
  // hover and `luabox doc`, SHAPES.md §2).
  extras: ($) => [/\s/, $.line_comment, $.block_comment, $.doc_comment],

  supertypes: ($) => [$._item, $._type],

  rules: {
    source_file: ($) => repeat($._item),

    _item: ($) =>
      choice(
        $.struct_definition,
        $.trait_definition,
        $.impl_declaration,
        $.type_alias,
        $.use_declaration,
      ),

    // === struct ==========================================================
    // struct IDENT generics? "{" field* open? "}"
    struct_definition: ($) =>
      seq(
        "struct",
        field("name", $._type_identifier),
        optional($.generic_parameters),
        $.struct_body,
      ),

    struct_body: ($) =>
      seq("{", repeat($.field), optional($.open_marker), "}"),

    field: ($) =>
      seq(
        field("name", $.identifier),
        ":",
        field("type", $._type),
        optional(","),
      ),

    // `..` — open shape, extra keys allowed.
    open_marker: (_) => "..",

    // === trait ===========================================================
    // trait IDENT generics? supertraits? "{" trait_fn* "}"
    trait_definition: ($) =>
      seq(
        "trait",
        field("name", $._type_identifier),
        optional($.generic_parameters),
        optional($.supertraits),
        $.trait_body,
      ),

    // ": Shape + Sized"
    supertraits: ($) => seq(":", sep1("+", $._type_identifier)),

    trait_body: ($) => seq("{", repeat($.function_signature), "}"),

    // fn IDENT "(" params? ")" ("->" ret)? ";"
    function_signature: ($) =>
      seq(
        "fn",
        field("name", $.identifier),
        $.parameters,
        optional($.return_type),
        ";",
      ),

    parameters: ($) => seq("(", optional(sep1(",", $.parameter)), ")"),

    parameter: ($) =>
      choice(
        $.self,
        seq(field("name", $.identifier), ":", field("type", $._type)),
      ),

    self: (_) => "self",

    // Multi-return: "-> A, B" (SHAPES.md §3, `Result<T,E>` lowering).
    return_type: ($) => seq("->", sep1(",", $._type)),

    // === impl ============================================================
    // impl IDENT ("+" IDENT)* generics? "for" IDENT ";"
    // (the `+` trait-sum form is the accepted sugar, SHAPES.md §12.3.)
    impl_declaration: ($) =>
      seq(
        "impl",
        sep1("+", $._trait_ref),
        "for",
        field("type", $._type_identifier),
        ";",
      ),

    _trait_ref: ($) =>
      seq($._type_identifier, optional($.generic_arguments)),

    // === type alias ======================================================
    // type IDENT generics? "=" type ";"
    type_alias: ($) =>
      seq(
        "type",
        field("name", $._type_identifier),
        optional($.generic_parameters),
        "=",
        field("value", $._type),
        ";",
      ),

    // === use =============================================================
    // use path ";"
    use_declaration: ($) => seq("use", field("path", $.path), ";"),

    path: ($) => sep1(".", $.identifier),

    // === generics ========================================================
    // "<" IDENT (":" bound ("+" bound)*)? ("," ...)* ">"
    generic_parameters: ($) =>
      seq("<", sep1(",", $.generic_parameter), ">"),

    generic_parameter: ($) =>
      seq(
        field("name", $._type_identifier),
        optional(seq(":", sep1("+", $._type_identifier))),
      ),

    // Use-site args: "<T>", "<K, V>"
    generic_arguments: ($) => seq("<", sep1(",", $._type), ">"),

    // === types ===========================================================
    _type: ($) =>
      choice(
        $.optional_type,
        $.union_type,
        $.function_type,
        $.parenthesized_type,
        $.generic_type,
        $._type_identifier,
      ),

    // `Vec<T>`, `HashMap<K, V>`, `Option<T>` — generic application.
    generic_type: ($) =>
      prec(
        3,
        seq(field("name", $._type_identifier), $.generic_arguments),
      ),

    // `T?` — nil-union postfix (binds tightest).
    optional_type: ($) => prec(4, seq($._type, "?")),

    // `A | B` — union. Binds tighter than a function type's return arrow so
    // `fn() -> A | B` reads as `fn() -> (A | B)`.
    union_type: ($) => prec.left(2, seq($._type, "|", $._type)),

    // `fn(a: A) -> R` — function type. Single return here (multi-return is a
    // trait-signature feature; keeping this single avoids a comma ambiguity
    // with the enclosing field/parameter list).
    function_type: ($) =>
      prec(
        1,
        seq("fn", $.parameters, optional(seq("->", $._type))),
      ),

    parenthesized_type: ($) => seq("(", $._type, ")"),

    // === leaves ==========================================================
    // A type-position identifier (`Point`, `number`, `Vec`). Aliased so
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
