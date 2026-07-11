// luabox JetBrains plugin — registers the `luabox lsp` stdio language server
// through the IntelliJ Platform native LSP API (com.intellij.platform.lsp).
//
// The native LSP API is available in Ultimate-tier IDEs (IntelliJ IDEA
// Ultimate, WebStorm, PyCharm Pro, …) from 2023.2, so we target IU 2024.2 and
// depend on `com.intellij.modules.lsp`. Community-edition users should use the
// LSP4IJ route documented in README.md instead.
//
// Docs:
//   https://plugins.jetbrains.com/docs/intellij/language-server-protocol.html
//   https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin.html

plugins {
    id("java")
    // Kotlin 2.2+ is required to read the IU 2025.2 platform jars (their
    // metadata is Kotlin 2.2; KGP 2.0.x refuses it).
    id("org.jetbrains.kotlin.jvm") version "2.2.20"
    id("org.jetbrains.intellij.platform") version "2.16.0"
}

group = "com.luabox"
version = "0.1.0"

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        // Target IntelliJ IDEA 2025.3 — the first release where the native
        // LSP API (incl. the semantic tokens that drive all .lua
        // highlighting) is available to ALL users, not just licensed
        // Ultimate: in the unified 2025.2 installer the sandbox boots in
        // free mode with `com.intellij.modules.lsp` disabled, so the plugin
        // silently fails to load there.
        create("IU", "2025.3")
    }
}

intellijPlatform {
    pluginConfiguration {
        id = "com.luabox.jetbrains"
        name = "luabox"
        version = project.version.toString()
        description =
            "Lua support for JetBrains IDEs via the luabox " +
            "language server (typecheck, hover, goto-definition, completion, " +
            "document symbols, formatting, semantic highlighting)."

        ideaVersion {
            // 2025.3+: first build where `com.intellij.modules.lsp` is
            // available in every edition/mode (LSP semantic tokens drive
            // .lua highlighting).
            sinceBuild = "253"
            untilBuild = provider { null }
        }

        vendor {
            name = "luabox"
            url = "https://gitlab.beluga-sirius.ts.net/flying-dice/luabox"
        }
    }
}

kotlin {
    jvmToolchain(17)
}
