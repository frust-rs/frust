# frust-native-widgets Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically — its own `proguard-rules.pro` stays free of
# plugin rules, and the rules cannot outlive the plugin. Every failure these
# prevent is release-only: a debug build never reproduces them.
#
# The `frust-embedding` module's own consumer rules already carry a broad
# `-keep class dev.frust.** { *; }`, which happens to cover this subpackage
# too. These rules are stated anyway, for the same reason the camera module
# states its own: this plugin's classes must survive on their own terms, not
# because a *different* module's rule happens to be broad enough today.
#
# `dev.frust.nativewidgets.FrustNativeControlFactory` is never referenced from
# Kotlin or Java by a consuming app. The embedding's `FrustViewHost`
# instantiates it reflectively from the `viewType` string the api layer
# publishes (`crate::api::builders`' Android `VIEW_TYPE`) — a plain FQCN R8
# cannot trace — and the JVM resolves its `external` (native) declarations by
# their mangled `Java_dev_frust_nativewidgets_FrustNativeControlFactory_native*`
# names, which R8 cannot see either. The public no-arg constructor is part of
# the `FrustPlatformViewFactory` contract, so the whole class is kept.
-keep class dev.frust.nativewidgets.FrustNativeControlFactory { *; }

# `dev.frust.nativewidgets.FrustNativeListener` is constructed from Rust over
# JNI (`new FrustNativeListener(slotId)`, resolved by binary class name through
# the application classloader — `crate::android::ctx`'s `LISTENER_CLASS`), and
# its `nativeOnEvent` binds by the mangled
# `Java_dev_frust_nativewidgets_FrustNativeListener_nativeOnEvent` symbol. Its
# listener overrides are called by the Android framework against interfaces it
# implements, but nothing references the class itself by compiled reference —
# without this rule a minified build strips or renames it and every native
# control's tap/toggle/drag stops firing.
-keep class dev.frust.nativewidgets.FrustNativeListener { *; }
