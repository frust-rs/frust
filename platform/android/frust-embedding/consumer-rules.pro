# Shipped by the frust-embedding library module and merged into every consuming
# app's R8 configuration by AGP (consumerProguardFiles). These rules protect
# classes this module owns, so they live beside the classes rather than
# accumulating in an app's own proguard-rules.pro.

# `dev.frust.FrustSurfaceView` declares the fixed `external` (JNI) methods the
# Rust side's `#[unsafe(no_mangle)]` exports bind by mangled symbol name (see
# docs/CODE_STANDARDS.md's Naming Conventions — JNI export names are LAW), and is
# itself constructed from Kotlin (FrustActivity's layout, not reflection). AGP's
# default `-keepclasseswithmembernames` native-method rule (in
# proguard-android-optimize.txt) already protects a class with `native`/`external`
# methods from *member* renaming, but does not stop R8 from renaming/removing the
# *class* itself when nothing else references it by name — belt and braces, keep
# the whole class.
-keep class dev.frust.** { *; }

# `dev.accesskit.android.Delegate` (vendored from accesskit_android 0.7.5) is
# located at runtime via a string class lookup
# (`JNIEnv::find_class("dev/accesskit/android/Delegate")`) and its native methods
# are bound dynamically via `RegisterNatives`, not the
# `Java_<pkg>_<Class>_native<Name>` symbol-mangling scheme AGP's default
# native-method keep rule expects. Since this class is found by string, not
# compiled reference, R8 cannot see that it's live and would otherwise drop it.
-keep class dev.accesskit.android.** { *; }
