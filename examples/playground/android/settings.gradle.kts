pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "playground"
include(":app")

// Frust's Android embedding (the Kotlin host: FrustActivity, FrustSurfaceView,
// FrustViewHost, the accesskit delegate) is a `com.android.library` module
// consumed by path out of the frust checkout. `frust.embedding.dir` lives in
// `gradle.properties` — the single line to edit when this project moves to
// another machine. It becomes a published Maven coordinate post-crates.io.
include(":frust-embedding")
project(":frust-embedding").projectDir =
    file(providers.gradleProperty("frust.embedding.dir").get())

// Keep the shared frust checkout pristine AND keep this app's source tree
// clean: every Gradle build directory this project has — the root project's,
// `:app`'s, and the out-of-tree `:frust-embedding` module's — is redirected
// under THIS app's `build/android/` root. So two apps can build against one
// frust checkout without either polluting it, nothing is written under
// `android/build` or `android/app/build` any more, and one `frust clean`
// removes the lot. (Flutter's generated project does the same thing:
// `rootProject.buildDir = ../build`, every subproject under it.)
//
// `rootDir` is THIS `android/` directory, so `../build/android` is
// `<app>/build/android`. These three paths are the Gradle half of
// `frust-drive`'s `build_dirs::BuildLayout` (`android_root` / `android_app` /
// `android_embedding`) — that module is where the CLI looks for the outputs,
// so a change here without a change there loses the APK.
//
// Gradle's own per-project cache is NOT set here: it is written during
// configuration, before this block runs, so the Frust CLI passes
// `--project-cache-dir <app>/build/android/.gradle` on the `gradlew` command
// line instead. A hand-run `./gradlew` from `android/` gets the default
// `android/.gradle` — harmless, and `frust clean` removes it as a legacy path.
gradle.lifecycle.beforeProject {
    when (path) {
        ":" -> layout.buildDirectory.set(rootDir.resolve("../build/android"))
        ":app" -> layout.buildDirectory.set(rootDir.resolve("../build/android/app"))
        ":frust-embedding" ->
            layout.buildDirectory.set(rootDir.resolve("../build/android/frust-embedding"))
    }
}

// frust:plugin-includes — plugin-contributed `include(...)` lines go below.

// frust-camera: hand-wired exactly as `frust-drive::plugin::apply_gradle_module`
// applies it to a scaffolded app (this example wires its plugins directly
// rather than through Add Plugin) — the same include/projectDir/build-dir-
// redirect trio `:frust-embedding` above uses, per that applier's own doc
// comment.
include(":frust-camera")
project(":frust-camera").projectDir = file("../../../plugins/camera/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-camera") {
        layout.buildDirectory.set(rootDir.resolve("../build/android/frust-camera"))
    }
}

// frust-native-widgets: the same
// include/projectDir/build-dir-redirect trio as `:frust-camera` above, and
// exactly what `frust-drive::plugin::apply_gradle_module` writes for this
// plugin's `Contribution::GradleModule`. Carried in-tree because playground is
// a committed example, not a generated project.
//
// The module supplies the `dev.frust.nativewidgets` factory/listener classes;
// no per-control Kotlin lives in this app's own source tree.
include(":frust-native-widgets")
project(":frust-native-widgets").projectDir =
    file("../../../plugins/native-widgets/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-native-widgets") {
        layout.buildDirectory.set(rootDir.resolve("../build/android/frust-native-widgets"))
    }
}

// frust-video-player: the same include/projectDir/build-dir-redirect trio as
// `:frust-camera` above, and exactly what `frust-drive::plugin::apply_gradle_module`
// writes for this plugin's `Contribution::GradleModule`. Carried in-tree
// because playground is a committed example, not a generated project.
//
// The module supplies the `dev.frust.videoplayer` session host + picture
// factory; no per-control Kotlin lives in this app's own source tree.
include(":frust-video-player")
project(":frust-video-player").projectDir =
    file("../../../plugins/video-player/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-video-player") {
        layout.buildDirectory.set(rootDir.resolve("../build/android/frust-video-player"))
    }
}

// frust-iap: the R8 keep-rule tripwire vehicle for the openiap-google pin
// (docs/PLUGINS_DEVELOPMENT.md, `openiap-google 3.0.1` row): this Gradle module
// ensures the plugin's `consumer-rules.pro` keep rules are exercised during
// minification and the pin stays in sync with upstream. Playground has no
// Rust-side IAP dep or page yet (this is scaffolding only); removing this
// include orphans the tripwire and lets the version pin silently rot.
//
// Hand-wired exactly as `frust-drive::plugin::apply_gradle_module` applies it
// to a scaffolded app (this example predates Add Plugin and wires its plugins
// directly) — the same include/projectDir/build-dir-redirect trio `:frust-camera`
// above uses, per that applier's own doc comment.
include(":frust-iap")
project(":frust-iap").projectDir = file("../../../plugins/iap/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-iap") {
        layout.buildDirectory.set(rootDir.resolve("../build/android/frust-iap"))
    }
}

// frust-auth-session: the same include/projectDir/build-dir-redirect trio as
// `:frust-camera` above, and exactly what `frust-drive::plugin::apply_gradle_module`
// writes for this plugin's `Contribution::GradleModule`. Carried in-tree
// because playground is a committed example, not a generated project.
//
// The module supplies the Chrome Custom Tabs host (`FrustAuthSessionHost`)
// + its process-start init provider; no per-control Kotlin lives in this
// app's own source tree. `src/pages/auth_session.rs`'s "Auth" section is
// the first end-to-end exercise of this module against the Rust JNI backend
// (`plugins/auth-session/src/android.rs`).
include(":frust-auth-session")
project(":frust-auth-session").projectDir =
    file("../../../plugins/auth-session/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-auth-session") {
        layout.buildDirectory.set(rootDir.resolve("../build/android/frust-auth-session"))
    }
}
