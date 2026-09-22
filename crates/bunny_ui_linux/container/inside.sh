#!/usr/bin/env bash
# Runs INSIDE the image (see run.sh). Raises the two displays — a
# headless Weston for the Wayland door and an Xvfb for the X11 door —
# under a session bus, points Mesa at its software renderers, and
# then runs what the mode asks: the tests, one example across the
# matrix, everything, or a shell.
#
# The matrix: every `--drive` example × {wayland, x11} × {cpu, gl, vk}.
# A run passes when the example exits 0 before BUNNY_DRIVE_TIMEOUT
# seconds; the table at the end says which did not, and the script's
# exit code is the number of failures.
set -uo pipefail

mode=${1:-all}
shift || true

# one session bus for the whole run: the life tests and the portal
# reads answer honestly instead of skipping
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} ]]; then
    exec dbus-run-session -- bash "$0" "$mode" "$@"
fi

export XDG_RUNTIME_DIR=/tmp/xdg
mkdir -p -m 700 "$XDG_RUNTIME_DIR"
export LIBGL_ALWAYS_SOFTWARE=1
export VK_ICD_FILENAMES
VK_ICD_FILENAMES=$(ls /usr/share/vulkan/icd.d/lvp_icd*.json 2>/dev/null | head -1)
export WAYLAND_DISPLAY=bunny
export DISPLAY=:99
export CARGO_TERM_COLOR=never

weston --backend=headless --socket=bunny --width=1280 --height=800 \
    --scale="${SCALE:-1}" --renderer=pixman --idle-time=0 \
    --log=/tmp/weston.log >/dev/null 2>&1 &
weston_pid=$!
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
xvfb_pid=$!
for _ in $(seq 1 100); do
    if [[ -S $XDG_RUNTIME_DIR/bunny && -S /tmp/.X11-unix/X99 ]]; then
        break
    fi
    sleep 0.1
done
if [[ ! -S $XDG_RUNTIME_DIR/bunny ]]; then
    echo "inside: weston did not raise its socket" >&2
    cat /tmp/weston.log >&2
fi
if [[ ! -S /tmp/.X11-unix/X99 ]]; then
    echo "inside: Xvfb did not raise its socket" >&2
    cat /tmp/xvfb.log >&2
fi
trap 'kill $weston_pid $xvfb_pid 2>/dev/null' EXIT

target=${CARGO_TARGET_DIR:-/cargo-target}
examples_dir=$target/debug/examples

# the drive examples: one row per `[[example]]` whose file answers --drive
drive_examples=()
for file in /work/crates/bunny_ui_linux/examples/*.rs; do
    if grep -q -- '"--drive"' "$file"; then
        name=$(basename "$file" .rs)
        drive_examples+=("${name}_linux")
    fi
done

run_tests() {
    local status=0
    # the core with the features the Linux shell turns on; `codec` only
    # once the core declares it
    local core_features=canvas,gpu
    if grep -q '^codec' /work/crates/bunny_ui/Cargo.toml; then
        core_features=$core_features,codec
    fi
    cargo test --no-fail-fast -p bunny-ui --features "$core_features" || status=1
    cargo test --no-fail-fast -p bunny-ui-linux -p bunny-ui-vulkan || status=1
    return $status
}

build_examples() {
    cargo build -p bunny-ui-linux --examples
}

# run_one <example> <backend> <present> [args...] → prints one table row
rows=()
failures=0
run_one() {
    local example=$1 backend=$2 present=$3
    shift 3
    mkdir -p /work/target/drive
    local log="/work/target/drive/$example-$backend-$present.log"
    local env_present=()
    if [[ $present != vk ]]; then
        env_present=(BUNNY_PRESENT="$present")
    fi
    local started=$SECONDS
    # the tape is on for every run: the `P` lines say which tier
    # actually presented, which the table reports as `road`
    env BUNNY_BACKEND="$backend" BUNNY_FRAME_STATS=1 ${env_present[@]+"${env_present[@]}"} \
        timeout --signal=KILL "${BUNNY_DRIVE_TIMEOUT:-90}" \
        "$examples_dir/$example" --drive "$@" >"$log" 2>&1
    local code=$?
    local took=$((SECONDS - started))
    local presents road
    presents=$(grep -o 'presents=[0-9]*' "$log" | tail -1 | cut -d= -f2)
    road=$(grep -o '^P t=[0-9.]* road=[a-z]*' "$log" | tail -1 | sed 's/.*road=//')
    local result=ok
    if [[ $code -ne 0 ]]; then
        result="FAIL($code)"
        failures=$((failures + 1))
        echo "---- $example $backend $present: exit $code, last lines:" >&2
        tail -n 15 "$log" >&2
    fi
    rows+=("$(printf '%-28s %-8s %-4s %-4s %-9s %5ss %s' "$example" "$backend" "$present" "${road:--}" "$result" "$took" "${presents:-}")")
}

run_matrix() {
    local example=$1
    shift
    for backend in wayland x11; do
        for present in cpu gl vk; do
            run_one "$example" "$backend" "$present" "$@"
        done
    done
}

print_table() {
    echo
    printf '%-28s %-8s %-4s %-4s %-9s %6s %s\n' example backend tier road result took presents
    for row in "${rows[@]}"; do
        echo "$row"
    done
    echo "failures: $failures"
}

case $mode in
    test)
        run_tests
        exit $?
        ;;
    drive)
        example=$1
        shift
        build_examples || exit 1
        run_matrix "$example" "$@"
        print_table
        exit $failures
        ;;
    shell)
        echo "displays up: WAYLAND_DISPLAY=$WAYLAND_DISPLAY DISPLAY=$DISPLAY (scale ${SCALE:-1})"
        exec bash
        ;;
    exec)
        bash -c "$*"
        exit $?
        ;;
    all)
        status=0
        run_tests || status=1
        build_examples || exit 1
        for example in "${drive_examples[@]}"; do
            run_matrix "$example"
        done
        print_table
        exit $((status + failures))
        ;;
    *)
        echo "inside: unknown mode '$mode'" >&2
        exit 2
        ;;
esac
