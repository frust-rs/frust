# frust-connectivity Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically. Every failure these prevent is
# release-only: a debug build never reproduces them.
#
# `dev.frust.connectivity.FrustConnectivityHost` is never referenced from a
# consuming app's Kotlin or Java. The Rust side loads it through the
# application classloader (`ClassLoader.loadClass`) and calls its `start`
# static over JNI, and the JVM binds its `external` method by the mangled
# `Java_dev_frust_connectivity_FrustConnectivityHost_nativeOnConnectivityEvent`
# name. R8 sees neither, so without this rule a minified build strips or
# renames the class and the monitor never starts.
-keep class dev.frust.connectivity.FrustConnectivityHost { *; }
-keep class dev.frust.connectivity.FrustConnectivityHost$* { *; }

# The manifest-declared init provider (see `src/main/AndroidManifest.xml`).
# AGP already keeps manifest components; this is belt-and-braces against a
# consuming app's own rule-set surprises — the provider is how the host gets
# an application `Context` before any Activity exists.
-keep class dev.frust.connectivity.FrustConnectivityInitProvider { *; }
