#!/usr/bin/env sh

set -ex

# How many cores to allocate to Guix building.
JOBS="${JOBS:-$(nproc)}"

# Execute "$@" in a pinned, possibly older version of Guix, for reproducibility
# across time.
time_machine() {
    guix time-machine --url=https://git.savannah.gnu.org/git/guix.git \
                      --commit= \
                      --cores="$JOBS" \
                      --keep-failed \
                      --fallback \
                      -- "$@"
}

time_machine shell --no-cwd \
               --cores="$JOBS" \
               --container \
               --pure \
               --fallback \
               --rebuild-cache \
               -m $PWD/guix/manifest.scm \
               -- env CC=gcc JOBS="$JOBS" \
                /bin/sh -c "cd /ckb && make prod"

set +ex

echo "Build successful. Output available at $OUT_DIR"
