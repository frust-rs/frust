// `frust-iap`'s Android side — a `com.android.library` module holding the
// plugin's OpenIAP-backed billing host (`FrustIapHost`, the Kotlin half of the
// frozen Rust<->Kotlin contract in `plugins/iap/src/android.rs`'s module doc).
// It is wired into a generated app exactly like
// `plugins/secure-storage/platform/android` (`frust tui` -> Add Plugin, or the
// plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-iap")
//     project(":frust-iap").projectDir = frustLocalDir("frust.plugin.frust-iap.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-iap"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — the crate cargo
// resolved (or a shared frust checkout) is never written to. That is exactly the
// shape `platform/android/frust-embedding` uses, and the module directory must
// be writable (Gradle 9 refuses a read-only `projectDir`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the embedding module,
// the secure-storage module, and the app template.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is the embedding module's EXCLUSIVE package (its JNI export
    // names derive from it, and a package split across two AARs is undocumented
    // territory); a plugin takes a subpackage — `docs/CODE_STANDARDS.md`'s
    // Plugin Conventions.
    namespace = "dev.frust.iap"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and the camera/secure-storage modules exactly
    // (`compileSdk = 36`, `minSdk = 26`). A mismatch is a silent behavior
    // change, not a build error — keep them in lockstep (`docs/DEVELOPMENT.md`'s
    // Platform-Support Policy).
    compileSdk = 36

    defaultConfig {
        minSdk = 26

        // The R8 keep rules for the JNI-referenced host and the
        // manifest-declared init provider travel WITH this module: a consuming
        // app inherits them automatically instead of hand-editing its own
        // `proguard-rules.pro` (the `plugins/camera/platform/android`
        // precedent). See `consumer-rules.pro` for why each rule is needed.
        consumerProguardFiles("consumer-rules.pro")
    }

    // No BuildConfig, matching the embedding module, the camera/secure-storage
    // modules and the app template. Stated explicitly rather than inherited
    // from an AGP default so a future default flip cannot resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // openiap-google 3.0.1 — LAW-pinned in lockstep with the iOS side's SPM pin
    // (`docs/DEVELOPMENT.md`'s Version-Pin Policy). `api`, not `implementation`:
    // `FrustIapHost`'s public surface hands OpenIAP types
    // (e.g. `dev.hyo.openiap.OpenIapError`) back across this module's boundary,
    // so a consumer needs them on its own compile classpath rather than only
    // transitively at runtime.
    api("io.github.hyochan.openiap:openiap-google:3.0.1")

    // Kotlin coroutines, pinned to the SAME 1.9.0 `openiap-google` itself
    // resolves — two versions would give one process two coroutine runtimes.
    //
    // Declared explicitly because openiap-google's POM lists both
    // `kotlinx-coroutines-core` and `-android` at **runtime** scope, so neither
    // is on this module's compile classpath: every OpenIAP entry point is a
    // `suspend` member, and `FrustIapHost` needs `CoroutineScope`/`launch`/
    // `Dispatchers` of its own to call one from the non-suspending JNI static.
    // `-android` (not `-core`) because it brings `-core` transitively and adds
    // the `Dispatchers.Main` implementation `OpenIapStore`'s own scope requires
    // at runtime. `implementation`, not `api`: no coroutine type appears in
    // this module's own surface.
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
}
