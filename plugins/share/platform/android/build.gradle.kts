// `frust-share`'s Android side: a `com.android.library` module holding the
// plugin's file-share host (`FrustShareHost`) and the `FileProvider` subclass
// (`FrustShareFileProvider`) that serves the copies it shares. Text and URL
// sharing stay plain JNI in `plugins/share/src/android.rs`; only file sharing
// needs this module. It is wired into a generated app like
// `plugins/permissions/platform/android` (`frust tui` -> Add Plugin, or by
// hand):
//
//     // android/settings.gradle.kts
//     include(":frust-share")
//     project(":frust-share").projectDir =
//         frustLocalDir("frust.plugin.frust-share.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-share"))
//
// plus the `gradle.lifecycle.beforeProject` build-directory redirect, so this
// module's build output lands under the consuming app's tree.
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the other plugin
// modules and the app template.
plugins {
    id("com.android.library")
}

android {
    // A subpackage of the embedding module's exclusive `dev.frust`. Baked into
    // the JNI class name the Rust side loads (`dev.frust.share.FrustShareHost`),
    // so it never moves.
    namespace = "dev.frust.share"

    // Identical to the app template, `frust-embedding` and every other plugin
    // module (`docs/DEVELOPMENT.md`'s Platform-Support Policy) - keep them in
    // lockstep.
    compileSdk = 36

    defaultConfig {
        minSdk = 29

        // The R8 keep rules for the JNI-loaded host and the manifest-declared
        // provider travel with this module. See `consumer-rules.pro`.
        consumerProguardFiles("consumer-rules.pro")
    }

    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `androidx.core.content.FileProvider` comes from the `androidx.core` the
    // embedding module re-exports through its own `api(...)` declaration (its
    // 1.13.1 pin), so this module adds no version of its own. `implementation`,
    // not `api`: a consuming app already depends on `:frust-embedding`.
    implementation(project(":frust-embedding"))
}
