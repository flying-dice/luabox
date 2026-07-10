# luabox for Neovim

Native LSP integration for the [luabox](https://github.com/luabox/luabox) Lua
toolchain: typecheck, lint, hover, goto-definition, completion and document
symbols for `.lua` sources and `.lb` shape files, plus `.lb` filetype detection.

## Requirements

- **Neovim 0.11+** for the native `vim.lsp.config` / `vim.lsp.enable` API.
  (An nvim-lspconfig fallback for older versions is included in `luabox.lua`.)
- A `luabox` binary on your `PATH` (or point `cmd` at it). Build from the repo
  root with `cargo build --release` → `target/release/luabox`.

## Install

Copy `luabox.lua` onto your `runtimepath` (e.g.
`~/.config/nvim/lua/luabox.lua`), or point at this repo. Then in your config:

```lua
require("luabox").setup()
```

With overrides:

```lua
require("luabox").setup({
  cmd = { "/abs/path/to/luabox", "lsp" },   -- default: { "luabox", "lsp" }
  filetypes = { "lua", "luabox" },           -- attach to plain Lua + .lb shapes
  root_markers = { "luabox.toml", ".git" },
})
```

`setup()`:

- registers `.lb` as the `luabox` filetype (`vim.filetype.add`),
- sets the shape comment string to `// %s` for that filetype,
- defines and enables the `luabox` LSP server (`vim.lsp.config` + `vim.lsp.enable`).

The server is launched as `luabox lsp` (LSP over stdio; there is no `--stdio`
flag). It attaches to both `lua` and `luabox` (`.lb`) buffers.

## nvim-lspconfig fallback (Neovim < 0.11)

If you use nvim-lspconfig, copy the snippet at the bottom of `luabox.lua`:

```lua
vim.filetype.add({ extension = { lb = "luabox" } })
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
  to it and never redefine its syntax, so existing Lua highlighting/plugins are
  untouched.
- `.lb` files get a dedicated `luabox` filetype. Syntax highlighting for `.lb`
  beyond the LSP's semantic tokens is not provided here (no Vim syntax file);
  the VS Code extension ships a TextMate grammar if you need static highlighting.
