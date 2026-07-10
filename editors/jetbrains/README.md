# luabox for JetBrains IDEs

A JetBrains plugin that wires the `luabox lsp` stdio language server into
IntelliJ-based IDEs through the platform's **native LSP API**
(`com.intellij.platform.lsp`): diagnostics, hover, goto-definition, completion,
document symbols, formatting and semantic highlighting for `.lua` sources and
`.luab` shape files.

It also registers a `LuaBox` file type for `.luab` (with its own icon) so the
shape DSL is a first-class language rather than plain text.

> **Edition requirement.** The native LSP API ships in **Ultimate-tier** IDEs
> (IntelliJ IDEA Ultimate, WebStorm, PyCharm Professional, GoLand, …) from
> 2023.2. This plugin targets **IntelliJ IDEA Ultimate 2024.2+** and declares
> `<depends>com.intellij.modules.lsp</depends>`, so it only loads where that API
> exists. On **Community** editions use the [LSP4IJ route](#alternative-lsp4ij-community-editions)
> below — no plugin build required.

## Requirements

- A `luabox` binary on your `PATH`, or a path set in
  **Settings ▸ Tools ▸ luabox**. Build it from the repo root with
  `cargo build --release` (binary: `target/release/luabox`). The server is
  launched as `<path> lsp`.
- To build the plugin: **JDK 17+** (the IntelliJ Platform Gradle Plugin 2.x and
  IDE 2024.2 require it — Java 8 will not work).

## Build

```sh
cd editors/jetbrains
./gradlew buildPlugin        # Windows: gradlew.bat buildPlugin
```

The distributable lands at `build/distributions/luabox-0.1.0.zip`. The first
build downloads the IntelliJ Platform (~1 GB), so it takes a while.

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
or `.luab` file to start the server.

## Install from disk

1. **Settings ▸ Plugins ▸ ⚙ ▸ Install Plugin from Disk…**
2. Select `build/distributions/luabox-0.1.0.zip`.
3. Restart the IDE.
4. Ensure `luabox` is on `PATH` or set its path in **Settings ▸ Tools ▸ luabox**.

## Configuration

**Settings ▸ Tools ▸ luabox** exposes one field, the `luabox` executable path.
Leave it blank to resolve `luabox` on `PATH`. The value is stored per
installation (`luabox.xml`).

## How it works

| File | Role |
|---|---|
| `LuaboxLspServerSupportProvider.kt` | `LspServerSupportProvider` — starts `luabox lsp` when a `.lua`/`.luab` file opens; `ProjectWideLspServerDescriptor` builds the command line. |
| `LuaboxFileType.kt` / `LuaboxLanguage.kt` | `.luab` file type + `LuaBox` language. |
| `LuaboxSettings.kt` | `PersistentStateComponent` holding the binary path. |
| `LuaboxConfigurable.kt` | the Settings ▸ Tools ▸ luabox panel. |
| `resources/META-INF/plugin.xml` | extension-point registrations. |
| `resources/icons/luabox.svg` | 16×16 `.luab` file icon. |

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
3. **Mappings** tab — add file-name patterns `*.lua` and `*.luab`.
4. **Apply**, then open a `.lua`/`.luab` file. The **LSP Consoles** tool window
   shows traffic for troubleshooting.

> `.luab` has no built-in JetBrains file type under LSP4IJ. Under *Settings ▸
> Editor ▸ File Types* register the pattern `*.luab` (with `//` line and
> `/* */` block comments) so the editor opens it as text; LSP4IJ then supplies
> the language features.

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
