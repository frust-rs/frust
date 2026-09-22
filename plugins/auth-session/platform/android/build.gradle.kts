// `frust-auth-session`' Android side — a `com.android.library` module
// holding the plugin's Chrome Custom Tabs integration.
//
// It is wired into a generated app exactly like other plugin modules:
//
//     // android/settings.gradle.kts
//     include(":frust-auth-session")
//     project(":frust-auth-session").projectDir =
//         file("<frust checkout>/plugins/auth-session/platform/android")
//
//     // android/app/build.gradle.kts
//     implementation(project(":frust-auth-session"))
//
// plus a `gradle.lifecycle.beforeProject` build-directory redirect so this
// module's build output lands under the consuming app's tree.
//
// AGP 9's built-in Kotlin support means no separate
// `org.jetbrains.kotlin.android` plugin is applied.
plugins {
    id("com.android.library")
}

android {
    namespace = "dev.frust.authsession"

    // Both SDK levels mirror the app template's `app/build.gradle.kts`, the
    // `frust-embedding` module and other plugin modules exactly
    // (`compileSdk = 36`, `minSdk = 26`).
    compileSdk = 36

    defaultConfig {
        minSdk = 26

        // The R8 keep rules for any reflectively-instantiated classes travel
        // WITH this module: a consuming app inherits them automatically.
        consumerProguardFiles("consumer-rules.pro")
    }

    buildFeatures {
        buildConfig = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `FrustPlatformViewFactory` — the embedding-owned interface
    // any platform views in this module implement. `implementation`, not `api`.
    implementation(project(":frust-embedding"))
}
