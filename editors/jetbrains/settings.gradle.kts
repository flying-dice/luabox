// Repositories are declared in build.gradle.kts. The IntelliJ Platform
// Gradle Plugin's `org.jetbrains.intellij.platform.settings` plugin is only
// needed when repositories are centralised via dependencyResolutionManagement;
// its settings-level `intellijPlatform` DSL fails to compile under
// Gradle 9 + plugin 2.16, and FAIL_ON_PROJECT_REPOS would conflict with the
// project-level repository declarations anyway.

rootProject.name = "luabox-jetbrains"
