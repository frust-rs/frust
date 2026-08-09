# frust-iap Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically — its own `proguard-rules.pro` stays free of
# plugin rules, and the rules cannot outlive the plugin. Every failure these
# prevent is release-only: a debug build never reproduces them.
#
# Validated against a real minified build by examples/playground/android's
# `:app:minifyReleaseWithR8` — see docs/PLUGINS_DEVELOPMENT.md's pin table
# (openiap-google row) and this plugin's README.md §6 for the tripwire.
#
# `dev.frust.iap.FrustIapHost` is never referenced from Kotlin or Java by a
# consuming app. The Rust backend resolves it at runtime through the application
# classloader (`ClassLoader.loadClass`) and calls its `call` static over JNI, and
# the JVM resolves its `external` (native) declarations by their mangled
# `Java_dev_frust_iap_FrustIapHost_native*` names — R8 can see neither, so
# without this rule a minified build strips or renames the class and every
# billing call fails.
-keep class dev.frust.iap.FrustIapHost { *; }
-keep class dev.frust.iap.FrustIapHost$* { *; }

# `dev.frust.iap.FrustIapInitProvider` is a manifest-declared `ContentProvider`
# (see `src/main/AndroidManifest.xml`). AGP already generates keep rules for
# manifest components, so this is belt-and-braces against a consuming app's own
# manifest-merger or rule-set surprises — the provider is how the host obtains an
# application `Context` (and registers its Activity lifecycle callbacks) before
# any Activity exists.
-keep class dev.frust.iap.FrustIapInitProvider { *; }
