# frust-secure-storage biometric helper (looked up via JNI/classloader)
#
# `dev.frust.securestorage.FrustBiometric` is never referenced from Kotlin or
# Java. The Rust backend resolves it at runtime through the application
# classloader (`ClassLoader.loadClass`) and calls `authenticate` over JNI, so
# R8 cannot see that it is live: without this rule a minified release build
# strips or renames it and every gated `get`/`set` fails with
# `NotAvailable(HelperMissing)` — a release-only failure a debug build never
# reproduces.
#
# This ships as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits the rule automatically — its own `proguard-rules.pro` stays free
# of plugin rules, and the rule cannot outlive the plugin.
-keep class dev.frust.securestorage.FrustBiometric { *; }
