pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS); repositories { google(); mavenCentral() } }
rootProject.name = "kausway-client-android"
include(":kausway-client", ":sample")
project(":sample").projectDir = file("../../examples/android-basic")
