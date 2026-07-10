; Syntax highlighting for the luabox `.luab` shape DSL (SHAPES-V2.md).
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
  "type"
] @keyword

(export_modifier) @keyword

(self) @variable.special

; === Types ============================================================
; Primitive + builtin container/sugar types (SHAPES-V2.md vocabulary).
((type_identifier) @type.builtin
  (#any-of? @type.builtin
    "number" "integer" "string" "boolean" "unknown" "any" "nil"
    "Vec" "HashMap" "Option" "Result"))

; Everything else in type position is a user-declared type (including each
; segment of a qualified reference like `love.graphics.Canvas`).
((type_identifier) @type
  (#not-any-of? @type
    "number" "integer" "string" "boolean" "unknown" "any" "nil"
    "Vec" "HashMap" "Option" "Result"))

; === Members ==========================================================
(field
  name: (identifier) @property)

(method
  name: (identifier) @function.method)

(parameter
  name: (identifier) @variable.parameter)

; === Comments =========================================================
[
  (line_comment)
  (block_comment)
] @comment

(doc_comment) @comment.doc

; === Operators & punctuation =========================================
[
  "=>"
  "?"
  "|"
  "&"
  "="
] @operator

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
  ":"
  "."
] @punctuation.delimiter
