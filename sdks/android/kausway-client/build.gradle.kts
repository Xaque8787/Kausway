plugins { id("com.android.library"); id("org.jetbrains.kotlin.android"); id("maven-publish") }
group = "io.github.xaque8787"
// Own the source archive rather than AGP's overlapping Java/Kotlin source collection.
val sdkSourcesJar = tasks.register<org.gradle.api.tasks.bundling.Zip>("kauswayReleaseSourcesJar") {
    archiveFileName.set("kausway-client-${project.version}-sources.jar")
    destinationDirectory.set(layout.buildDirectory.dir("kausway-sdk-sources"))
    from((android.sourceSets.getByName("main").java.srcDirs + file("src/main/kotlin"))
        .distinctBy { it.canonicalFile })
    include("**/*.kt", "**/*.java")
    duplicatesStrategy = DuplicatesStrategy.FAIL
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
}
android {
    namespace = "io.github.xaque8787.kausway"
    compileSdk = 35
    ndkVersion = "27.2.12479018"
    defaultConfig {
        minSdk = 24
        consumerProguardFiles("consumer-rules.pro")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    publishing { singleVariant("release") }
}
dependencies {
    implementation(files("libs/rustls-platform-verifier.jar"))
    api("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
}
afterEvaluate {
    publishing { publications { create<MavenPublication>("release") {
        from(components["release"])
        artifactId = "kausway-client-android"
        artifact(sdkSourcesJar) { classifier = "sources"; extension = "jar" }
    } } }
}
