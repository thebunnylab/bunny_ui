#!/usr/bin/env bash
# Isolated native protocol witness; this does not qualify a human Orca workflow.
set -euo pipefail
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} ]]; then
  exec dbus-run-session -- bash "$0" "$@"
fi
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
export WAYLAND_DISPLAY=bunny-accessibility DISPLAY=:98 BUNNY_PRESENT=cpu
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
