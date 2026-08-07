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

rootProject.name = "glyphcatalog"
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

// frust-camera: not yet a `frust tui` Add Plugin contribution, so
// hand-wired exactly as `frust-drive::plugin::apply_gradle_module` would
// apply it — the same include/projectDir/build-dir-redirect
// trio `:frust-embedding` above uses, per that applier's own doc comment.
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
// plugin's `Contribution::GradleModule`. Carried in-tree because the catalog
// is a committed example, not a generated project.
//
// This module supersedes the two `dev.frust.*` Kotlin files that used to be
// hand-copied into `app/src/main/kotlin/dev/frust/`; they were deleted with
// this wiring, and the classes now arrive under `dev.frust.nativewidgets`
// from the plugin's own library module.
include(":frust-native-widgets")
project(":frust-native-widgets").projectDir =
    file("../../../plugins/native-widgets/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-native-widgets") {
        layout.buildDirectory.set(rootDir.resolve("build/frust-native-widgets"))
    }
}

// frust-iap: not yet a `frust tui` Add Plugin contribution (spike task
// 02-android-module-spike), so hand-wired exactly as
// `frust-drive::plugin::apply_gradle_module` would apply it — the same
// include/projectDir/build-dir-redirect trio `:frust-camera` above uses, per
// that applier's own doc comment.
include(":frust-iap")
project(":frust-iap").projectDir = file("../../../plugins/iap/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-iap") {
        layout.buildDirectory.set(rootDir.resolve("build/frust-iap"))
    }
}
