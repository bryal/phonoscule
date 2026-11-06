plugins {
    id("com.android.application") version "8.13.2"
    id("org.jetbrains.kotlin.android") version "2.4.10"
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.10"
}

// Single source of truth for the version is the crate's Cargo.toml, so the APK
// and the Rust core can never disagree about what they are.
val crateVersion: String = rootDir.parentFile.resolve("Cargo.toml").readLines()
    .first { it.startsWith("version") }
    .substringAfter('"').substringBefore('"')

// The launcher icon, rendered from the desktop player's artwork rather than committed as a copy.
val iconRes = layout.buildDirectory.dir("generated/icon/res")

android {
    namespace = "zone.jo.phonoscule"
    compileSdk = 36
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "zone.jo.phonoscule"
        // AAudio wants 26; 29 is what neuma targets too.
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = crateVersion
        // One device, one ABI: no emulator, so no x86_64.
        ndk { abiFilters += "arm64-v8a" }
    }

    // Flatten away Gradle's src/main/... convention. Kotlin resolves packages
    // from the `package` declaration, not the directory path, so the usual
    // zone/jo/phonoscule/ nesting is unnecessary too.
    sourceSets["main"].apply {
        manifest.srcFile("AndroidManifest.xml")
        kotlin.srcDirs("src")
        res.srcDirs("res", iconRes)
        jniLibs.srcDirs("jniLibs")
    }

    buildFeatures { compose = true }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

dependencies {
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation(platform("androidx.compose:compose-bom:2026.06.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    // Pinned: the icon set left the BOM when it stopped growing.
    implementation("androidx.compose.material:material-icons-extended:1.7.8")
    implementation("androidx.media3:media3-session:1.11.1")
    debugImplementation("androidx.compose.ui:ui-tooling")
}

// Cross-compile the Rust cdylib into jniLibs before AGP packages the APK.
// Deliberately a plain Exec task rather than a Gradle plugin: a few legible lines
// beat a `cargo {}` DSL that hides what is actually being run.
//
// Always a release build, debug APK or not: an unoptimised Opus decoder cannot
// keep up with playback on a phone.
val cargoNdk by tasks.registering(Exec::class) {
    workingDir = rootDir.parentFile
    environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-P", "29", "-o", "app/jniLibs", "build", "--release")
}

tasks.named("preBuild") { dependsOn(cargoNdk) }

// Mipmap-free: one raster at the largest launcher density, which the launcher scales down.
val renderIcon by tasks.registering(Exec::class) {
    val svg = rootDir.parentFile.parentFile.resolve("gui/assets/icon/phonoscule.svg")
    val png = iconRes.get().file("drawable-xxxhdpi/ic_launcher.png").asFile
    inputs.file(svg)
    outputs.file(png)
    doFirst { png.parentFile.mkdirs() }
    commandLine("rsvg-convert", "-w", "192", "-h", "192", "-o", png.absolutePath, svg.absolutePath)
}

tasks.named("preBuild") { dependsOn(renderIcon) }
