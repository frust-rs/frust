# frust-camera Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically — its own `proguard-rules.pro` stays free of
# plugin rules, and the rules cannot outlive the plugin. Every failure these
# prevent is release-only: a debug build never reproduces them.
#
# `dev.frust.camera.FrustCameraHost` is never referenced from Kotlin or Java by
# a consuming app. The Rust backend resolves it at runtime through the
# application classloader (`ClassLoader.loadClass`) and calls its static methods
# over JNI, and the JVM resolves its `external` (native) declarations by their
# mangled `Java_dev_frust_camera_FrustCameraHost_native*` names — R8 can see
# neither, so without this rule a minified build strips or renames the class and
# every camera call fails.
-keep class dev.frust.camera.FrustCameraHost { *; }
-keep class dev.frust.camera.FrustCameraHost$* { *; }

# `dev.frust.camera.CameraPreviewFactory` is instantiated reflectively by the
# embedding's `FrustViewHost` from the `viewType` string
# `CameraSession::preview_view_type()` returns — a plain FQCN R8 cannot trace.
# The public no-arg constructor is part of the `FrustPlatformViewFactory`
# contract, so the whole class is kept.
-keep class dev.frust.camera.CameraPreviewFactory { *; }

# `dev.frust.camera.FrustCameraInitProvider` is a manifest-declared
# `ContentProvider` (see `src/main/AndroidManifest.xml`). AGP already generates
# keep rules for manifest components, so this is belt-and-braces against a
# consuming app's own manifest-merger or rule-set surprises — the provider is
# how the host obtains an application `Context` before any view exists.
-keep class dev.frust.camera.FrustCameraInitProvider { *; }
