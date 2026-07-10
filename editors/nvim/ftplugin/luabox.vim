" luabox — filetype plugin for `.lb` shape files.
if exists("b:did_ftplugin")
  finish
endif
let b:did_ftplugin = 1

" Shape files use `//` line comments (and `/* */` blocks, `///` docs).
setlocal commentstring=//\ %s
setlocal comments=:///,://

let b:undo_ftplugin = "setlocal commentstring< comments<"
