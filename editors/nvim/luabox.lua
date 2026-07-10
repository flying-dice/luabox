-- luabox — Neovim integration
-- ============================================================================
-- Wires the `luabox lsp` language server into Neovim and teaches the editor
-- about `.lb` shape files (filetype + `//` comment string).
--
-- Requires a `luabox` binary on your PATH (or set `opts.cmd` below). Build it
-- from the repo root with `cargo build --release` (binary: target/release/luabox).
--
-- Usage (Neovim 0.11+, no plugins needed):
--
--     require("luabox").setup()
--
-- or with overrides:
--
--     require("luabox").setup({
--       cmd = { "/abs/path/to/luabox", "lsp" },
--       filetypes = { "lua", "luabox" },
--     })
--
-- lspconfig users: see the fallback snippet at the bottom of this file.
-- ============================================================================

local M = {}

--- @class luabox.Opts
--- @field cmd?        string[]  Command to launch the server. Default {"luabox","lsp"}.
--- @field filetypes?  string[]  Filetypes to attach to. Default {"lua","luabox"}.
--- @field root_markers? string[] Project root markers. Default {"luabox.toml",".git"}.
--- @field settings?   table     Server settings forwarded to the LSP.

--- Register `.lb` filetype detection and the shape comment string.
local function register_filetype()
  -- `.lb` files are the luabox shape DSL, not Lua. Give them their own filetype.
  vim.filetype.add({
    extension = {
      lb = "luabox",
    },
  })

  -- Shape files use `//` line comments and `/* */` blocks.
  vim.api.nvim_create_autocmd("FileType", {
    pattern = "luabox",
    callback = function()
      vim.bo.commentstring = "// %s"
    end,
    desc = "luabox shape comment string",
  })
end

--- @param opts? luabox.Opts
function M.setup(opts)
  opts = opts or {}
  local cmd = opts.cmd or { "luabox", "lsp" }
  local filetypes = opts.filetypes or { "lua", "luabox" }
  local root_markers = opts.root_markers or { "luabox.toml", ".git" }

  register_filetype()

  -- Neovim 0.11+ native LSP API. `vim.lsp.config` defines the server under a
  -- name; `vim.lsp.enable` activates it for its filetypes.
  if vim.lsp.config and vim.lsp.enable then
    vim.lsp.config("luabox", {
      cmd = cmd,
      filetypes = filetypes,
      root_markers = root_markers,
      settings = opts.settings or {},
    })
    vim.lsp.enable("luabox")
  else
    vim.notify(
      "luabox: Neovim 0.11+ is required for the native LSP API. "
        .. "Use the lspconfig fallback in editors/nvim/luabox.lua instead.",
      vim.log.levels.WARN
    )
  end
end

return M

-- ============================================================================
-- Fallback for Neovim < 0.11 (or if you prefer nvim-lspconfig)
-- ============================================================================
-- Do NOT require this file for the snippet below — copy it into your config.
--
--   -- `.lb` filetype + comment string
--   vim.filetype.add({ extension = { lb = "luabox" } })
--   vim.api.nvim_create_autocmd("FileType", {
--     pattern = "luabox",
--     callback = function() vim.bo.commentstring = "// %s" end,
--   })
--
--   -- Register a custom server with lspconfig
--   local lspconfig = require("lspconfig")
--   local configs = require("lspconfig.configs")
--   if not configs.luabox then
--     configs.luabox = {
--       default_config = {
--         cmd = { "luabox", "lsp" },
--         filetypes = { "lua", "luabox" },
--         root_dir = lspconfig.util.root_pattern("luabox.toml", ".git"),
--         settings = {},
--       },
--     }
--   end
--   lspconfig.luabox.setup({})
-- ============================================================================
