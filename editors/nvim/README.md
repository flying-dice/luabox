# luabox for Neovim

Native LSP integration for the [luabox](https://github.com/luabox/luabox) Lua
toolchain: typecheck, lint, hover, goto-definition, completion, document
symbols, formatting and semantic tokens for `.lua` sources.

## Requirements

- **Neovim 0.11+** for the native `vim.lsp.config` / `vim.lsp.enable` API.
  (An nvim-lspconfig fallback for older versions is included in
  `lua/luabox.lua`.)
- A `luabox` binary on your `PATH` (or point `cmd` at it). Build from the repo
  root with `cargo build --release` → `target/release/luabox`.

## Install

`editors/nvim` is a regular Neovim plugin (layout: `lua/`), so any plugin
manager can point straight at it:

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
  filetypes = { "lua" },                     -- attach to plain Lua
  root_markers = { "luabox.toml", ".git" },
})
```

`setup()`:

- defines and enables the `luabox` LSP server (`vim.lsp.config` + `vim.lsp.enable`).

The server is launched as `luabox lsp` (LSP over stdio; there is no `--stdio`
flag). It attaches to `lua` buffers.

## Formatting and semantic tokens (via the LSP)

- **Format** a buffer with `vim.lsp.buf.format()` (`.lua` gets the canonical
  luabox style for the project's edition).
  Range formatting is accepted too, with MVP semantics: the whole document is
  formatted (the canonical formatter is whole-file). Format on save:

  ```lua
  vim.api.nvim_create_autocmd("BufWritePre", {
    pattern = { "*.lua" },
    callback = function() vim.lsp.buf.format() end,
  })
  ```

  The formatter never destroys code: documents with parse errors are left
  untouched (no edits), not failed.

- **Semantic tokens** are applied automatically by Neovim 0.9+ when the
  server attaches — locals vs globals, parameters and LuaCATS `---@` annotation
  comments all get distinct (standard) token types, so any colorscheme with
  LSP semantic-token support works. Set
  `vim.lsp.semantic_tokens.enable(false, { bufnr = 0 })` (0.12+) or detach
  the server to disable them.

## nvim-lspconfig fallback (Neovim < 0.11)

If you use nvim-lspconfig, copy the snippet at the bottom of `lua/luabox.lua`:

```lua
local lspconfig = require("lspconfig")
local configs = require("lspconfig.configs")
if not configs.luabox then
  configs.luabox = {
    default_config = {
      cmd = { "luabox", "lsp" },
      filetypes = { "lua" },
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
