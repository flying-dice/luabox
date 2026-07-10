package com.luabox

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspServerSupportProvider
import com.intellij.platform.lsp.api.ProjectWideLspServerDescriptor

/**
 * Wires the `luabox lsp` stdio language server into IntelliJ-based IDEs through
 * the native LSP API (`com.intellij.platform.lsp`, Ultimate-tier, 2023.2+).
 *
 * Registered in plugin.xml as `com.intellij.platform.lsp.serverSupportProvider`.
 * Community-edition users should use LSP4IJ instead (see README.md).
 *
 * API verified against
 * https://plugins.jetbrains.com/docs/intellij/language-server-protocol.html
 */
internal class LuaboxLspServerSupportProvider : LspServerSupportProvider {
    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        serverStarter: LspServerSupportProvider.LspServerStarter,
    ) {
        if (isLuaboxFile(file)) {
            serverStarter.ensureServerStarted(LuaboxLspServerDescriptor(project))
        }
    }
}

/**
 * Project-wide descriptor: one `luabox lsp` process per project, serving all
 * `.lua` and `.luab` files. The binary is resolved from the plugin settings
 * (see [LuaboxSettings]); a blank setting falls back to `luabox` on PATH.
 */
internal class LuaboxLspServerDescriptor(project: Project) :
    ProjectWideLspServerDescriptor(project, "luabox") {

    override fun isSupportedFile(file: VirtualFile): Boolean = isLuaboxFile(file)

    override fun createCommandLine(): GeneralCommandLine {
        val configured = LuaboxSettings.getInstance().serverPath.trim()
        val exe = configured.ifEmpty { "luabox" }
        // `luabox lsp` speaks LSP over stdio unconditionally (no --stdio flag).
        return GeneralCommandLine(exe, "lsp")
    }
}

/** `.lua` sources and `.luab` shape files are both served by luabox. */
private fun isLuaboxFile(file: VirtualFile): Boolean =
    file.extension == "lua" || file.extension == "luab"
