# frust-share Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically. The failures these prevent are
# release-only: a debug build never reproduces them.
#
# `dev.frust.share.FrustShareHost` is never referenced from a consuming app's
# Kotlin or Java. The Rust side loads it through the application classloader
# (`ClassLoader.loadClass`) and calls its `shareFiles` static over JNI. R8 sees
# no reference, so without this rule a minified build strips or renames it and
# every file share fails.
-keep class dev.frust.share.FrustShareHost { *; }

# The manifest-declared FileProvider subclass. AGP keeps manifest components;
# this is belt-and-braces against a consuming app's own rule-set surprises.
-keep class dev.frust.share.FrustShareFileProvider { *; }
