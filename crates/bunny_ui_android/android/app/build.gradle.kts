plugins {
    id("com.android.application")
}

// which example this APK carries — run-emu.sh passes it; every example
// is its own app, side by side on the device
val bunnyExample: String =
    (project.findProperty("bunnyExample") as String?) ?: "counter_window_android"

android {
    namespace = "com.bunnylab.bunnyui"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.bunnylab.$bunnyExample"
        minSdk = 30 // WindowInsets.getInsets and AImageDecoder
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
        manifestPlaceholders["nativeLibraryName"] = bunnyExample
        manifestPlaceholders["appLabel"] = bunnyExample
    }

    buildTypes {
        debug {
            isDebuggable = true
        }
    }

    sourceSets {
        getByName("main") {
            // one directory per example: a sibling's stale shared
            // object never rides along
            jniLibs.setSrcDirs(listOf("src/main/jniLibs/$bunnyExample"))
        }
    }
}
