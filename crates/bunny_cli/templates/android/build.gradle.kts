// The app's Android project: a pure android.app.NativeActivity around
// the app's shared object — no Java, no Kotlin. `bunny` builds the
// shared object with cargo, writes local.properties with the app's names
// and paths, and asks Gradle only to package, sign and align.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.1.0")
    }
}

tasks.register("clean", Delete::class) {
    delete(rootProject.layout.buildDirectory)
}
