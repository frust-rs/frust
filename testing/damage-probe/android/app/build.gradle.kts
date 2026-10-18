// NativeActivity host: `android:hasCode="false"` in the manifest, the native
// library name in its `android.app.lib_name` meta-data. Debug-signed release
// (a measurement artifact, never shipped).
plugins {
    id("com.android.application")
}

android {
    namespace = "dev.frust.damageprobe"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.frust.damageprobe"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1"
    }

    buildTypes {
        getByName("release") {
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = false
        }
    }
}
