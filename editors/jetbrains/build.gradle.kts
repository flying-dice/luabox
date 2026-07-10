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
    id("org.jetbrains.kotlin.jvm") version "2.0.21"
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
        // Target IntelliJ IDEA Ultimate 2024.2 — the native LSP API is
        // Ultimate-only at this build. "IU" = IntelliJ IDEA Ultimate; the
        // typed equivalent is `intellijIdeaUltimate("2024.2")`.
        create("IU", "2024.2")
    }
}

intellijPlatform {
    pluginConfiguration {
        id = "com.luabox.jetbrains"
        name = "luabox"
        version = project.version.toString()
        description =
            "Lua + .luab shape-file support for JetBrains IDEs via the luabox " +
            "language server (typecheck, hover, goto-definition, completion, " +
            "document symbols, formatting, semantic highlighting)."

        ideaVersion {
            sinceBuild = "242"
            // Open-ended: the LSP API surface used here is stable across
            // 2024.2+. Pin `untilBuild = "242.*"` if you want to restrict it.
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
