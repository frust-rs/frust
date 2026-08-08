// `frust-camera`'s Android side — a `com.android.library` module holding the
// plugin's CameraX session owner (`FrustCameraHost`) and its SurfaceView-backed
// preview factory (`CameraPreviewFactory`). It is wired into a generated app
// exactly like `plugins/secure-storage/platform/android` (`frust tui` → Add
// Plugin, or the plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-camera")
//     project(":frust-camera").projectDir =
//         file("<frust checkout>/plugins/camera/platform/android")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-camera"))
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
    //
    // Both class names under it are hard contracts:
    //   * `dev.frust.camera.FrustCameraHost` is baked into this plugin's JNI
    //     symbol names (`Java_dev_frust_camera_FrustCameraHost_native*`, see
    //     `plugins/camera/src/android.rs`) AND is what the Rust backend looks
    //     up through the application classloader. It may never move once
    //     shipped.
    //   * `dev.frust.camera.CameraPreviewFactory` is the platform-view
    //     `viewType` string `CameraSession::preview_view_type()` returns; the
    //     embedding's `FrustViewHost` resolves it reflectively by that exact
    //     FQCN (and requires the `dev.frust.` prefix).
    namespace = "dev.frust.camera"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and the secure-storage module exactly
    // (`compileSdk = 36`, `minSdk = 26`). A mismatch is a silent behavior
    // change, not a build error — keep them in lockstep. (CameraX 1.6's own
    // minSdk is 23, below frust's 26, so it imposes no floor of its own.)
    compileSdk = 36

    defaultConfig {
        minSdk = 26

        // The R8 keep rules for the JNI-referenced host, the
        // classloader-instantiated factory and the manifest-declared init
        // provider travel WITH this module: a consuming app inherits them
        // automatically instead of hand-editing its own `proguard-rules.pro`.
        // See `consumer-rules.pro` for why each rule is needed.
        consumerProguardFiles("consumer-rules.pro")
    }

    // No BuildConfig, matching the embedding module, the secure-storage module
    // and the app template. Stated explicitly rather than inherited from an AGP
    // default so a future default flip cannot resurrect the class.
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
    // `CameraPreviewFactory` implements. `implementation`, not `api`: a
    // consuming app already depends on `:frust-embedding` directly (every
    // generated app does), and nothing in this module's public surface exposes
    // an embedding type. Note Kotlin's `internal` is MODULE-scoped, so only the
    // embedding's `public` types are visible from here.
    //
    // Two compile-classpath deps arrive transitively through the embedding's
    // own `api(...)` declarations and are deliberately not re-declared here:
    // `androidx.core` (`ActivityCompat.requestPermissions`, and the
    // `androidx.core.util.Consumer` CameraX's `provideSurface` takes) and
    // `androidx.activity` → `androidx.lifecycle:lifecycle-runtime`
    // (`LifecycleRegistry`, the session-owned lifecycle below). If the
    // embedding ever demotes those to `implementation`, declare them here.
    implementation(project(":frust-embedding"))

    // CameraX 1.6.1 (current stable, May 2026; the 1.6.0 line moved the backend
    // to CameraPipe). `camera-core` is the use cases (`Preview`,
    // `ImageCapture`, `SurfaceRequest`), `camera-camera2` the Camera2 backend
    // implementation, `camera-lifecycle` the `ProcessCameraProvider` +
    // `bindToLifecycle` seam.
    //
    // `camera-view` is deliberately ABSENT: `PreviewView` owns its own view
    // hierarchy and transform policy, and its fill scale types oversize the
    // preview and rely on an ancestor clipping a compositor layer — which is
    // exactly what real devices do not honour. This module hosts a plain
    // `SurfaceView` sized to fit inside its slot instead; see
    // `CameraPreviewFactory.kt`'s `CameraPreviewView` for the geometry that
    // forces. A `TextureView` was tried here and is not usable: its content is
    // composited inside the app window, and a vendor compositor was observed
    // dropping that content on most frames for a window configured the way
    // frust's is. Adding `camera-view` later would pull in a second,
    // conflicting preview implementation.
    implementation("androidx.camera:camera-core:1.6.1")
    implementation("androidx.camera:camera-camera2:1.6.1")
    implementation("androidx.camera:camera-lifecycle:1.6.1")
}
