// IntelliJ Platform Gradle Plugin 2.x requires the `intellijPlatform`
// repository to be declared for BOTH the settings plugin (here) and the
// project (build.gradle.kts). See:
// https://plugins.jetbrains.com/docs/intellij/tools-intellij-platform-gradle-plugin.html

rootProject.name = "luabox-jetbrains"

plugins {
    id("org.jetbrains.intellij.platform.settings") version "2.16.0"
}

dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
        mavenCentral()
        intellijPlatform {
            defaultRepositories()
        }
    }
}
