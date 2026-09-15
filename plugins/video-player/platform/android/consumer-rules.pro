# frust-video-player Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically — its own `proguard-rules.pro` stays free of
# plugin rules, and the rules cannot outlive the plugin. Every failure these
# prevent is release-only: a debug build never reproduces them.
#
# `dev.frust.videoplayer.FrustVideoPlayerHost` is never referenced from Kotlin
# or Java by a consuming app. The Rust backend resolves it at runtime through
# the application classloader (`ClassLoader.loadClass`) and calls its static
# methods over JNI, and the JVM resolves its `external` (native) declarations by
# their mangled `Java_dev_frust_videoplayer_FrustVideoPlayerHost_native*` names
# — R8 can see neither, so without this rule a minified build strips or renames
# the class and every video call fails.
-keep class dev.frust.videoplayer.FrustVideoPlayerHost { *; }
-keep class dev.frust.videoplayer.FrustVideoPlayerHost$* { *; }

# `dev.frust.videoplayer.VideoPlayerViewFactory` is instantiated reflectively by
# the embedding's `FrustViewHost` from the `viewType` string
# `PlayerSession::view_type()` returns — a plain FQCN R8 cannot trace. The
# public no-arg constructor is part of the `FrustPlatformViewFactory` contract,
# so the whole class is kept.
-keep class dev.frust.videoplayer.VideoPlayerViewFactory { *; }

# `dev.frust.videoplayer.FrustVideoPlayerInitProvider` is a manifest-declared
# `ContentProvider` (see `src/main/AndroidManifest.xml`). AGP already generates
# keep rules for manifest components, so this is belt-and-braces against a
# consuming app's own manifest-merger or rule-set surprises — the provider is
# how the host obtains an application `Context` before any view exists.
-keep class dev.frust.videoplayer.FrustVideoPlayerInitProvider { *; }
