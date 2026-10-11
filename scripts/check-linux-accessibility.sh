#!/usr/bin/env bash
# Isolated native protocol witness; this does not qualify a human Orca workflow.
set -euo pipefail
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} ]]; then
  exec dbus-run-session -- bash "$0" "$@"
fi
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
export WAYLAND_DISPLAY=bunny-accessibility DISPLAY=:98 BUNNY_PRESENT=cpu BUNNY_ATSPI_TRACE=1
weston --backend=headless-backend.so --socket="$WAYLAND_DISPLAY" --width=1280 --height=800 \
  --use-pixman --idle-time=0 --log="$XDG_RUNTIME_DIR/weston.log" &
weston_pid=$!
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp >"$XDG_RUNTIME_DIR/xvfb.log" 2>&1 &
xvfb_pid=$!
trap 'kill "$weston_pid" "$xvfb_pid" 2>/dev/null || true; cat "$XDG_RUNTIME_DIR/weston.log"' EXIT
for _ in $(seq 1 100); do
  if [[ -S $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY && -S /tmp/.X11-unix/X98 ]]; then break; fi
  sleep 0.1
done
test -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"
test -S /tmp/.X11-unix/X98
cargo test -p bunny-ui-linux --locked --test accessibility_native --no-run --message-format=json > "$XDG_RUNTIME_DIR/build.jsonl"
fixture=$(/usr/bin/python3 - "$XDG_RUNTIME_DIR/build.jsonl" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    item = json.loads(line)
    if item.get("target", {}).get("name") == "accessibility_native" and item.get("executable"):
        print(item["executable"])
PY
)
test -n "$fixture"
failures=0
for backend in x11 wayland; do
  for paranoid in '' all; do
    echo "Portal appearance backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_PORTAL_APPEARANCE_PROBE=1 timeout 45s \
      cargo test -p bunny-ui-linux --locked --test system_appearance; then
      failures=$((failures + 1))
    fi
    echo "Native confirmation backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" timeout 45s \
      cargo test -p bunny-ui-linux --locked --lib confirmation_native_protocol -- --ignored --nocapture; then
      failures=$((failures + 1))
    fi
    echo "Window lifecycle backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_WINDOW_LIFECYCLE_PROBE=1 timeout 100s \
      cargo test -p bunny-ui-linux --locked --test window_lifecycle; then
      failures=$((failures + 1))
    fi
    echo "Window relationships backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_WINDOW_RELATIONSHIPS_PROBE=1 timeout 100s \
      cargo test -p bunny-ui-linux --locked --test window_relationships; then
      failures=$((failures + 1))
    fi
    echo "Window geometry backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_WINDOW_GEOMETRY_PROBE=1 timeout 100s \
      cargo test -p bunny-ui-linux --locked --test window_geometry; then
      failures=$((failures + 1))
    fi
    echo "Dialog geometry backend=$backend paranoid=${paranoid:-off}"
    if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_DIALOG_GEOMETRY_PROBE=1 timeout 100s \
      cargo test -p bunny-ui-linux --locked --test dialog_geometry; then
      failures=$((failures + 1))
    fi
    if [[ $backend == x11 ]]; then
      if ! BUNNY_BACKEND=x11 BUNNY_PARANOID="$paranoid" timeout 45s \
        cargo test -p bunny-ui-linux --locked --lib relationship_property_round_trip -- --ignored --nocapture; then
        failures=$((failures + 1))
      fi
    else
      if ! BUNNY_BACKEND=wayland BUNNY_PARANOID="$paranoid" timeout 45s \
        cargo test -p bunny-ui-linux --locked --lib buffer_scale_reset_reaches_the_wayland_wire -- --ignored --nocapture; then
        failures=$((failures + 1))
      fi
    fi
    for modal_first in 0 1; do
      echo "AT-SPI backend=$backend paranoid=${paranoid:-off} modal-first=$modal_first"
      if ! BUNNY_BACKEND="$backend" BUNNY_PARANOID="$paranoid" BUNNY_PROBE_MODAL_FIRST="$modal_first" timeout 45s \
        /usr/bin/python3 crates/bunny_ui_linux/tests/accessibility_native.py "$fixture"; then
        failures=$((failures + 1))
      fi
    done
  done
done
exit "$failures"
