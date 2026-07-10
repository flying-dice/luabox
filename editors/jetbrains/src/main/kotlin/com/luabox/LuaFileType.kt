package com.luabox

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * File type for `.lua` sources, backed by [LuaLanguage]. Claiming the
 * extension makes `.lua` a first-class file (icon, no "plugins supporting
 * *.lua" marketplace nagging, LSP semantic-token highlighting) instead of
 * detected plain text.
 */
class LuaFileType private constructor() : LanguageFileType(LuaLanguage) {
    override fun getName(): String = "Lua (luabox)"
    override fun getDescription(): String = "Lua source (served by luabox)"
    override fun getDefaultExtension(): String = "lua"
    override fun getIcon(): Icon = LuaboxIcons.FILE

    companion object {
        @JvmField
        val INSTANCE = LuaFileType()
    }
}
