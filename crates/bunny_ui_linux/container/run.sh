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
# A SNAPSHOT of the repository is mounted at /work — an rsync copy
# under target/container-snapshot, so the container builds what the
# tree held when the run started and an edit on the host during the
# run cannot reach a build in flight. The cargo registry and the Linux
# target directory live in named volumes (`bunny-ui-cargo`,
# `bunny-ui-target`, mounted outside the repository): the host's own
# target/ is never touched, and a rebuilt image keeps both. The logs
# of a run land in target/container-snapshot/target/drive/. SCALE=2
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

snapshot=$repo/target/container-snapshot
mkdir -p "$snapshot"
rsync -a --delete --exclude .git --exclude target --exclude target-linux "$repo/" "$snapshot/"

# (the `+` form: an empty array is not "unbound" under set -u on old bash)
exec docker run --rm ${tty_flags[@]+"${tty_flags[@]}"} \
    -v "$snapshot":/work \
    -v bunny-ui-cargo:/root/.cargo/registry \
    -v bunny-ui-target:/cargo-target \
    -e CARGO_TARGET_DIR=/cargo-target \
    -e SCALE="${SCALE:-1}" \
    -e BUNNY_DRIVE_TIMEOUT="${BUNNY_DRIVE_TIMEOUT:-90}" \
    -w /work \
    "$image" \
    bash /work/crates/bunny_ui_linux/container/inside.sh "$mode" "$@"
