# frust-permissions Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically. Every failure these prevent is
# release-only: a debug build never reproduces them.
#
# `dev.frust.permissions.FrustPermissionsHost` is never referenced from a
# consuming app's Kotlin or Java. The Rust side loads it through the
# application classloader (`ClassLoader.loadClass`) and calls its `status`,
# `request`, `undeclaredPermission` and `openSettings` statics over JNI, and
# the JVM binds its `external` method by the mangled
# `Java_dev_frust_permissions_FrustPermissionsHost_nativeOnPermissionResult`
# name. R8 sees neither, so without this rule a minified build strips or
# renames the class (or the native method) and every permission call fails.
-keep class dev.frust.permissions.FrustPermissionsHost { *; }
-keep class dev.frust.permissions.FrustPermissionsHost$* { *; }
-keepclasseswithmembernames class dev.frust.permissions.FrustPermissionsHost {
    native <methods>;
}

# The manifest-declared init provider (see `src/main/AndroidManifest.xml`).
# AGP already keeps manifest components; this is belt-and-braces against a
# consuming app's own rule-set surprises — the provider is how the host gets
# an application `Context` and its Activity lifecycle callbacks before any
# Activity exists.
-keep class dev.frust.permissions.FrustPermissionsInitProvider { *; }
