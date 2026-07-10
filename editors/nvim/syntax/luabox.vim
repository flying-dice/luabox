" Vim syntax file
" Language:  luabox shapes (.luab)
" Fallback highlighting so `.luab` files read well even without the luabox
" language server attached. When the LSP is running, Neovim's semantic
" tokens (0.9+) layer on top of these groups.

if exists("b:current_syntax")
  finish
endif

" --- Keywords (SHAPES-V2.md: the single `export? type` item form) ----------
syn keyword lbKeyword export type
syn keyword lbSelf self

" --- Types ----------------------------------------------------------------
" Builtin/primitive type names.
syn keyword lbBuiltinType number integer string boolean unknown any nil
" User types read as capitalised identifiers (declared types, generics like
" T, and each segment of a qualified reference).
syn match lbType "\<[A-Z][A-Za-z0-9_]*\>"

" --- Declarations ---------------------------------------------------------
" A method member name: `area(self): number`.
syn match lbFunction "\<[a-z_][A-Za-z0-9_]*\>\ze\s*("
" A field or parameter name before `:` / `?:`.
syn match lbField "\<[a-z_][A-Za-z0-9_]*\>\ze?\=\s*:"

" --- Operators & generics --------------------------------------------------
syn match lbAngle "[<>]"
syn match lbOperator "=>\||\|?\|&\|="

" --- Comments ---------------------------------------------------------------
" Order matters: `///` doc comments are defined after `//` so they win.
syn region lbComment start="//" end="$" contains=@Spell
syn region lbDocComment start="///" end="$" contains=@Spell
" `/* */` blocks nest (SHAPES-V2.md), hence the self-containment.
syn region lbBlockComment start="/\*" end="\*/" contains=lbBlockComment,@Spell

" --- Default highlight links -------------------------------------------------
hi def link lbKeyword      Keyword
hi def link lbSelf         Special
hi def link lbBuiltinType  Type
hi def link lbType         Type
hi def link lbFunction     Function
hi def link lbField        Identifier
hi def link lbAngle        Delimiter
hi def link lbOperator     Operator
hi def link lbComment      Comment
hi def link lbDocComment   SpecialComment
hi def link lbBlockComment Comment

let b:current_syntax = "luabox"
