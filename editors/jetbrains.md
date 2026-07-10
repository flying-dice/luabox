# luabox in JetBrains IDEs

JetBrains IDEs don't ship a Lua or `.luab` language server of their own, but the
`luabox lsp` server can be wired in through the **Language Server Protocol
(LSP)**. There are two routes depending on your IDE edition and how much you
want to build.

## Route A — LSP4IJ (recommended, works everywhere)

[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) is a free,
open-source LSP/DAP client that runs on **all** IntelliJ-based IDEs, including
the free Community editions (IDEA CE, PyCharm CE) where the native LSP API is
not available. It lets you register a language server through a UI — no plugin
development required.

1. **Install LSP4IJ**: *Settings ▸ Plugins ▸ Marketplace*, search for
   **LSP4IJ**, install, and restart the IDE.
2. **Add a language server**: *Settings ▸ Languages & Frameworks ▸ Language
   Servers ▸ `+`* → **New Language Server**.
   - **Name**: `luabox`
   - **Command**: `luabox lsp`
     (use an absolute path, e.g. `/usr/local/bin/luabox lsp` or
     `C:\path\to\luabox.exe lsp`, if `luabox` is not on the IDE's `PATH`).
3. **Mappings tab** — tell LSP4IJ which files to route to the server:
   - Add a **file name pattern** `*.lua` (associate with a "Lua" language/None).
   - Add a **file name pattern** `*.luab`.
   Alternatively map by *Language* if you have a Lua plugin installed.
4. **Apply**. Open a `.lua` or `.luab` file — diagnostics, hover, go-to-definition
   and completion from `luabox lsp` now appear. The **LSP Consoles** tool window
   shows the traffic for troubleshooting.

> `.luab` files have no built-in JetBrains file type. Under *Settings ▸ Editor ▸
> File Types*, add a file type (or map the pattern `*.luab`) so the editor opens
> them as text; LSP4IJ then supplies the language features. `//` line comments
> and `/* */` blocks can be configured on that file type for comment toggling.

## Route B — Native LSP API (IntelliJ-based, plugin authors)

JetBrains exposes a native LSP client API (`com.intellij.platform.lsp`). Its
availability has expanded significantly:

- Introduced for **paid/Ultimate-tier** IDEs (IDEA Ultimate, PyCharm Pro, etc.)
  in 2023.2.
- As of **IntelliJ IDEA Ultimate 2025.2**, the LSP API remains usable even after
  a subscription lapses, under the unified-distribution model.
- JetBrains is **open-sourcing the LSP client API in 2026.2**, bringing it to
  Community editions and other IntelliJ-based products.

The native API is a **developer** API: you consume it from a plugin, not from a
settings UI. To ship a dedicated luabox plugin, implement an
`LspServerSupportProvider` that starts `luabox lsp`:

```kotlin
// build.gradle.kts targets a 2023.2+ IDE; declares <depends>com.intellij.modules.ultimate</depends>
class LuaboxLspSupportProvider : LspServerSupportProvider {
    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        serverStarter: LspServerSupportProvider.LspServerStarter,
    ) {
        if (file.extension == "lua" || file.extension == "luab") {
            serverStarter.ensureServerStarted(LuaboxLspServerDescriptor(project))
        }
    }
}

class LuaboxLspServerDescriptor(project: Project) :
    ProjectWideLspServerDescriptor(project, "luabox") {
    override fun isSupportedFile(file: VirtualFile) =
        file.extension == "lua" || file.extension == "luab"

    override fun createCommandLine() = GeneralCommandLine("luabox", "lsp")
}
```

Register the provider in `plugin.xml`:

```xml
<extensions defaultExtensionNlsKeys="com.intellij">
  <platform.lsp.serverSupportProvider
      implementation="com.luabox.LuaboxLspSupportProvider"/>
</extensions>
```

Docs: [LSP for plugin developers](https://blog.jetbrains.com/platform/2023/07/lsp-for-plugin-developers/).

## Which route to pick

| | LSP4IJ (Route A) | Native API (Route B) |
| --- | --- | --- |
| IDE editions | All (incl. Community) | Ultimate/Pro today; Community from 2026.2 |
| Setup effort | UI config, minutes | Build & install a plugin |
| Best for | End users, quick start | Shipping a polished luabox plugin |

For most users, **Route A (LSP4IJ)** is the fastest path to working diagnostics,
hover, completion and go-to-definition against `luabox lsp`.

## Sources

- [LSP4IJ — JetBrains Marketplace](https://plugins.jetbrains.com/plugin/23257-lsp4ij)
- [The LSP API Is Now Available to All IntelliJ IDEA Users and Plugin Developers (Sept 2025)](https://blog.jetbrains.com/platform/2025/09/the-lsp-api-is-now-available-to-all-intellij-idea-users-and-plugin-developers/)
- [Open-Sourcing the LSP Client API in IntelliJ IDEA 2026.2](https://blog.jetbrains.com/platform/2026/06/open-sourcing-the-lsp-client-api-in-intellij-idea-2026-2/)
- [Language Server Protocol (LSP) for Plugin Developers](https://blog.jetbrains.com/platform/2023/07/lsp-for-plugin-developers/)
