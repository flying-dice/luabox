package com.luabox

import com.intellij.lang.Language

/**
 * The `.luab` shape DSL language (SHAPES.md). Registered so the platform (and
 * the LSP client) can key features off a real [Language] rather than a bare
 * file extension. Plain Lua keeps its own language id (`Lua`) — we never
 * redefine it here.
 */
object LuaboxLanguage : Language("LuaBox") {
    override fun getDisplayName(): String = "luabox shape"
    override fun isCaseSensitive(): Boolean = true
    private fun readResolve(): Any = LuaboxLanguage
}
