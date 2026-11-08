# frust-photo-library Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically. Every failure these prevent is
# release-only: a debug build never reproduces them.
#
# `dev.frust.photolibrary.FrustPhotoLibraryHost` is never referenced from a
# consuming app's Kotlin or Java. The Rust side loads it through the
# application classloader (`ClassLoader.loadClass`) and calls its `pick`
# static over JNI, and the JVM binds its two `external` methods by the mangled
# `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickResult` and
# `Java_dev_frust_photolibrary_FrustPhotoLibraryHost_nativeOnPickError` names.
# R8 sees neither, so without this rule a minified build strips or renames the
# class (or the native methods) and every pick fails.
-keep class dev.frust.photolibrary.FrustPhotoLibraryHost { *; }
-keep class dev.frust.photolibrary.FrustPhotoLibraryHost$* { *; }
-keepclasseswithmembernames class dev.frust.photolibrary.FrustPhotoLibraryHost {
    native <methods>;
}

# The manifest-declared init provider (see `src/main/AndroidManifest.xml`).
# AGP already keeps manifest components; this is belt-and-braces against a
# consuming app's own rule-set surprises — the provider is how the host gets
# an application `Context` and its Activity lifecycle callbacks before any
# Activity exists.
-keep class dev.frust.photolibrary.FrustPhotoLibraryInitProvider { *; }
