#!/usr/bin/env bash
# An example of the iOS shell on the iOS Simulator: build it for
# aarch64-apple-ios-sim, assemble the unsigned .app (the simulator
# installs those), install and launch. No Xcode project — a real device
# needs signing, which is a lane of its own.
#
#   crates/bunny_ui_ios/simulator/run-sim.sh counter_window_ios
#   SIM_DEVICE="iPad Pro 11-inch (M5)" crates/bunny_ui_ios/simulator/run-sim.sh touch_window_ios
#   SIMCTL_CHILD_BUNNY_PRESENT_TRACE=1 …   # the present tape rides into the app
set -euo pipefail
cd "$(dirname "$0")/../../.."
REPO_ROOT="$PWD"

NAME="${1:?usage: run-sim.sh <example name, e.g. counter_window_ios>}"
TARGET=aarch64-apple-ios-sim
# rustc's default deployment target is a decade old; the SDK's frameworks
# are not. Pin the floor the plist names.
export IPHONEOS_DEPLOYMENT_TARGET=17.0

cargo build -p bunny-ui-ios --example "$NAME" --target "$TARGET"

BINARY="$REPO_ROOT/target/$TARGET/debug/examples/$NAME"
[[ -f "$BINARY" ]] || { echo "error: cargo produced no binary at $BINARY" >&2; exit 1; }

# the bundle: the binary under its OWN name (the linker's ad-hoc
# signature is named after it) and the plist with the name filled in
DASHED="${NAME//_/-}"
APP="$REPO_ROOT/target/ios/$NAME.app"
rm -rf "$APP" && mkdir -p "$APP"
cp "$BINARY" "$APP/"
sed -e "s/@NAME@/$NAME/g" -e "s/@DASHED@/$DASHED/g" \
    crates/bunny_ui_ios/simulator/Info.plist.in > "$APP/Info.plist"
BUNDLE_ID="com.bunnylab.$DASHED"

# a booted simulator, or the one asked for, or the first iPhone
UDID="$(xcrun simctl list devices booted | grep -oE '[0-9A-F-]{36}' | head -1 || true)"
if [[ -n "${SIM_DEVICE:-}" ]]; then
    UDID="$(xcrun simctl list devices available | grep -F "$SIM_DEVICE (" | grep -oE '[0-9A-F-]{36}' | head -1 || true)"
    [[ -n "$UDID" ]] || { echo "error: no simulator named '$SIM_DEVICE'" >&2; exit 1; }
fi
if [[ -z "$UDID" ]]; then
    UDID="$(xcrun simctl list devices available | grep -E 'iPhone' | grep -oE '[0-9A-F-]{36}' | head -1 || true)"
    [[ -n "$UDID" ]] || { echo "error: no iPhone simulator — create one in Xcode" >&2; exit 1; }
fi
xcrun simctl boot "$UDID" 2>/dev/null || true
open -a Simulator

# terminate BEFORE installing: installing over a running app leaves the
# simulator on the container it already had open, and the relaunch comes
# back on the previous binary — a build that reports success and shows
# the old screen
xcrun simctl terminate "$UDID" "$BUNDLE_ID" 2>/dev/null || true
xcrun simctl install "$UDID" "$APP"
echo "note: launching $BUNDLE_ID on $UDID"
if [[ "${SIM_CONSOLE:-1}" == "1" ]]; then
    RUST_BACKTRACE=1 xcrun simctl launch --console-pty "$UDID" "$BUNDLE_ID"
else
    RUST_BACKTRACE=1 xcrun simctl launch "$UDID" "$BUNDLE_ID"
fi
