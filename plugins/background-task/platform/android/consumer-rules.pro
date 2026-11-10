# frust-background-task Android module keep rules
#
# Shipped as `consumerProguardFiles` (see `build.gradle.kts`), so a consuming
# app inherits them automatically. The failures these prevent are
# release-only: a debug build never reproduces them.
#
# `FrustBackgroundTaskHost` is never referenced from a consuming app's Kotlin
# or Java. The Rust side loads it through the application classloader
# (`ClassLoader.loadClass`), calls its statics over JNI and implements its
# `nativeOnTimeout`. R8 sees no reference, so without this rule a minified
# build strips or renames it and every guard is refused.
-keep class dev.frust.backgroundtask.FrustBackgroundTaskHost { *; }

# The manifest-declared service. AGP keeps manifest components; this is
# belt-and-braces against a consuming app's own rule-set surprises.
-keep class dev.frust.backgroundtask.FrustBackgroundTaskService { *; }
