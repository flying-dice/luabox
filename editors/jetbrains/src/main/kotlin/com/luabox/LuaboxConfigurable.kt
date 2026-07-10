package com.luabox

import com.intellij.openapi.options.Configurable
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.ui.components.JBLabel
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent
import javax.swing.JPanel

/**
 * Settings panel: Settings ▸ Tools ▸ luabox. One field — the path to the
 * `luabox` executable. Registered in plugin.xml via `applicationConfigurable`.
 */
class LuaboxConfigurable : Configurable {
    private var pathField: TextFieldWithBrowseButton? = null
    private var panel: JPanel? = null

    override fun getDisplayName(): String = "luabox"

    override fun createComponent(): JComponent {
        val field = TextFieldWithBrowseButton()
        pathField = field
        val built = FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("luabox executable path:"), field, 1, false)
            .addComponentToRightColumn(
                JBLabel(
                    "Leave blank to resolve \"luabox\" on PATH. " +
                        "The server is launched as \"<path> lsp\".",
                ),
                1,
            )
            .addComponentFillVertically(JPanel(), 0)
            .panel
        panel = built
        return built
    }

    override fun isModified(): Boolean =
        (pathField?.text ?: "") != LuaboxSettings.getInstance().serverPath

    override fun apply() {
        LuaboxSettings.getInstance().serverPath = pathField?.text?.trim().orEmpty()
    }

    override fun reset() {
        pathField?.text = LuaboxSettings.getInstance().serverPath
    }

    override fun disposeUIResources() {
        pathField = null
        panel = null
    }
}
