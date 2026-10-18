// Minimal NativeActivity host for the damage probe: no Java/Kotlin, the Rust
// cdylib (`libdamage_probe.so`, staged by cargo-ndk into app/src/main/jniLibs)
// is the whole app.
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

rootProject.name = "damageprobe"
include(":app")
