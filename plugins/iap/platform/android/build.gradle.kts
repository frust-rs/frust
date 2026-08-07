// `frust-iap`'s Android side — a `com.android.library` module holding the
// plugin's OpenIAP-backed billing host (`FrustIapHost`, stub in this spike —
// see that file's doc comment). It is wired into a generated app exactly like
// `plugins/secure-storage/platform/android` (`frust tui` -> Add Plugin, or the
// plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-iap")
//     project(":frust-iap").projectDir =
//         file("<frust checkout>/plugins/iap/platform/android")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-iap"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — two apps can
// share one frust checkout without either polluting it. That is exactly the
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
    // `FrustIapHost`'s eventual public surface is expected to hand OpenIAP types
    // (e.g. `dev.hyo.openiap.OpenIapError`) back across this module's boundary,
    // so a consumer needs them on its own compile classpath rather than only
    // transitively at runtime. No other external dependency belongs in this
    // module — this spike proves the module compiles against the pinned
    // artifact and nothing more; the billing wiring itself is later work.
    api("io.github.hyochan.openiap:openiap-google:3.0.1")
}
