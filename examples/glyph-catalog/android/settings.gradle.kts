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

// frust-camera (frust-camera task 11): hand-wired exactly as
// `frust-drive::plugin::apply_gradle_module` would (task 10 hasn't landed
// the registry entry yet) — the same include/projectDir/build-dir-redirect
// trio `:frust-embedding` above uses, per that applier's own doc comment.
include(":frust-camera")
project(":frust-camera").projectDir = file("../../../plugins/camera/platform/android")

gradle.lifecycle.beforeProject {
    if (path == ":frust-camera") {
        layout.buildDirectory.set(rootDir.resolve("build/frust-camera"))
    }
}
