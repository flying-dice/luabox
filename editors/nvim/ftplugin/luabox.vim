" luabox — filetype plugin for `.luab` shape files.
if exists("b:did_ftplugin")
  finish
endif
let b:did_ftplugin = 1

" Shape files use Lua-convention comments (SHAPES-V2.md): `--` lines,
" `---` docs, `--[[ ]]` blocks.
setlocal commentstring=--\ %s
setlocal comments=:---,:--

let b:undo_ftplugin = "setlocal commentstring< comments<"
