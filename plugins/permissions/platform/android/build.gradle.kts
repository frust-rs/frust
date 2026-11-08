// `frust-permissions`' Android side — a `com.android.library` module holding the
// plugin's runtime-permission host (`FrustPermissionsHost`, the Kotlin half of
// the frozen Rust<->Kotlin contract in `plugins/permissions/src/android.rs`'s
// module doc) and its process-start init provider. It is wired into a
// generated app exactly like `plugins/connectivity/platform/android`
// (`frust tui` -> Add Plugin, or by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-permissions")
//     project(":frust-permissions").projectDir =
//         frustLocalDir("frust.plugin.frust-permissions.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-permissions"))
//
// plus the `gradle.lifecycle.beforeProject` build-directory redirect, so this
// module's build output lands under the consuming app's tree and the crate
// cargo resolved is never written to.
//
// The module declares no `<uses-permission>`: an app declares exactly the
// permissions it uses (see `src/main/AndroidManifest.xml`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the connectivity
// module and the app template.
plugins {
    id("com.android.library")
}

android {
    // A subpackage of the embedding module's exclusive `dev.frust`
    // (`docs/CODE_STANDARDS.md`'s Plugin Conventions). Baked into the JNI
    // export's symbol name
    // (`Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`),
    // so it never moves.
    namespace = "dev.frust.permissions"

    // Identical to the app template, `frust-embedding` and every other plugin
    // module (`docs/DEVELOPMENT.md`'s Platform-Support Policy) — keep them in
    // lockstep. A mismatch is a silent behavior change, not a build error.
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

dependencies {
    // Needed only for two transitive compile-classpath dependencies the
    // embedding re-exports through its own `api(...)` declarations:
    // `androidx.activity` (`ComponentActivity.activityResultRegistry` and
    // `ActivityResultContracts.RequestMultiplePermissions`, which deliver the
    // prompt's answer without the app forwarding `onRequestPermissionsResult`)
    // and `androidx.core` (`ContextCompat.checkSelfPermission`,
    // `ActivityCompat.shouldShowRequestPermissionRationale`). No embedding type
    // is used directly. `implementation`, not `api`: a consuming app already
    // depends on `:frust-embedding` itself. If the embedding ever demotes those
    // two to `implementation`, declare them here.
    implementation(project(":frust-embedding"))
}
