#!/usr/bin/env bash
# Cargo linker wrapper for cross-compilation inside the Guix container.
#
# Rust/Cargo uses a single CARGO_TARGET_<triple>_LINKER for both build
# scripts (host) and the final binary (target).  This wrapper routes
# them to the correct linker with the correct flags.
#
# No direct Bitcoin Core equivalent — Bitcoin uses C++/CMake where host
# and target compilers are separate variables.
set -euo pipefail

if [[ -z "${CKB_RUST_HOST_LINKER:-}" || -z "${CKB_RUST_TARGET_LINKER:-}" || -z "${CKB_RUST_TARGET_TRIPLE:-}" ]]; then
    # CKB_RUST_DYNAMIC_LINKER is optional (empty for Windows/macOS).
    echo "ERR: Missing required CKB_RUST_* linker environment" >&2
    exit 1
fi

is_target_link=0
for arg in "$@"; do
    if [[ "$arg" == *"/target/${CKB_RUST_TARGET_TRIPLE}/"* ]]; then
        is_target_link=1
        break
    fi
done

if [[ "$is_target_link" -eq 1 ]]; then
    # Disable Guix's automatic rpath injection for the final target binary
    # so no /gnu/store paths leak into the release ELF.
    export GUIX_LD_WRAPPER_DISABLE_RPATH=yes

    case "${CKB_RUST_TARGET_TRIPLE}" in
        *linux*)
            exec "${CKB_RUST_TARGET_LINKER}" "$@" \
                -Wl,--as-needed \
                "-Wl,--dynamic-linker=${CKB_RUST_DYNAMIC_LINKER}" \
                -Wl,-O2 \
                -static-libstdc++ \
                -static-libgcc
            ;;
        *windows*)
            # Windows PE: no dynamic linker, no rpath.
            # -Wl,--no-insert-timestamp for deterministic PE headers.
            exec "${CKB_RUST_TARGET_LINKER}" "$@" \
                -Wl,--no-insert-timestamp \
                -static-libstdc++ \
                -static-libgcc
            ;;
        *darwin*)
            # macOS Mach-O: use LLD, no ad-hoc codesign, no UUID.
            # LC_UUID is non-deterministic even with --threads=1 due to
            # LLD's parallel hash computation.  Remove it entirely.
            exec "${CKB_RUST_TARGET_LINKER}" "$@" \
                -fuse-ld=lld \
                -Wl,-no_adhoc_codesign \
                -Wl,-no_uuid \
                -Wl,-platform_version,macos,14.0,14.0
            ;;
        *)
            exec "${CKB_RUST_TARGET_LINKER}" "$@" \
                -static-libstdc++ \
                -static-libgcc
            ;;
    esac
fi

# Host link (build scripts, proc-macros).  Set LIBRARY_PATH to the native
# gcc-toolchain so the linker finds the native glibc rather than the
# cross-glibc-2.31 that also sits in the Guix profile.
if [[ -n "${CKB_RUST_HOST_LIBRARY_PATH:-}" ]]; then
    export LIBRARY_PATH="${CKB_RUST_HOST_LIBRARY_PATH}${LIBRARY_PATH:+:${LIBRARY_PATH}}"
fi

exec "${CKB_RUST_HOST_LINKER}" "$@"
