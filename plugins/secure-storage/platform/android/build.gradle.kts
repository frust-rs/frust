// `frust-secure-storage`'s Android side — a `com.android.library` module the
// plugin's `biometric-gate` feature wires into a generated app (`frust tui` →
// Add Plugin, or the plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-secure-storage")
//     project(":frust-secure-storage").projectDir =
//         file("<frust checkout>/plugins/secure-storage/platform/android")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-secure-storage"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — two apps can
// share one frust checkout without either polluting it. That is exactly the
// shape `platform/android/frust-embedding` uses, and the module directory must
// be writable (Gradle 9 refuses a read-only `projectDir`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the embedding module
// and the app template.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is the embedding module's EXCLUSIVE package (its JNI export
    // names derive from it, and a package split across two AARs is
    // undocumented territory); a plugin takes a subpackage.
    // `dev.frust.securestorage.FrustBiometric` is a hard contract: it is what
    // `HELPER_CLASS_BINARY` in `plugins/secure-storage/src/android.rs` looks up
    // through the application classloader. Renaming it here means renaming it
    // there.
    namespace = "dev.frust.securestorage"

    // Both SDK levels mirror the app template's `app/build.gradle.kts` and the
    // `frust-embedding` module exactly (`compileSdk = 36`, `minSdk = 24`). A
    // mismatch is a silent behavior change, not a build error — keep them in
    // lockstep. (The biometric gate itself needs API 28+; a gated open below
    // that returns `NotAvailable(UnsupportedApiLevel)` at runtime, so minSdk
    // stays at the framework's 24.)
    compileSdk = 36

    defaultConfig {
        minSdk = 24

        // The R8 keep rule for the helper travels WITH this module: a consuming
        // app inherits it automatically instead of hand-editing its own
        // `proguard-rules.pro`. See `consumer-rules.pro` for why the rule is
        // needed at all.
        consumerProguardFiles("consumer-rules.pro")
    }

    // No BuildConfig, matching the embedding module and the app template.
    // Stated explicitly rather than inherited from an AGP default so a future
    // default flip cannot resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// No dependencies — deliberately NOT on `:frust-embedding` either: the
// biometric helper needs no frust framework type, only the Android framework's
// own `BiometricPrompt`. The seam exists for the next plugin, though: one that
// needs e.g. `FrustPlatformViewFactory` adds
//
//     dependencies {
//         implementation(project(":frust-embedding"))
//     }
//
// here (Flutter's plugin subprojects take `flutter_embedding` the same way, as
// an `api` dependency). Note that Kotlin's `internal` is MODULE-scoped: an
// `internal` declaration in the embedding stays invisible from here even for a
// class in the same source package, so a framework type a plugin needs must be
// `public` there.
