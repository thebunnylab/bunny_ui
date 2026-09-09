#!/usr/bin/env bash
# An example of the Android shell on the emulator: build its cdylib for
# aarch64-linux-android with the NDK's own linker, wrap it in an APK
# over a pure NativeActivity (Gradle), install and launch, then follow
# the shell's lines in logcat.
#
#   crates/bunny_ui_android/android/run-emu.sh counter_window_android
#   AVD=flexlab-emu-35 crates/bunny_ui_android/android/run-emu.sh touch_window_android
#   EMU_SHOT=1 …        # a screenshot into target/android/<name>.png instead of logcat
#   PROFILE=release …   # the release build
#
# Switches ride as system properties, since an app has no environment:
#   adb shell setprop debug.bunny.trace 1      # every event, one line each
#   adb shell setprop debug.bunny.present cpu  # the CPU floor instead of Vulkan
set -euo pipefail
cd "$(dirname "$0")/../../.."
REPO_ROOT="$PWD"
LANE="$REPO_ROOT/crates/bunny_ui_android/android"

NAME="${1:?usage: run-emu.sh <example name, e.g. counter_window_android>}"
TARGET=aarch64-linux-android
PROFILE="${PROFILE:-debug}"

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
NDK="${ANDROID_NDK_HOME:-$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)}"
export ANDROID_NDK_HOME="$NDK"
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
ADB="$ANDROID_HOME/platform-tools/adb"
[[ -x "$TOOLCHAIN/aarch64-linux-android30-clang" ]] || { echo "error: no NDK clang at $TOOLCHAIN" >&2; exit 1; }

# cargo links with the NDK's clang for API 30; 16 KB pages are the
# platform's direction and cost nothing to honor
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$TOOLCHAIN/aarch64-linux-android30-clang"
export CC_aarch64_linux_android="$TOOLCHAIN/aarch64-linux-android30-clang"
export AR_aarch64_linux_android="$TOOLCHAIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"

if [[ "$PROFILE" == "release" ]]; then
    cargo build -p bunny-ui-android --example "$NAME" --target "$TARGET" --release
else
    cargo build -p bunny-ui-android --example "$NAME" --target "$TARGET"
fi

SO="$REPO_ROOT/target/$TARGET/$PROFILE/examples/lib$NAME.so"
[[ -f "$SO" ]] || { echo "error: cargo produced no shared object at $SO" >&2; exit 1; }
# the entry the activity looks for — an example without the macro
# would install fine and crash on launch
"$TOOLCHAIN/llvm-nm" -D --defined-only "$SO" | grep -q ' ANativeActivity_onCreate$' \
    || { echo "error: $NAME exports no ANativeActivity_onCreate — add bunny_ui_android::activity!(main)" >&2; exit 1; }

# the shared object goes where Gradle packs it from, in a directory of
# its own so a sibling's stale one never rides along
JNI="$LANE/app/src/main/jniLibs/$NAME/arm64-v8a"
rm -rf "$LANE/app/src/main/jniLibs/$NAME" && mkdir -p "$JNI"
cp "$SO" "$JNI/"
echo "sdk.dir=$ANDROID_HOME" > "$LANE/local.properties"

# the emulator, if none is up: the AVD with room on its data partition
AVD="${AVD:-flexlab-emu-35}"
if ! "$ADB" get-state >/dev/null 2>&1; then
    echo "== booting the $AVD AVD =="
    "$ANDROID_HOME/emulator/emulator" -avd "$AVD" -no-snapshot-save -no-audio -no-boot-anim >/dev/null 2>&1 &
fi
"$ADB" wait-for-device
until [[ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" == "1" ]]; do sleep 1; done

(cd "$LANE" && ./gradlew --console=plain -q -PbunnyExample="$NAME" assembleDebug)
APK="$LANE/app/build/outputs/apk/debug/app-debug.apk"
[[ -f "$APK" ]] || { echo "error: gradle produced no APK at $APK" >&2; exit 1; }
# the APK carries THIS build's shared object, byte for byte
unzip -p "$APK" "lib/arm64-v8a/lib$NAME.so" | cmp -s - "$SO" \
    || { echo "error: the APK's lib$NAME.so is not the one cargo built" >&2; exit 1; }

# install over a RUNNING app relaunches the old binary: stop it first
PKG="com.bunnylab.$NAME"
"$ADB" shell am force-stop "$PKG" >/dev/null 2>&1 || true
"$ADB" install -r "$APK" >/dev/null
"$ADB" logcat -c
"$ADB" shell am start -n "$PKG/android.app.NativeActivity" >/dev/null

if [[ -n "${EMU_SHOT:-}" ]]; then
    sleep 3
    mkdir -p "$REPO_ROOT/target/android"
    "$ADB" exec-out screencap -p > "$REPO_ROOT/target/android/$NAME.png"
    echo "target/android/$NAME.png"
    exit 0
fi
exec "$ADB" logcat -s bunny_ui:V
