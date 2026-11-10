// `frust-background-task`'s Android side: a `com.android.library` module holding
// the foreground service (`FrustBackgroundTaskService`) and the host the Rust
// backend calls (`FrustBackgroundTaskHost`). It is wired into a generated app
// like `plugins/permissions/platform/android` (`frust tui` -> Add Plugin, or by
// hand):
//
//     // android/settings.gradle.kts
//     include(":frust-background-task")
//     project(":frust-background-task").projectDir =
//         frustLocalDir("frust.plugin.frust-background-task.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-background-task"))
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
    // the JNI class name the Rust side loads, so it never moves.
    namespace = "dev.frust.backgroundtask"

    // Identical to the app template, `frust-embedding` and every other plugin
    // module (`docs/DEVELOPMENT.md`'s Platform-Support Policy) - keep them in
    // lockstep.
    compileSdk = 36

    defaultConfig {
        minSdk = 29

        // The R8 keep rules for the JNI-loaded host and the manifest-declared
        // service travel with this module. See `consumer-rules.pro`.
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
    // `NotificationCompat` and `ServiceCompat` come from the `androidx.core` the
    // embedding module re-exports through its own `api(...)` declaration (its
    // 1.13.1 pin), so this module adds no version of its own. `implementation`,
    // not `api`: a consuming app already depends on `:frust-embedding`.
    implementation(project(":frust-embedding"))
}
