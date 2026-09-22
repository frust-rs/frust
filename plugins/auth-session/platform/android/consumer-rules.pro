# frust-auth-session Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically — its own `proguard-rules.pro` stays free of
# plugin rules, and the rules cannot outlive the plugin. Every failure these
# prevent is release-only: a debug build never reproduces them.
#
# `dev.frust.authsession.FrustAuthSessionHost` is never referenced from Kotlin
# or Java by a consuming app. The Rust backend resolves it at runtime through
# the application classloader (`ClassLoader.loadClass`) and calls its
# `@JvmStatic` entry points (`start`) over JNI, and the JVM resolves its
# `external` (native) declaration by its mangled
# `Java_dev_frust_authsession_FrustAuthSessionHost_native*` name — R8 can see
# neither, so without this rule a minified build strips or renames the class
# and every session launch fails.
-keep class dev.frust.authsession.FrustAuthSessionHost { *; }
-keep class dev.frust.authsession.FrustAuthSessionHost$* { *; }

# `dev.frust.authsession.FrustAuthSessionInitProvider` is a manifest-declared
# `ContentProvider` (see `src/main/AndroidManifest.xml`). AGP already generates
# keep rules for manifest components, so this is belt-and-braces against a
# consuming app's own manifest-merger or rule-set surprises — the provider is
# how the host obtains an application `Context` (and registers its Activity
# lifecycle callbacks) before any Activity exists.
-keep class dev.frust.authsession.FrustAuthSessionInitProvider { *; }
