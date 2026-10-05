// `frust-auth-session`'s Android side — a `com.android.library` module holding
// the plugin's Chrome Custom Tabs host (`FrustAuthSessionHost`) and its
// process-start init provider. It is wired into a generated app exactly like
// `plugins/secure-storage/platform/android` (`frust tui` → Add Plugin, or the
// plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-auth-session")
//     project(":frust-auth-session").projectDir = frustLocalDir("frust.plugin.frust-auth-session.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-auth-session"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — the crate cargo
// resolved (or a shared frust checkout) is never written to. That is exactly the
// shape `platform/android/frust-embedding` uses, and the module directory must
// be writable (Gradle 9 refuses a read-only `projectDir`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the embedding
// module, the secure-storage module, and the app template.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is the embedding module's EXCLUSIVE package (its JNI export
    // names derive from it, and a package split across two AARs is
    // undocumented territory); a plugin takes a subpackage —
    // `docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions.
    //
    // `dev.frust.authsession.FrustAuthSessionHost` is a hard contract: it is
    // baked into this plugin's JNI symbol names
    // (`Java_dev_frust_authsession_FrustAuthSessionHost_native*`, see
    // `plugins/auth-session/src/android.rs`) AND is what the Rust backend
    // looks up through the application classloader. It may never move once
    // shipped.
    namespace = "dev.frust.authsession"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and the other plugin modules exactly
    // (`compileSdk = 36`, `minSdk = 26`). A mismatch is a silent behavior
    // change, not a build error — keep them in lockstep.
    compileSdk = 36

    defaultConfig {
        minSdk = 26

        // The R8 keep rule for the JNI-referenced host and the
        // manifest-declared init provider travels WITH this module: a
        // consuming app inherits it automatically instead of hand-editing its
        // own `proguard-rules.pro`. See `consumer-rules.pro` for why the rule
        // is needed at all.
        consumerProguardFiles("consumer-rules.pro")
    }

    // No BuildConfig, matching the embedding module, the secure-storage module
    // and the app template. Stated explicitly rather than inherited from an
    // AGP default so a future default flip cannot resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `androidx.browser` 1.10.0 (current stable) — `CustomTabsIntent` is the
    // whole of what `FrustAuthSessionHost` needs to launch a Chrome Custom Tab;
    // this is a Version-Pin Policy pin recorded in `docs/PLUGINS_DEVELOPMENT.md`
    // § Version Pins (the plugin's doc-update task adds the row).
    //
    // Deliberately NOT on `:frust-embedding`: the host talks only to the
    // Activity/Application the platform hands it and to Custom Tabs — it needs
    // no frust framework type, the same shape `plugins/secure-storage`'s
    // biometric helper takes for the same reason (see that module's own
    // `build.gradle.kts` comment on the seam for a module that DOES need one).
    implementation("androidx.browser:browser:1.10.0")

    // `ProviderSelectionTest` (JVM unit test, `testDebugUnitTest`) covers
    // `FrustAuthSessionHost.chooseCustomTabsProvider` — a plain Kotlin
    // decision function with no Android framework dependency, so JUnit 4 is
    // enough; no Robolectric needed. Exact pin, per
    // `docs/DEVELOPMENT.md` § Version-Pin Policy — recorded in
    // `docs/PLUGINS_DEVELOPMENT.md` § Version Pins.
    testImplementation("junit:junit:4.13.2")
}
