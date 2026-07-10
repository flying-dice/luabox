# luabox for Neovim

Native LSP integration for the [luabox](https://github.com/luabox/luabox) Lua
toolchain: typecheck, lint, hover, goto-definition, completion, document
symbols, formatting and semantic tokens for `.lua` sources and `.luab` shape
files — plus `.luab` filetype detection and fallback syntax highlighting that
works without the LSP.

## Requirements

- **Neovim 0.11+** for the native `vim.lsp.config` / `vim.lsp.enable` API.
  (An nvim-lspconfig fallback for older versions is included in
  `lua/luabox.lua`. The filetype/syntax files work on any modern Neovim.)
- A `luabox` binary on your `PATH` (or point `cmd` at it). Build from the repo
  root with `cargo build --release` → `target/release/luabox`.

## Install

`editors/nvim` is a regular Neovim plugin (layout: `lua/`, `syntax/`,
`ftdetect/`, `ftplugin/`), so any plugin manager can point straight at it:

```lua
-- lazy.nvim
{
  dir = "/path/to/luabox/editors/nvim",  -- or a fork published as its own repo
  config = function()
    require("luabox").setup()
  end,
}
```

Or without a plugin manager, via `:packadd`:

```sh
mkdir -p ~/.local/share/nvim/site/pack/luabox/start
ln -s /path/to/luabox/editors/nvim ~/.local/share/nvim/site/pack/luabox/start/luabox
```

then in your config:

```lua
require("luabox").setup()
```

With overrides:

```lua
require("luabox").setup({
  cmd = { "/abs/path/to/luabox", "lsp" },   -- default: { "luabox", "lsp" }
  filetypes = { "lua", "luabox" },           -- attach to plain Lua + .luab shapes
  root_markers = { "luabox.toml", ".git" },
})
```

`setup()`:

- registers `.luab` as the `luabox` filetype (`vim.filetype.add`),
- sets the shape comment string to `// %s` for that filetype,
- defines and enables the `luabox` LSP server (`vim.lsp.config` + `vim.lsp.enable`).

The server is launched as `luabox lsp` (LSP over stdio; there is no `--stdio`
flag). It attaches to both `lua` and `luabox` (`.luab`) buffers.

## What the plugin files do (no `setup()` required)

Once `editors/nvim` is on the runtimepath, these work even before calling
`setup()` — i.e. without the LSP:

- `ftdetect/luabox.vim` — detects `*.luab` as the `luabox` filetype.
- `syntax/luabox.vim` — classic Vim syntax highlighting for `.luab`:
  keywords (`export`/`type`/`self`), type names,
  field/parameter names, generics, operators, `//` + `/* */` comments and
  `///` doc comments.
- `ftplugin/luabox.vim` — `commentstring=// %s` and `///` -aware `comments`.

## Formatting and semantic tokens (via the LSP)

- **Format** a buffer with `vim.lsp.buf.format()` (`.lua` gets the canonical
  luabox style for the project's edition, `.luab` the canonical shape style).
  Range formatting is accepted too, with MVP semantics: the whole document is
  formatted (the canonical formatter is whole-file). Format on save:

  ```lua
  vim.api.nvim_create_autocmd("BufWritePre", {
    pattern = { "*.lua", "*.luab" },
    callback = function() vim.lsp.buf.format() end,
  })
  ```

  The formatter never destroys code: documents with parse errors are left
  untouched (no edits), not failed.

- **Semantic tokens** are applied automatically by Neovim 0.9+ when the
  server attaches — locals vs globals, parameters, LuaCATS `---@` annotation
  comments, and `.luab` types/members/generics all get distinct (standard)
  token types, so any colorscheme with LSP semantic-token support works. Set
  `vim.lsp.semantic_tokens.enable(false, { bufnr = 0 })` (0.12+) or detach
  the server to fall back to the static syntax file.

## nvim-lspconfig fallback (Neovim < 0.11)

If you use nvim-lspconfig, copy the snippet at the bottom of `lua/luabox.lua`:

```lua
vim.filetype.add({ extension = { luab = "luabox" } })
vim.api.nvim_create_autocmd("FileType", {
  pattern = "luabox",
  callback = function() vim.bo.commentstring = "// %s" end,
})

local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")
if not configs.luabox then
  configs.luabox = {
    default_config = {
      cmd = { "luabox", "lsp" },
      filetypes = { "lua", "luabox" },
      root_dir = lspconfig.util.root_pattern("luabox.toml", ".git"),
      settings = {},
    },
  }
end
lspconfig.luabox.setup({})
```

## Notes

- The `.lua` filetype is Neovim's built-in; we only *attach* the luabox server
  to it and never redefine its syntax, so existing Lua highlighting/plugins
  are untouched.
- `.luab` files get a dedicated `luabox` filetype with its own fallback syntax
  file; the LSP's semantic tokens refine it when the server is running.
