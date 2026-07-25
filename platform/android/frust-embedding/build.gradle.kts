// The Frust Android embedding — the framework-owned `com.android.library`
// module every generated app builds against, replacing the four platform files
// `frust create` used to copy into each project. A generated app wires it up
// out-of-tree (verified in the Phase 0 spike, `research/SPIKE_ANDROID.md`):
//
//     include(":frust-embedding")
//     project(":frust-embedding").projectDir =
//         file(providers.gradleProperty("frust.embedding.dir").get())
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect, so two apps
// can share one frust checkout without either polluting it. The consuming app's
// `frust.embedding.dir` must point at THIS directory, and it must be writable
// (Gradle 9 refuses a read-only `projectDir` — spike §7).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, exactly as in the app
// template's own build script.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is this module's EXCLUSIVE package: the JNI export names
    // (`Java_dev_frust_FrustSurfaceView_native*`) derive from it, and a split
    // package across two AARs is undocumented territory. A plugin's Kotlin
    // takes a subpackage (`dev.frust.securestorage`), never `dev.frust` itself.
    namespace = "dev.frust"

    // Both SDK levels mirror the app template's `app/build.gradle.kts` exactly
    // (`compileSdk = 36`, `minSdk = 24`). A mismatch here is a silent behavior
    // change, not a build error — keep them in lockstep.
    compileSdk = 36

    defaultConfig {
        minSdk = 24
    }

    // No BuildConfig, matching the app template (which never enables the
    // feature): `FrustViewHost`'s `TestLabelFactory` documents and relies on
    // its absence, using `ApplicationInfo.FLAG_DEBUGGABLE` in place of
    // `BuildConfig.DEBUG`. Stated explicitly rather than inherited from an AGP
    // default so a future default flip cannot resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// `api`, not `implementation`: `FrustActivity` extends `androidx.activity`'s
// `ComponentActivity` and exposes AndroidX types on its public surface, so a
// consuming app must see both transitively — the app template drops its own two
// `implementation` lines in favour of these. Versions match what that template
// declared (androidx.core backs the inset listener, edge-to-edge and system-bar
// icon contrast; androidx.activity backs the ComponentActivity base class and
// its back-press dispatcher).
dependencies {
    api("androidx.core:core:1.13.1")
    api("androidx.activity:activity:1.8.1")
}
