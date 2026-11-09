// `frust-file-dialogs`' Android side: a `com.android.library` module holding the
// plugin's Storage Access Framework host (`FrustFileDialogsHost`, the Kotlin
// half of the frozen Rust<->Kotlin contract in
// `plugins/file-dialogs/src/android.rs`'s module doc) and its process-start init
// provider. It is wired into a generated app (`frust tui` -> Add Plugin, or by
// hand):
//
//     // android/settings.gradle.kts
//     include(":frust-file-dialogs")
//     project(":frust-file-dialogs").projectDir =
//         frustLocalDir("frust.plugin.frust-file-dialogs.dir")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-file-dialogs"))
//
// plus the `gradle.lifecycle.beforeProject` build-directory redirect, so this
// module's build output lands under the consuming app's tree and the crate
// cargo resolved is never written to.
//
// The module declares no `<uses-permission>`: the Storage Access Framework
// needs none (see `src/main/AndroidManifest.xml`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the app template.
plugins {
    id("com.android.library")
}

android {
    // A subpackage of the embedding module's exclusive `dev.frust`
    // (`docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions). Baked into the
    // JNI exports' symbol names
    // (`Java_dev_frust_filedialogs_FrustFileDialogsHost_nativeOnPaths`, ...), so
    // it never moves.
    namespace = "dev.frust.filedialogs"

    // Identical to the app template, `frust-embedding` and every other plugin
    // module (`docs/DEVELOPMENT.md`'s Platform-Support Policy): keep them in
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
    // Needed only for one transitive compile-classpath dependency the
    // embedding re-exports through its own `api(...)` declaration:
    // `androidx.activity` (`ComponentActivity.activityResultRegistry` and
    // `ActivityResultContracts.OpenDocument` / `OpenMultipleDocuments` /
    // `CreateDocument` / `OpenDocumentTree`, which open the system dialogs and
    // deliver their answer without the app forwarding `onActivityResult`). No
    // embedding type is used directly. `implementation`, not `api`: a consuming
    // app already depends on `:frust-embedding` itself. If the embedding ever
    // demotes `androidx.activity` to `implementation`, declare it here.
    implementation(project(":frust-embedding"))
}
