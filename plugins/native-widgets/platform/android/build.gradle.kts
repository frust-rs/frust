// `frust-native-widgets`' Android side — a `com.android.library` module
// holding the plugin's ONE generic platform-view factory
// (`FrustNativeControlFactory`) and its ONE generic listener
// (`FrustNativeListener`). That two-class surface is fixed forever: which
// control a slot means travels in `paramsJson`, never in a new Kotlin class.
//
// It is wired into a generated app exactly like
// `plugins/camera/platform/android` and
// `plugins/secure-storage/platform/android` (`frust tui` → Add Plugin, or the
// plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-native-widgets")
//     project(":frust-native-widgets").projectDir = frustLocalDir("frust.plugin.frust-native-widgets.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-native-widgets"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — the crate cargo
// resolved (or a shared frust checkout) is never written to. That is exactly the
// shape `platform/android/frust-embedding` uses, and the module directory must
// be writable (Gradle 9 refuses a read-only `projectDir`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the embedding module,
// the camera/secure-storage modules, and the app template.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is the embedding module's EXCLUSIVE package (its JNI export
    // names derive from it, and a package split across two AARs is undocumented
    // territory); a plugin takes a subpackage — `docs/CODE_STANDARDS.md`'s
    // Plugin Conventions.
    //
    // Both class names under it are hard contracts:
    //   * `dev.frust.nativewidgets.FrustNativeControlFactory` is baked into
    //     three of this plugin's four JNI symbol names
    //     (`Java_dev_frust_nativewidgets_FrustNativeControlFactory_native*`,
    //     see `plugins/native-widgets/src/android/mod.rs`) AND is the
    //     platform-view `viewType` string the api layer publishes
    //     (`crate::api::builders`' Android `VIEW_TYPE`), which the embedding's
    //     `FrustViewHost` resolves reflectively by that exact FQCN — which is
    //     also why the subpackage must keep the `dev.frust.` prefix that host
    //     requires.
    //   * `dev.frust.nativewidgets.FrustNativeListener` is baked into the
    //     fourth symbol name and into the binary class name Rust constructs it
    //     by (`crate::android::ctx`'s `LISTENER_CLASS`).
    //
    // Neither may move once shipped.
    namespace = "dev.frust.nativewidgets"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and the camera/secure-storage modules exactly
    // (`compileSdk = 36`, `minSdk = 26`). A mismatch is a silent behavior
    // change, not a build error — keep them in lockstep. Every control this
    // plugin builds is a plain `android.widget` view available since well
    // before API 26, so nothing here imposes a floor of its own.
    compileSdk = 36

    defaultConfig {
        minSdk = 26

        // The R8 keep rules for the two JNI-bound, reflectively-instantiated
        // classes travel WITH this module: a consuming app inherits them
        // automatically instead of hand-editing its own `proguard-rules.pro`.
        // See `consumer-rules.pro` for why each rule is needed.
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
    // `FrustPlatformViewFactory` — the embedding-owned interface
    // `FrustNativeControlFactory` implements. `implementation`, not `api`: a
    // consuming app already depends on `:frust-embedding` directly (every
    // generated app does), and nothing in this module's public surface exposes
    // an embedding type. Note Kotlin's `internal` is MODULE-scoped, so only the
    // embedding's `public` types are visible from here.
    //
    // This is the module's ONLY dependency — every control is a plain
    // `android.widget` view built from Rust over JNI, so no AndroidX artifact
    // is needed here (the framework `Switch`/`SeekBar`/`ProgressBar`, not
    // their Material counterparts).
    implementation(project(":frust-embedding"))
}
