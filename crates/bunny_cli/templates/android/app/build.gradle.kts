// `java` alone names the project's Java extension in a Gradle script,
// not the JDK's package: the class is imported by its full name
import java.util.Properties

plugins {
    id("com.android.application")
}

// Everything that names the app comes from local.properties, which
// `bunny run -d android` and `bunny build android` write on every build
// from Cargo.toml's [package.metadata.bunny]: this file stays the same
// for every app, and yours to change.
val bunnyProperties = Properties().apply {
    val file = rootProject.file("local.properties")
    if (file.exists()) file.inputStream().use { load(it) }
}

fun bunny(key: String): String =
    bunnyProperties.getProperty("bunny.$key")
        ?: throw GradleException(
            "local.properties has no bunny.$key: build with `bunny run -d android` or `bunny build android`"
        )

android {
    namespace = bunny("applicationId")
    compileSdk = 36

    defaultConfig {
        applicationId = bunny("applicationId")
        // the framework's floor: WindowInsets.getInsets and AImageDecoder
        minSdk = 30
        targetSdk = 36
        versionCode = bunny("versionCode").toInt()
        versionName = bunny("versionName")
        ndk {
            abiFilters += bunny("abis").split(",")
        }
        manifestPlaceholders["bunnyLibName"] = bunny("libName")
        manifestPlaceholders["bunnyAppLabel"] = bunny("appLabel")
    }

    sourceSets {
        getByName("main") {
            // the shared objects cargo built, one folder per ABI, under
            // the project's target/
            jniLibs.directories.clear()
            jniLibs.directories.add(bunny("jniLibsDir"))
        }
    }

    packaging {
        jniLibs {
            // `bunny` strips the shared objects itself and keeps their
            // symbols apart; Gradle packs the bytes as they are
            keepDebugSymbols += "**/*.so"
        }
    }
}
