package com.luabox

import com.intellij.lang.Language

/**
 * Lua as served by the luabox language server. The id is namespaced
 * (`luabox.Lua`, not `Lua`) so installing another Lua plugin that registers
 * the plain `Lua` language id (e.g. EmmyLua) can never hard-clash at the
 * Language-registry level; extension ownership is then just a file-type
 * mapping the user controls under Settings ▸ Editor ▸ File Types.
 */
object LuaLanguage : Language("luabox.Lua") {
    override fun getDisplayName(): String = "Lua"
    override fun isCaseSensitive(): Boolean = true
    private fun readResolve(): Any = LuaLanguage
}
