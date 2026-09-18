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

rootProject.name = "material3demo"
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