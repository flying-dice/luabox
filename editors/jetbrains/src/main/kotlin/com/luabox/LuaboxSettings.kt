package com.luabox

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage

/**
 * Application-level persistent settings for the luabox plugin. Currently just
 * the path to the `luabox` executable; a blank value means "resolve `luabox`
 * on PATH" (see [LuaboxLspServerDescriptor.createCommandLine]).
 *
 * Registered implicitly by `@Service(Service.Level.APP)` — no plugin.xml entry
 * needed. Stored in `luabox.xml` under the IDE config directory.
 */
@Service(Service.Level.APP)
@State(name = "LuaboxSettings", storages = [Storage("luabox.xml")])
class LuaboxSettings : PersistentStateComponent<LuaboxSettings.State> {

    class State {
        @JvmField
        var serverPath: String = ""
    }

    private var state = State()

    override fun getState(): State = state

    override fun loadState(state: State) {
        this.state = state
    }

    var serverPath: String
        get() = state.serverPath
        set(value) {
            state.serverPath = value
        }

    companion object {
        @JvmStatic
        fun getInstance(): LuaboxSettings =
            ApplicationManager.getApplication().getService(LuaboxSettings::class.java)
    }
}
