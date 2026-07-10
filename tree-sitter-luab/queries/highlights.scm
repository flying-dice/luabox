; Syntax highlighting for the luabox `.luab` shape DSL.
;
; Capture names are drawn from Zed's theme-recognized set
; (https://zed.dev/docs/extensions/languages#syntax-highlighting): note
; `@comment.doc` (not `@comment.documentation`) and `@variable.parameter`
; (not `@type.parameter`). They are also standard tree-sitter captures, so the
; same file works under nvim-treesitter / Helix.
;
; Builtin vs user types are split with `#any-of?` / `#not-any-of?` so the two
; `type_identifier` rules never overlap — highlighting is correct regardless of
; whether the host resolves captures first-match or last-match.

; === Keywords =========================================================
[
  "struct"
  "trait"
  "impl"
  "for"
  "fn"
  "type"
  "use"
] @keyword

(self) @variable.special

; === Types ============================================================
; Primitive + builtin container/sugar types (SHAPES.md §3 vocabulary).
((type_identifier) @type.builtin
  (#any-of? @type.builtin
    "number" "integer" "string" "boolean" "unknown" "nil"
    "Vec" "HashMap" "Option" "Result" "Self"))

; Everything else in type position is a user-declared type.
((type_identifier) @type
  (#not-any-of? @type
    "number" "integer" "string" "boolean" "unknown" "nil"
    "Vec" "HashMap" "Option" "Result" "Self"))

; === Members ==========================================================
(field
  name: (identifier) @property)

(function_signature
  name: (identifier) @function)

(parameter
  name: (identifier) @variable.parameter)

; Module path in `use <name>;` — `@variable` is Zed-theme-recognized (Zed has
; no `@namespace`); it still reads as an imported module name.
(path
  (identifier) @variable)

; === Comments =========================================================
[
  (line_comment)
  (block_comment)
] @comment

(doc_comment) @comment.doc

; === Operators & punctuation =========================================
[
  "->"
  "?"
  "|"
  "+"
  "="
] @operator

(open_marker) @operator

[
  "{"
  "}"
  "("
  ")"
  "<"
  ">"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
] @punctuation.delimiter
