#!/usr/bin/env bash
# The Linux shell's proving ground, from any host with Docker.
#
#   container/run.sh            build the image if needed, run everything
#   container/run.sh build      (re)build the image only
#   container/run.sh test       the crate tests: core (+codec), linux, vulkan
#   container/run.sh drive X    one `--drive` example across the matrix
#   container/run.sh shell      a shell inside, both displays up
#   container/run.sh exec CMD   one command inside, both displays up
#
# The repository is bind-mounted at /work. The cargo registry and the
# Linux target directory live in named volumes (`bunny-ui-cargo`,
# `bunny-ui-target`, mounted outside the repository): the host's own
# target/ is never touched, and a rebuilt image keeps both. SCALE=2
# gives the headless output an integer scale; everything else is
# decided inside (see inside.sh).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)
image=bunny-ui-linux:trixie
mode=${1:-all}
shift || true

if [[ $mode == build ]] || ! docker image inspect "$image" >/dev/null 2>&1; then
    docker build -t "$image" "$here"
    [[ $mode == build ]] && exit 0
fi

tty_flags=()
if [[ -t 0 && -t 1 ]]; then
    tty_flags=(-it)
fi

# (the `+` form: an empty array is not "unbound" under set -u on old bash)
exec docker run --rm ${tty_flags[@]+"${tty_flags[@]}"} \
    -v "$repo":/work \
    -v bunny-ui-cargo:/root/.cargo/registry \
    -v bunny-ui-target:/cargo-target \
    -e CARGO_TARGET_DIR=/cargo-target \
    -e SCALE="${SCALE:-1}" \
    -e BUNNY_DRIVE_TIMEOUT="${BUNNY_DRIVE_TIMEOUT:-90}" \
    -w /work \
    "$image" \
    bash /work/crates/bunny_ui_linux/container/inside.sh "$mode" "$@"
