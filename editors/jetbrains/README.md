# luabox for JetBrains IDEs

A JetBrains plugin that wires the `luabox lsp` stdio language server into
IntelliJ-based IDEs through the platform's **native LSP API**
(`com.intellij.platform.lsp`): diagnostics, hover, goto-definition, completion,
document symbols, formatting and semantic highlighting for `.lua` sources.

It also registers a `Lua (luabox)` file type for `.lua` (with its own icon and
base lexical highlighting) so Lua is a first-class language rather than plain
text.

> **Edition requirement.** The native LSP API ships in **Ultimate-tier** IDEs
> (IntelliJ IDEA Ultimate, WebStorm, PyCharm Professional, GoLand, …) from
> 2023.2. This plugin targets **IntelliJ IDEA Ultimate 2024.2+** and declares
> `<depends>com.intellij.modules.lsp</depends>`, so it only loads where that API
> exists. On **Community** editions use the [LSP4IJ route](#alternative-lsp4ij-community-editions)
> below — no plugin build required.

## Requirements

- A `luabox` binary on your `PATH`, or a path set in
  **Settings ▸ Tools ▸ luabox**. Get it via the install script rather than
  building from source — see [Getting the `luabox` binary](#getting-the-luabox-binary)
  below. The server is launched as `<path> lsp`.
- To build the *plugin itself* (not required if you're just installing a
  released zip): **JDK 17+** on `PATH`/`JAVA_HOME` (the IntelliJ Platform
  Gradle Plugin 2.x and IDE 2024.2 require it — Gradle refuses to run at all
  under JDK 8).

## Getting the `luabox` binary

From a released build, run the install script for your platform (see the
repo root [`RELEASING.md`](../../RELEASING.md) for how releases are cut):

```sh
# Linux / macOS
curl -fsSL https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.sh | bash
```

```powershell
# Windows
irm https://gitlab.beluga-sirius.ts.net/flying-dice/luabox/-/raw/main/scripts/install.ps1 | iex
```

Both scripts fetch the latest tagged GitLab Release's binary asset and place
it on `PATH`. Until the first `v*` tag is pushed, both print a
`cargo install --git` source-build fallback instead.

## For maintainers: building the plugin

```sh
cd editors/jetbrains
./gradlew buildPlugin        # Windows: gradlew.bat buildPlugin
```

The distributable lands at `build/distributions/luabox-jetbrains-0.1.0.zip`.
The first build downloads the IntelliJ Platform (~1 GB), so it takes a while,
and requires **JDK 17+** on `PATH`/`JAVA_HOME` (Gradle itself refuses to run
under JDK 8).

> **Wrapper bootstrap.** This repo commits `gradle/wrapper/gradle-wrapper.properties`
> and the `gradlew` / `gradlew.bat` launchers, but not the binary
> `gradle-wrapper.jar`. Generate it once with a system Gradle
> (`gradle wrapper --gradle-version 8.10.2`) or simply open `editors/jetbrains`
> in IntelliJ IDEA and let it import the Gradle project — the jar is created
> automatically.

### Run in a sandbox IDE

```sh
./gradlew runIde
```

Launches a scratch IntelliJ IDEA Ultimate with the plugin loaded; open a `.lua`
file to start the server.

## Install from disk

1. **Settings ▸ Plugins ▸ ⚙ ▸ Install Plugin from Disk…**
2. Select `build/distributions/luabox-jetbrains-0.1.0.zip` (either your own
   build, or the zip attached to a GitLab release once one exists — see
   [Publishing to the JetBrains Marketplace](#publishing-to-the-jetbrains-marketplace)
   below for the residual manual steps).
3. Restart the IDE.
4. Ensure `luabox` is on `PATH` or set its path in **Settings ▸ Tools ▸ luabox**.

## Configuration

**Settings ▸ Tools ▸ luabox** exposes one field, the `luabox` executable path.
Leave it blank to resolve `luabox` on `PATH`. The value is stored per
installation (`luabox.xml`).

## How it works

| File | Role |
|---|---|
| `LuaboxLspServerSupportProvider.kt` | `LspServerSupportProvider` — starts `luabox lsp` when a `.lua` file opens; `ProjectWideLspServerDescriptor` builds the command line. |
| `LuaFileType.kt` / `LuaLanguage.kt` | `.lua` file type + namespaced `luabox.Lua` language. |
| `LuaHighlighting.kt` | base lexical highlighting for `.lua` (LSP semantic tokens overlay on top). |
| `LuaboxSettings.kt` | `PersistentStateComponent` holding the binary path. |
| `LuaboxConfigurable.kt` | the Settings ▸ Tools ▸ luabox panel. |
| `resources/META-INF/plugin.xml` | extension-point registrations. |
| `resources/icons/luabox.svg` | 16×16 `.lua` file icon. |

## Alternative: LSP4IJ (Community editions)

[LSP4IJ](https://plugins.jetbrains.com/plugin/23257-lsp4ij) is a free LSP/DAP
client that runs on **all** IntelliJ-based IDEs, including the Community
editions where the native LSP API is unavailable. No plugin build required.

1. **Install LSP4IJ**: *Settings ▸ Plugins ▸ Marketplace*, search **LSP4IJ**,
   install, restart.
2. **Add a language server**: *Settings ▸ Languages & Frameworks ▸ Language
   Servers ▸ `+`* → **New Language Server**.
   - **Name**: `luabox`
   - **Command**: `luabox lsp` (use an absolute path if `luabox` is not on the
     IDE's `PATH`, e.g. `C:\path\to\luabox.exe lsp`).
3. **Mappings** tab — add the file-name pattern `*.lua`.
4. **Apply**, then open a `.lua` file. The **LSP Consoles** tool window
   shows traffic for troubleshooting.

## Publishing to the JetBrains Marketplace

Publishing requires a JetBrains Marketplace account and is not automated here:

1. Create/claim a vendor at <https://plugins.jetbrains.com/> and generate a
   **permanent Marketplace token** (Marketplace ▸ *My tokens*).
2. Configure signing + publishing in `build.gradle.kts` via the
   `intellijPlatform { signing { … }; publishing { token = … } }` blocks
   (certificate chain + private key for signing; the token for upload). Keep
   secrets out of the repo — read them from environment variables.
3. `./gradlew signPlugin` then `./gradlew publishPlugin`.

First-time plugins are reviewed by JetBrains before they appear in the
Marketplace.

## References

- [LSP for plugin developers](https://plugins.jetbrains.com/docs/intellij/language-server-protocol.html)
- [The LSP API is now available to all IntelliJ IDEA users (Sept 2025)](https://blog.jetbrains.com/platform/2025/09/the-lsp-api-is-now-available-to-all-intellij-idea-users-and-plugin-developers/)
- [IntelliJ Platform Gradle Plugin 2.x](https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin.html)
- [LSP4IJ — JetBrains Marketplace](https://plugins.jetbrains.com/plugin/23257-lsp4ij)
