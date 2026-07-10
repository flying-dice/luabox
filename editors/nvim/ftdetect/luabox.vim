" luabox — `.lb` shape files get their own filetype.
" `setfiletype` only applies when no filetype is set yet, so a user override
" (or require("luabox").setup()'s vim.filetype.add) always wins.
autocmd BufRead,BufNewFile *.lb setfiletype luabox
