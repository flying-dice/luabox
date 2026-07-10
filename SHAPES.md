# luabox shape spec — retired (v1)

**This spec is retired.** The v1 shape DSL — Rust `struct`/`trait`/`impl`
syntax, nominal conformance, and the `---@use`/`---@struct`/`---@impl`
binding tags — has been replaced by the v2 design:

→ **[SHAPES-V2.md](SHAPES-V2.md)** — TypeScript-adjacent `type` declarations,
ambient fully-qualified names, structural positional conformance, zero new
annotation tags, `export type` + `[types] entry` for publication.

The motivating critique (why the bare `impl Shape for Circle;` statement and
its `---@impl` echo carried no information a structural checker couldn't
derive) is recorded in SHAPES-V2.md's Motivation section.

For history, the v1 text is available in git:
`git show 7e80af5:SHAPES.md`.
