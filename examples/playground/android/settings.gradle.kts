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

// Keep the shared frust checkout pristine: this project's build output goes
// under THIS app's tree, so two apps can build against one checkout.
gradle.lifecycle.beforeProject {
    if (path == ":frust-embedding") {
        layout.buildDirectory.set(rootDir.resolve("build/frust-embedding"))
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
        layout.buildDirectory.set(rootDir.resolve("build/frust-camera"))
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
        layout.buildDirectory.set(rootDir.resolve("build/frust-native-widgets"))
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
        layout.buildDirectory.set(rootDir.resolve("build/frust-video-player"))
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
        layout.buildDirectory.set(rootDir.resolve("build/frust-iap"))
    }
}
