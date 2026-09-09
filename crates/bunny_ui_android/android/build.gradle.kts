// Packages one example's cdylib into an APK over a pure
// android.app.NativeActivity — zero Java, zero Kotlin sources. The
// cargo build and the copy of the shared object are run-emu.sh's;
// Gradle only wraps, signs and aligns.
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
