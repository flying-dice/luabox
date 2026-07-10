# tree-sitter-luab

Tree-sitter grammar for the luabox `.luab` **shape modules**
(TypeScript-adjacent `type` declarations, analyser-only — see
[`SHAPES-V2.md`](../SHAPES-V2.md)).

It exists to give tree-sitter hosts (Zed, Neovim, Helix, …) syntax
highlighting and structural selection for `.luab`. It is **not** the language's
source of truth — the hand-written rowan grammar in
[`crates/luabox-syntax/src/shape/`](../crates/luabox-syntax/src/shape) is. This
grammar mirrors it.

## Coverage (SHAPES-V2.md)

- The single item form: `export? type Name<T, ...> = <type-expr>` — no item
  terminators; declarations are self-delimiting.
- Object types: fields (`name?: T`), method members (`area(self): number`)
  with `self`/named/optional parameters.
- Generics: unbounded declaration params (`<T>`, `<K, V>`), use-site args
  (`Pair<number>`, `geometry.Pair<T>`).
- Types: named references — bare or fully qualified (`love.graphics.Canvas`)
  — generic application, optional `T?`, intersection `A & B`, union `A | B`,
  function types `(x: A) => R`, parenthesised groups and multi-return lists
  `(A, B)`.
- Comments (Lua conventions): `--` line, `---` doc (own node), `--[[ ]]`
  block.

The bundled fixtures
[`test/fixtures/spec_example.luab`](test/fixtures/spec_example.luab) and
[`test/fixtures/edge_cases.luab`](test/fixtures/edge_cases.luab) exercise all
of the above and parse with zero errors.

## Develop

```sh
# Generate the parser (writes src/parser.c, src/grammar.json, src/node-types.json)
npx tree-sitter-cli generate

# Parse the fixtures (should show no ERROR / MISSING nodes)
npx tree-sitter-cli parse test/fixtures/spec_example.luab

# Check the highlight query against the fixture
npx tree-sitter-cli query queries/highlights.scm test/fixtures/spec_example.luab
```

`src/parser.c` and friends are committed: Zed compiles the grammar to wasm from
`src/parser.c` on install, so they must be present at the referenced git rev.

## Highlight captures

`queries/highlights.scm` uses captures from Zed's theme-recognized set (also
standard tree-sitter): `@keyword`, `@type`, `@type.builtin`, `@property`,
`@function.method`, `@variable.parameter`, `@variable.special` (for `self`),
`@comment`, `@comment.doc`, `@operator`, `@punctuation.bracket`,
`@punctuation.delimiter`. Builtin vs user types are split with
`#any-of?`/`#not-any-of?` so the two `type_identifier` rules never overlap.

## Known deviations from the rowan parser

- **Long-bracket levels**: the rowan lexer accepts any `=` level
  (`--[=[ ]=]`); tree-sitter's regex tokenizer can't match balanced levels
  without an external scanner, so only level-0 `--[[ ]]` highlights as a
  block here.
- **`----`+**: four-plus dashes are highlighted here as a doc comment; the
  rowan lexer demotes them to a plain comment, as LuaCATS does (cosmetic
  only).
