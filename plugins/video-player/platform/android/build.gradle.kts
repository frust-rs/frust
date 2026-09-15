// `frust-video-player`'s Android side — a `com.android.library` module holding
// the plugin's ExoPlayer session owner (`FrustVideoPlayerHost`) and its
// SurfaceView-backed picture factory (`VideoPlayerViewFactory`). It is wired
// into a generated app exactly like `plugins/camera/platform/android` (`frust
// tui` → Add Plugin, or the plugin README by hand):
//
//     // android/settings.gradle.kts
//     include(":frust-video-player")
//     project(":frust-video-player").projectDir =
//         file("<frust checkout>/plugins/video-player/platform/android")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-video-player"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree — two apps can
// share one frust checkout without either polluting it. That is exactly the
// shape `platform/android/frust-embedding`, `:frust-camera` and `:frust-iap`
// use, and the module directory must be writable (Gradle 9 refuses a read-only
// `projectDir`).
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied, as in the embedding module,
// the camera module, and the app template.
plugins {
    id("com.android.library")
}

android {
    // `dev.frust` is the embedding module's EXCLUSIVE package (its JNI export
    // names derive from it, and a package split across two AARs is undocumented
    // territory); a plugin takes a subpackage —
    // `docs/PLUGINS_CODE_STANDARDS.md`'s Plugin Conventions.
    //
    // Both class names under it are hard contracts:
    //   * `dev.frust.videoplayer.FrustVideoPlayerHost` is baked into this
    //     plugin's JNI symbol names
    //     (`Java_dev_frust_videoplayer_FrustVideoPlayerHost_native*`, see
    //     `plugins/video-player/src/android.rs`) AND is what the Rust backend
    //     looks up through the application classloader. It may never move once
    //     shipped.
    //   * `dev.frust.videoplayer.VideoPlayerViewFactory` is the platform-view
    //     `viewType` string `PlayerSession::view_type()` returns on Android
    //     (`contract::ANDROID_VIEW_TYPE` in `plugins/video-player/src/lib.rs`);
    //     the embedding's `FrustViewHost` resolves it reflectively by that
    //     exact FQCN (and requires the `dev.frust.` prefix).
    namespace = "dev.frust.videoplayer"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and the camera module exactly (`compileSdk =
    // 36`, `minSdk = 26`). A mismatch is a silent behavior change, not a build
    // error — keep them in lockstep.
    //
    // `compileSdk = 36` is additionally a HARD FLOOR here, not just a
    // convention: the Media3 1.11.0 AARs below stamp `minCompileSdk = 36` in
    // their metadata, and AGP fails the build outright ("dependency requires
    // libraries and applications that depend on it to compile against version
    // 36 or later") for any consumer compiling against less. Lowering this line
    // would therefore break the build rather than change behavior silently.
    // (Media3's own `minSdk` is 23, below frust's 26, so it imposes no runtime
    // floor of its own.)
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

    // No BuildConfig, matching the embedding module, the camera module and the
    // app template. Stated explicitly rather than inherited from an AGP default
    // so a future default flip cannot resurrect the class.
    buildFeatures {
        buildConfig = false
    }

    // Frust's convention across every Android module in this repo. Media3
    // itself targets Java 8, so this setting is frust's own baseline rather
    // than anything the dependency asks for.
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `FrustPlatformViewFactory` — the embedding-owned interface
    // `VideoPlayerViewFactory` implements. `implementation`, not `api`: a
    // consuming app already depends on `:frust-embedding` directly (every
    // generated app does), and nothing in this module's public surface exposes
    // an embedding type. Note Kotlin's `internal` is MODULE-scoped, so only the
    // embedding's `public` types are visible from here.
    implementation(project(":frust-embedding"))

    // Media3 1.11.0 — the current stable line (released 2026-08-05), pinned
    // exactly, both artifacts in lockstep (mixing two Media3 versions in one
    // app is unsupported upstream).
    //
    //   * `media3-exoplayer` is the player itself (`ExoPlayer.Builder`,
    //     `Player.Listener`, `setVideoSurfaceView`) and carries
    //     `DefaultMediaSourceFactory`, whose `DefaultDataSource` already routes
    //     `asset:///…` URIs through `AssetDataSource` — no extra module for the
    //     bundled-asset source kind.
    //   * `media3-exoplayer-hls` is present so an `.m3u8` URL plays:
    //     `DefaultMediaSourceFactory` detects the playlist by content type and
    //     REFLECTIVELY loads `HlsMediaSource.Factory`, so without this artifact
    //     on the runtime classpath an HLS URL fails at playback time rather
    //     than at build time. Nothing in this module names an HLS type.
    //
    // `media3-ui` is deliberately ABSENT: `PlayerView` owns a view hierarchy,
    // transport controls and a resize policy of its own, and its fill modes
    // rely on an ancestor clipping a compositor layer — exactly what frust's
    // window arrangement was observed not to honour (see
    // `VideoPlayerViewFactory.kt`'s *Geometry* note, which inherits the camera
    // module's device finding). This module hosts a plain `SurfaceView` sized
    // to fit inside its slot instead; frust widgets paint the controls.
    implementation("androidx.media3:media3-exoplayer:1.11.0")
    implementation("androidx.media3:media3-exoplayer-hls:1.11.0")
}
