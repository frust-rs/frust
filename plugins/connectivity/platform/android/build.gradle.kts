// `frust-connectivity`'s Android side — a `com.android.library` module holding
// the plugin's `ConnectivityManager` host (`FrustConnectivityHost`, the Kotlin
// half of the frozen Rust<->Kotlin contract in
// `plugins/connectivity/src/android.rs`'s module doc). It is wired into a
// generated app exactly like `plugins/iap/platform/android` (`frust tui` ->
// Add Plugin, or by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-connectivity")
//     project(":frust-connectivity").projectDir =
//         frustLocalDir("frust.plugin.frust-connectivity.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-connectivity"))
//
// plus the `gradle.lifecycle.beforeProject` build-directory redirect, so this
// module's build output lands under the consuming app's tree and the crate
// cargo resolved is never written to.
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the iap module and
// the app template.
plugins {
    id("com.android.library")
}

android {
    // A subpackage of the embedding module's exclusive `dev.frust`
    // (`docs/CODE_STANDARDS.md`'s Plugin Conventions). Baked into the JNI
    // export's symbol name, so it never moves.
    namespace = "dev.frust.connectivity"

    // Identical to the app template, `frust-embedding` and every other plugin
    // module (`docs/DEVELOPMENT.md`'s Platform-Support Policy) — keep them in
    // lockstep.
    compileSdk = 36

    defaultConfig {
        minSdk = 29

        // The R8 keep rules for the JNI-referenced host and the
        // manifest-declared init provider travel with this module, so a
        // consuming app inherits them. See `consumer-rules.pro`.
        consumerProguardFiles("consumer-rules.pro")
    }

    // No BuildConfig, stated explicitly so a future AGP default flip cannot
    // resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// No dependencies: `ConnectivityManager`, `HandlerThread` and `ContentProvider`
// are all Android framework classes.
