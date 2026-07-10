package com.luabox

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

/**
 * File type for `.luab` shape files. Registered in plugin.xml via the
 * `com.intellij.fileType` extension point (name = "LuaBox", fieldName =
 * "INSTANCE"). Backed by [LuaboxLanguage] so the platform treats `.luab` as a
 * first-class language rather than plain text.
 *
 * The `luabox lsp` server (wired in by [LuaboxLspServerSupportProvider]) then
 * supplies diagnostics, hover, completion, etc.
 */
class LuaboxFileType private constructor() : LanguageFileType(LuaboxLanguage) {
    override fun getName(): String = "LuaBox"
    override fun getDescription(): String = "luabox shape declarations (.luab)"
    override fun getDefaultExtension(): String = "luab"
    override fun getIcon(): Icon = LuaboxIcons.FILE

    companion object {
        @JvmField
        val INSTANCE = LuaboxFileType()
    }
}
