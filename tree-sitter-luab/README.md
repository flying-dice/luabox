# tree-sitter-luab

Tree-sitter grammar for the luabox `.luab` **shape DSL** (Rust-style
`struct` / `trait` / `impl` declarations, analyser-only — see
[`SHAPES.md`](../SHAPES.md) §3).

It exists to give tree-sitter hosts (Zed, Neovim, Helix, …) syntax
highlighting and structural selection for `.luab`. It is **not** the language's
source of truth — the hand-written rowan grammar in
[`crates/luabox-syntax/src/shape/`](../crates/luabox-syntax/src/shape) is. This
grammar mirrors it.

## Coverage (SHAPES.md §3)

- Items: `struct`, `trait`, `impl` (incl. the `impl A + B for C;` trait-sum
  sugar), `type` alias, `use`.
- Structs: fields, optional `?` types, the `..` open marker, empty bodies.
- Generics: declaration params with bounds (`<K: Hash + Eq, V>`), use-site args
  (`Vec<T>`, `HashMap<K, V>`).
- Traits: supertraits (`trait D: Shape + Sized`), `fn` signatures with `self`,
  typed params, and multi-return (`-> A, B`).
- Types: named/builtin, generic application, optional `T?`, union `A | B`,
  function types `fn(x: A) -> R`, parenthesised.
- Comments: `//` line, `///` doc (own node), `/* */` block.

The bundled fixture [`test/fixtures/spec_example.luab`](test/fixtures/spec_example.luab)
exercises all of the above and parses with zero errors.

## Develop

```sh
# Generate the parser (writes src/parser.c, src/grammar.json, src/node-types.json)
npx tree-sitter-cli generate

# Parse the fixture (should show no ERROR / MISSING nodes)
npx tree-sitter-cli parse test/fixtures/spec_example.luab

# Check the highlight query against the fixture
npx tree-sitter-cli query queries/highlights.scm test/fixtures/spec_example.luab
```

`src/parser.c` and friends are committed: Zed compiles the grammar to wasm from
`src/parser.c` on install, so they must be present at the referenced git rev.

## Highlight captures

`queries/highlights.scm` uses captures from Zed's theme-recognized set (also
standard tree-sitter): `@keyword`, `@type`, `@type.builtin`, `@property`,
`@function`, `@variable.parameter`, `@variable.special` (for `self`),
`@variable`, `@comment`, `@comment.doc`, `@operator`, `@punctuation.bracket`,
`@punctuation.delimiter`. Builtin vs user types are split with
`#any-of?`/`#not-any-of?` so the two `type_identifier` rules never overlap.

## Known deviations from the rowan parser

- **Nested block comments**: the rowan lexer nests `/* /* */ */`; tree-sitter's
  regex tokenizer matches a single (non-nested) block. Highlighting is
  unaffected in practice.
- **`////`+**: four-plus slashes are highlighted here as a doc comment; the
  rowan lexer demotes them to a plain comment (cosmetic only).
