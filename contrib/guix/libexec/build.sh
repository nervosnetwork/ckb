#!/usr/bin/env bash
# Derived from Bitcoin Core's Guix container build script.
# Reference: https://github.com/bitcoin/bitcoin/blob/master/contrib/guix/libexec/build.sh
#
# Adapted for Rust/Cargo builds with a custom linker wrapper (rust-linker.sh)
# and cross-compiled OpenSSL from the Guix manifest.
export LC_ALL=C
set -euo pipefail

export TZ=UTC
umask 0022
export TAR_OPTIONS="--owner=0 --group=0 --numeric-owner --mtime=@${SOURCE_DATE_EPOCH:?not set} --sort=name"

RUST_TARGET="${RUST_TARGET:?not set}"

cat <<EOF
Required environment variables inside container:
  HOST=${HOST:?not set}
  RUST_TARGET=${RUST_TARGET}
  VERSION=${VERSION:?not set}
  JOBS=${JOBS:?not set}
  SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:?not set}
  DISTSRC=${DISTSRC:?not set}
  OUTDIR=${OUTDIR:?not set}
  DIST_ARCHIVE_BASE=${DIST_ARCHIVE_BASE:?not set}
EOF

mkdir -p "$DIST_ARCHIVE_BASE" "$OUTDIR"

DISTNAME="ckb_${VERSION}_${RUST_TARGET}"
GIT_ARCHIVE="${DIST_ARCHIVE_BASE}/ckb-${VERSION}-src.tar.gz"
STAGING_BASE="${DISTSRC}/staging"
INSTALLPATH="${STAGING_BASE}/${DISTNAME}"
TARGET_DIR="${DISTSRC}/target"

mkdir -p "$TARGET_DIR" "$STAGING_BASE"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runtime_linker="${DISTSRC}/rust-linker.sh"
sed "1s|^#!.*$|#!$(command -v bash)|" "${script_dir}/rust-linker.sh" > "${runtime_linker}"
chmod +x "${runtime_linker}"

export HOME="${DISTSRC}/home"
mkdir -p "$HOME"

export CARGO_TARGET_DIR="$TARGET_DIR"
case "$HOST" in
    *linux*)
        # Use the OpenSSL from the Guix profile, cross-compiled against
        # glibc-2.31 in manifest.scm.
        export OPENSSL_NO_VENDOR=1
        ;;
    *mingw*)
        # Let openssl-sys vendor and build OpenSSL from source using our
        # cross-gcc.  There is no system OpenSSL on Windows.
        export OPENSSL_STATIC=1
        # Jemalloc: set page size so configure doesn't need to run a test
        # program (which would be a Windows .exe that can't execute on Linux).
        export JEMALLOC_SYS_WITH_LG_PAGE=12  # 4096 bytes = 2^12
        # Jemalloc for MinGW produces jemalloc.lib instead of libjemalloc.a
        # (LIBPREFIX is empty, A=lib on Windows).  After cargo builds jemalloc,
        # we need to symlink libjemalloc.a -> jemalloc_s.lib so Rust can find it.
        # Use JEMALLOC_SYS_WITH_LG_QUANTUM to ensure consistent alignment.
        export JEMALLOC_SYS_WITH_LG_QUANTUM=3
        ;;
    *darwin*)
        # Vendor OpenSSL from source — no system OpenSSL in the Apple SDK.
        export OPENSSL_STATIC=1
        # Deterministic archives.
        export ZERO_AR_DATE=1
        ;;
esac
# NOTE: We do NOT set GUIX_LD_WRAPPER_DISABLE_RPATH=yes globally.
# Intermediate programs (autoconf test binaries, build scripts) need Guix's
# automatic rpath so they can actually execute inside the container.
# The final CKB binary gets GUIX_LD_WRAPPER_DISABLE_RPATH=yes via
# rust-linker.sh, which prevents /gnu/store paths from leaking into
# the release binary.

store_path() {
    grep --extended-regexp "/[^-]{32}-${1}-[^-]+${2:+-${2}}" "${GUIX_ENVIRONMENT}/manifest" \
        | head --lines=1 \
        | sed --expression='s|\x29*$||' \
              --expression='s|^[[:space:]]*"||' \
              --expression='s|"[[:space:]]*$||'
}

case "$HOST" in
    x86_64-linux-gnu)
        GNU_HOST="x86_64-linux-gnu"
        DYNAMIC_LINKER="/lib64/ld-linux-x86-64.so.2"
        TARGET_ENV_SUFFIX="X86_64_UNKNOWN_LINUX_GNU"
        ;;
    x86_64-w64-mingw32)
        GNU_HOST="x86_64-w64-mingw32"
        DYNAMIC_LINKER=""
        TARGET_ENV_SUFFIX="X86_64_PC_WINDOWS_GNU"
        ;;
    aarch64-apple-darwin)
        GNU_HOST="aarch64-apple-darwin"
        DYNAMIC_LINKER=""
        TARGET_ENV_SUFFIX="AARCH64_APPLE_DARWIN"
        ;;
    *)
        echo "ERR: Unsupported HOST '$HOST'" >&2
        exit 1
        ;;
esac

# Darwin uses Clang/LLVM, not GCC cross-toolchain.
case "$HOST" in
    *darwin*)
        CROSS_GCC=""
        CROSS_GCC_LIB_STORE=""
        ;;
    *)
        CROSS_GCC="$(store_path "gcc-cross-${GNU_HOST}")"
        CROSS_GCC_LIB_STORE="$(store_path "gcc-cross-${GNU_HOST}" lib)"
        ;;
esac

case "$HOST" in
    *linux*)
        CROSS_GLIBC="$(store_path "glibc-cross-${GNU_HOST}")"
        CROSS_GLIBC_STATIC="$(store_path "glibc-cross-${GNU_HOST}" static)"
        CROSS_KERNEL="$(store_path "linux-libre-headers-cross-${GNU_HOST}")"
        if [[ -z "$CROSS_GLIBC" || -z "$CROSS_KERNEL" ]]; then
            echo "ERR: Missing cross-glibc/kernel-headers for ${GNU_HOST}" >&2
            exit 1
        fi
        ;;
    *mingw*)
        CROSS_GLIBC="$(store_path "mingw-w64-x86_64-winpthreads")"
        CROSS_GLIBC_STATIC=""
        CROSS_KERNEL=""
        if [[ -z "$CROSS_GLIBC" ]]; then
            echo "ERR: Missing mingw-w64-winpthreads for ${GNU_HOST}" >&2
            exit 1
        fi
        ;;
    *darwin*)
        # Darwin uses the Apple SDK as sysroot, not glibc.
        CROSS_GLIBC=""
        CROSS_GLIBC_STATIC=""
        CROSS_KERNEL=""
        if [[ -z "${OSX_SDK:-}" || ! -d "${OSX_SDK}" ]]; then
            echo "ERR: OSX_SDK not set or not found at '${OSX_SDK:-}'" >&2
            exit 1
        fi
        ;;
esac

case "$HOST" in
    *darwin*)
        # Darwin uses Clang/LLVM, no GCC cross-toolchain needed.
        CROSS_GCC_LIB=""
        ;;
    *)
        if [[ -z "$CROSS_GCC" || -z "$CROSS_GCC_LIB_STORE" ]]; then
            echo "ERR: Missing cross-gcc for ${GNU_HOST}" >&2
            exit 1
        fi
        CROSS_GCC_LIBS=( "${CROSS_GCC_LIB_STORE}/lib/gcc/${GNU_HOST}"/* )
        CROSS_GCC_LIB="${CROSS_GCC_LIBS[0]}"
        ;;
esac

# Resolve the native gcc-toolchain library paths for host (build-script) linking.
NATIVE_GCC="$(store_path gcc-toolchain)"
NATIVE_GCC_STATIC="$(store_path gcc-toolchain static)"
case "$HOST" in
    *darwin*)
        # Darwin needs LIBRARY_PATH for zlib during x.py build (host stage1),
        # but it must NOT be set globally — Linux libc.so would leak into the
        # Mach-O linker.  Save it for use only during x.py invocation.
        DARWIN_HOST_LIBRARY_PATH="${NATIVE_GCC}/lib:${NATIVE_GCC_STATIC}/lib${LIBRARY_PATH:+:${LIBRARY_PATH}}"
        unset LIBRARY_PATH
        # Set SDKROOT so rustc/lld can find the Apple SDK.
        export SDKROOT="${OSX_SDK}"
        ;;
    *)
        unset LIBRARY_PATH
        ;;
esac
export CKB_RUST_HOST_LIBRARY_PATH="${NATIVE_GCC}/lib:${NATIVE_GCC_STATIC}/lib"

# Create thin CC/CXX wrappers.  For Linux targets, inject -Wl,-rpath so
# autoconf test programs can execute inside the container.  For Windows
# targets, no rpath is needed (PE format doesn't use it).  For Darwin,
# use Clang with -isysroot pointing to the Apple SDK.
case "$HOST" in
    *darwin*)
        # Use clang from clang-toolchain-20 (not the profile's default
        # clang-13 which can't parse the SDK's libc++ 20 headers).
        CLANG_TOOLCHAIN="$(store_path clang-toolchain)"
        REAL_CC="${CLANG_TOOLCHAIN}/bin/clang"
        REAL_CXX="${CLANG_TOOLCHAIN}/bin/clang++"
        if [[ ! -x "$REAL_CC" ]]; then
            echo "ERR: clang not found at '${REAL_CC}'" >&2
            exit 1
        fi
        ;;
    *)
        REAL_CC="$(command -v "${GNU_HOST}-gcc")"
        REAL_CXX="$(command -v "${GNU_HOST}-g++")"
        ;;
esac

BASH_PATH="$(command -v bash)"
CC_WRAPPER="${DISTSRC}/cross-cc"
CXX_WRAPPER="${DISTSRC}/cross-cxx"

case "$HOST" in
    *linux*)
        cat > "${CC_WRAPPER}" << CCEOF
#!${BASH_PATH}
for arg in "\$@"; do
    case "\$arg" in -c|-E|-S) exec "${REAL_CC}" "\$@" ;; esac
done
exec "${REAL_CC}" "\$@" -static-libgcc -Wl,-rpath="${CROSS_GLIBC}/lib"
CCEOF
        cat > "${CXX_WRAPPER}" << CXXEOF
#!${BASH_PATH}
for arg in "\$@"; do
    case "\$arg" in -c|-E|-S) exec "${REAL_CXX}" "\$@" ;; esac
done
exec "${REAL_CXX}" "\$@" -static-libgcc -static-libstdc++ -Wl,-rpath="${CROSS_GLIBC}/lib"
CXXEOF
        ;;
    *mingw*)
        # Windows: no rpath, just use the cross-gcc directly.
        cat > "${CC_WRAPPER}" << CCEOF
#!${BASH_PATH}
exec "${REAL_CC}" "\$@"
CCEOF
        cat > "${CXX_WRAPPER}" << CXXEOF
#!${BASH_PATH}
exec "${REAL_CXX}" "\$@"
CXXEOF
        ;;
    *darwin*)
        # macOS: Clang with Apple SDK sysroot, LLD linker, no ad-hoc codesign.
        # Reference: https://github.com/bitcoin/bitcoin/blob/master/depends/hosts/darwin.mk
        OSX_MIN_VERSION="14.0"
        OSX_SDK_VERSION="14.0"
        LLD_VERSION="711"
        DARWIN_TARGET="${RUST_TARGET}"
        cat > "${CC_WRAPPER}" << CCEOF
#!${BASH_PATH}
# Clear LIBRARY_PATH to prevent Linux libc.so from leaking into lld.
unset LIBRARY_PATH
unset C_INCLUDE_PATH CPLUS_INCLUDE_PATH
exec "${REAL_CC}" --target=${DARWIN_TARGET} \
    --sysroot="${OSX_SDK}" \
    -nostdlibinc \
    -iwithsysroot/usr/include \
    -iframeworkwithsysroot/System/Library/Frameworks \
    -mmacos-version-min=${OSX_MIN_VERSION} \
    -mlinker-version=${LLD_VERSION} \
    -fuse-ld=lld \
    "\$@"
CCEOF
        cat > "${CXX_WRAPPER}" << CXXEOF
#!${BASH_PATH}
unset LIBRARY_PATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH
exec "${REAL_CXX}" --target=${DARWIN_TARGET} \
    --sysroot="${OSX_SDK}" \
    -nostdlibinc \
    -iwithsysroot/usr/include/c++/v1 \
    -iwithsysroot/usr/include \
    -iframeworkwithsysroot/System/Library/Frameworks \
    -mmacos-version-min=${OSX_MIN_VERSION} \
    -mlinker-version=${LLD_VERSION} \
    -fuse-ld=lld \
    "\$@"
CXXEOF
        ;;
esac
chmod +x "${CC_WRAPPER}" "${CXX_WRAPPER}"

case "$HOST" in
    *darwin*)
        # For darwin, do NOT set CC/CXX globally — it would make the cc crate
        # use the darwin Clang for host build scripts too.  Only set the
        # target-specific CC.
        export AR="$(command -v llvm-ar)"
        export RANLIB="$(command -v llvm-ranlib)"
        export NM="$(command -v llvm-nm)"
        export STRIP="$(command -v llvm-strip)"
        ;;
    *)
        export CC="${CC_WRAPPER}"
        export CXX="${CXX_WRAPPER}"
        export AR="${GNU_HOST}-gcc-ar"
        export RANLIB="${GNU_HOST}-gcc-ranlib"
        export NM="${GNU_HOST}-gcc-nm"
        export STRIP="${GNU_HOST}-strip"
        ;;
esac

# Export CC/CXX for Cargo's cc crate, keyed by the RUST target triple.
export "CC_${RUST_TARGET//-/_}=${CC:-${CC_WRAPPER}}"
export "CXX_${RUST_TARGET//-/_}=${CXX:-${CXX_WRAPPER}}"
export "AR_${RUST_TARGET//-/_}=${AR}"
export "RANLIB_${RUST_TARGET//-/_}=${RANLIB}"

# Set host CC for build scripts — the `cc` crate uses CC_<host> for build
# scripts' C dependencies.  Without this, build scripts inherit the cross-CC
# and produce Windows objects that can't run on the Linux build machine.
# The wrapper clears C_INCLUDE_PATH to prevent MinGW headers in the Guix
# profile from contaminating host compilation.
# Set host CC for build scripts when cross-compiling to non-Linux targets.
# The Guix profile's merged include/ may contain target-specific headers
# (e.g., MinGW's corecrt.h) that conflict with native glibc headers.
# We create wrappers that isolate the host compiler from these.
case "$HOST" in
    *mingw*)
        NATIVE_KERNEL="$(store_path linux-libre-headers || true)"
        HOST_CC_WRAPPER="${DISTSRC}/host-cc"
        HOST_CXX_WRAPPER="${DISTSRC}/host-cxx"
        cat > "${HOST_CC_WRAPPER}" << HOSTCCEOF
#!${BASH_PATH}
export C_INCLUDE_PATH="${NATIVE_KERNEL:+${NATIVE_KERNEL}/include}"
export CPLUS_INCLUDE_PATH="${NATIVE_KERNEL:+${NATIVE_KERNEL}/include}"
export LIBRARY_PATH="${NATIVE_GCC}/lib:${NATIVE_GCC_STATIC}/lib"
exec "${NATIVE_GCC}/bin/gcc" "\$@"
HOSTCCEOF
        cat > "${HOST_CXX_WRAPPER}" << HOSTCXXEOF
#!${BASH_PATH}
export C_INCLUDE_PATH="${NATIVE_KERNEL:+${NATIVE_KERNEL}/include}"
export CPLUS_INCLUDE_PATH="${NATIVE_KERNEL:+${NATIVE_KERNEL}/include}"
export LIBRARY_PATH="${NATIVE_GCC}/lib:${NATIVE_GCC_STATIC}/lib"
exec "${NATIVE_GCC}/bin/g++" "\$@"
HOSTCXXEOF
        chmod +x "${HOST_CC_WRAPPER}" "${HOST_CXX_WRAPPER}"
        export CC_x86_64_unknown_linux_gnu="${HOST_CC_WRAPPER}"
        export CXX_x86_64_unknown_linux_gnu="${HOST_CXX_WRAPPER}"
        export AR_x86_64_unknown_linux_gnu="${NATIVE_GCC}/bin/gcc-ar"
        export RANLIB_x86_64_unknown_linux_gnu="${NATIVE_GCC}/bin/gcc-ranlib"
        ;;
    *darwin*)
        # Darwin CC wrapper uses Clang with Apple SDK — build scripts must
        # use native GCC instead to compile host C code (e.g., SQLite).
        # Also set HOST_CC for bindgen and set CFLAGS to avoid 32-bit stubs.
        export CC_x86_64_unknown_linux_gnu="$(command -v gcc)"
        export CXX_x86_64_unknown_linux_gnu="$(command -v g++)"
        export HOST_CC="$(command -v gcc)"
        export HOST_CXX="$(command -v g++)"
        # Create an empty gnu/stubs-32.h stub to satisfy the profile's glibc
        # stubs.h which conditionally includes it.  The 32-bit stubs are not
        # installed on 64-bit-only systems, and libclang can't find them when
        # targeting aarch64 (where __x86_64__ isn't defined).
        mkdir -p "${DISTSRC}/include-fixup/gnu"
        touch "${DISTSRC}/include-fixup/gnu/stubs-32.h"
        export C_INCLUDE_PATH="${DISTSRC}/include-fixup${C_INCLUDE_PATH:+:${C_INCLUDE_PATH}}"
        export AR_x86_64_unknown_linux_gnu="$(command -v gcc-ar)"
        export RANLIB_x86_64_unknown_linux_gnu="$(command -v gcc-ranlib)"
        ;;
esac

# Set cross-compilation search paths.
case "$HOST" in
    *linux*)
        export CROSS_C_INCLUDE_PATH="${CROSS_GCC_LIB}/include:${CROSS_GCC_LIB}/include-fixed:${CROSS_GLIBC}/include:${CROSS_KERNEL}/include"
        export CROSS_CPLUS_INCLUDE_PATH="${CROSS_GCC}/include/c++:${CROSS_GCC}/include/c++/${GNU_HOST}:${CROSS_GCC}/include/c++/backward:${CROSS_C_INCLUDE_PATH}"
        export CROSS_LIBRARY_PATH="${CROSS_GCC_LIB_STORE}/lib:${CROSS_GCC_LIB}:${CROSS_GLIBC}/lib:${CROSS_GLIBC_STATIC}/lib"
        ;;
    *mingw*)
        export CROSS_C_INCLUDE_PATH="${CROSS_GCC_LIB}/include:${CROSS_GCC_LIB}/include-fixed:${CROSS_GLIBC}/include"
        export CROSS_CPLUS_INCLUDE_PATH="${CROSS_GCC}/include/c++:${CROSS_GCC}/include/c++/${GNU_HOST}:${CROSS_GCC}/include/c++/backward:${CROSS_C_INCLUDE_PATH}"
        export CROSS_LIBRARY_PATH="${CROSS_GCC_LIB_STORE}/lib:${CROSS_GCC_LIB}:${CROSS_GLIBC}/lib"
        ;;
esac

# Validate cross-toolchain paths (not applicable for darwin — uses -isysroot).
if [[ -n "${CROSS_C_INCLUDE_PATH:-}" ]]; then
    IFS=':' read -r -a cross_paths <<< "${CROSS_C_INCLUDE_PATH}:${CROSS_CPLUS_INCLUDE_PATH}:${CROSS_LIBRARY_PATH}"
    for path in "${cross_paths[@]}"; do
        if [[ -n "$path" && ! -d "$path" ]]; then
            echo "ERR: Expected cross-toolchain path '$path' to exist" >&2
            exit 1
        fi
    done
fi

clang_root="$(dirname "$(dirname "$(command -v clang)")")"
export LIBCLANG_PATH="${clang_root}/lib"
export LLVM_CONFIG_PATH="$(command -v llvm-config)"

native_libgcc_dir="$(dirname "$(gcc -print-file-name=libgcc_s.so.1)")"
native_libstdcpp_dir="$(dirname "$(g++ -print-file-name=libstdc++.so.6)")"
export LD_LIBRARY_PATH="${native_libgcc_dir}:${native_libstdcpp_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"

export CKB_RUST_HOST_LINKER="$(command -v gcc)"
# For darwin, use the CC wrapper (which has --target and --sysroot) as linker.
# For Linux/Windows, use REAL_CC (the cross-gcc, without the rpath wrapper).
case "$HOST" in
    *darwin*) export CKB_RUST_TARGET_LINKER="${CC_WRAPPER}" ;;
    *)        export CKB_RUST_TARGET_LINKER="${REAL_CC}" ;;
esac
export CKB_RUST_TARGET_TRIPLE="${RUST_TARGET}"
export CKB_RUST_DYNAMIC_LINKER="${DYNAMIC_LINKER}"
export "CARGO_TARGET_${TARGET_ENV_SUFFIX}_LINKER=${runtime_linker}"

case "$HOST" in
    *darwin*)
        bindgen_clang_args=(
            "--target=${RUST_TARGET}"
            "--sysroot=${OSX_SDK}"
            "-iwithsysroot/usr/include"
        )
        ;;
    *)
        bindgen_clang_args=(
            "--target=${GNU_HOST}"
            "-isystem${CROSS_GCC_LIB}/include"
            "-isystem${CROSS_GCC_LIB}/include-fixed"
            "-isystem${CROSS_GLIBC}/include"
        )
        if [[ -n "$CROSS_KERNEL" ]]; then
            bindgen_clang_args+=("-isystem${CROSS_KERNEL}/include")
        fi
        bindgen_clang_args+=(
            "-isystem${CROSS_GCC}/include/c++"
            "-isystem${CROSS_GCC}/include/c++/${GNU_HOST}"
            "-isystem${CROSS_GCC}/include/c++/backward"
        )
        ;;
esac
case "$HOST" in
    *darwin*)
        # For darwin, only set the TARGET-specific bindgen args.
        # The global BINDGEN_EXTRA_CLANG_ARGS must NOT be set because build
        # scripts run bindgen on the HOST — darwin sysroot flags break HOST clang.
        export "BINDGEN_EXTRA_CLANG_ARGS_${RUST_TARGET//-/_}=${bindgen_clang_args[*]}"
        ;;
    *)
        export BINDGEN_EXTRA_CLANG_ARGS="${bindgen_clang_args[*]}"
        export "BINDGEN_EXTRA_CLANG_ARGS_${RUST_TARGET//-/_}=${bindgen_clang_args[*]}"
        ;;
esac

export CFLAGS="-O2 -g -ffile-prefix-map=/gnu/store=/usr -fdebug-prefix-map=${DISTSRC}=."
export CXXFLAGS="${CFLAGS}"

RUSTFLAGS="--remap-path-prefix=${DISTSRC}=. --remap-path-prefix=/gnu/store=/usr"

# For cross-compilation targets, set up the Rust sysroot.
case "$HOST" in
    *mingw*)
        # Windows: Guix's make-rust-sysroot built libstd; use the profile.
        RUSTFLAGS="${RUSTFLAGS} --sysroot=${GUIX_ENVIRONMENT}"
        ;;
    *darwin*)
        # macOS: Build library/std from source using Guix's bootstrapped rustc
        # and the Apple SDK.  Guix has no darwin platform definition so we
        # can't use make-rust-sysroot — do it manually.
        echo "Building Rust std library for ${RUST_TARGET} from source..."
        RUST_SRC_VERSION="$(rustc --version | awk '{print $2}')"
        RUST_SRC_DIR="${DISTSRC}/rustc-${RUST_SRC_VERSION}-src"
        RUST_SYSROOT_OUT="${DISTSRC}/rust-darwin-sysroot"

        # Find the Rust source from the rust-src package in the Guix profile.
        # It's hash-verified by Guix (same origin as rust-1.92's source).
        RUST_SRC_TARBALL="$(store_path rust-src)/rustc-src.tar.gz"
        if [[ ! -e "$RUST_SRC_TARBALL" ]]; then
            echo "ERR: Rust source not found at '${RUST_SRC_TARBALL}'" >&2
            echo "     The rust-src package should be in the manifest for darwin targets." >&2
            exit 1
        fi
        echo "  Using Rust source: ${RUST_SRC_TARBALL}"

        if [[ ! -d "${RUST_SYSROOT_OUT}/lib/rustlib/${RUST_TARGET}" ]]; then
            tar -C "${DISTSRC}" --no-same-owner -xf "$RUST_SRC_TARBALL"

            cd "${RUST_SRC_DIR}"

            # Unbundle xz to avoid macOS-specific __assert_rtn in vendored
            # lzma-sys.  This forces lzma-sys to use the system liblzma.
            # Same fix as Guix's make-rust-sysroot/implementation.
            for lzma_dir in vendor/lzma-sys-*; do
                rm -rf "$lzma_dir/xz-5.2"
                sed -i 's/!want_static && //' "$lzma_dir/build.rs"
                # Remove deleted files from cargo checksums and update build.rs hash.
                python3 -c "
import json, hashlib, os
cksum_file = '$lzma_dir/.cargo-checksum.json'
with open(cksum_file, 'r') as f:
    data = json.load(f)
# Remove entries for deleted xz-5.2/ files
data['files'] = {k: v for k, v in data['files'].items() if not k.startswith('xz-5.2/')}
# Update build.rs hash
with open('$lzma_dir/build.rs', 'rb') as f:
    data['files']['build.rs'] = hashlib.sha256(f.read()).hexdigest()
with open(cksum_file, 'w') as f:
    json.dump(data, f)
"
            done

            # Regenerate cargo checksums for all vendored crates.
            # Guix's origin snippet modifies some Cargo.toml files (e.g.,
            # tempfile adding "use-libc"), but x.py's --frozen checks original
            # checksums.  This updates them to match actual file contents.
            for cksum in vendor/*/.cargo-checksum.json; do
                crate_dir="$(dirname "$cksum")"
                python3 -c "
import json, hashlib, os
with open('$cksum', 'r') as f:
    data = json.load(f)
new_files = {}
for relpath in data.get('files', {}):
    fpath = os.path.join('$crate_dir', relpath)
    if os.path.exists(fpath):
        with open(fpath, 'rb') as f:
            new_files[relpath] = hashlib.sha256(f.read()).hexdigest()
data['files'] = new_files
with open('$cksum', 'w') as f:
    json.dump(data, f)
"
            done

            # Write config.toml for x.py
            cat > config.toml << XPYCONF
change-id = "ignore"

[llvm]
download-ci-llvm = false

[build]
cargo = "$(command -v cargo)"
rustc = "$(command -v rustc)"
docs = false
python = "$(command -v python3)"
vendor = true
submodules = false
# Use Rust-only compiler-builtins, not LLVM's compiler-rt (which was
# deleted by Guix's origin snippet to reduce source size).
optimized-compiler-builtins = false
target = ["${RUST_TARGET}"]

[install]
prefix = "${RUST_SYSROOT_OUT}"
sysconfdir = "etc"

[rust]
debug = false
jemalloc = false
default-linker = "${CC_WRAPPER}"
channel = "stable"
codegen-units = 1

[target.x86_64-unknown-linux-gnu]
# Native host tools
llvm-config = "$(command -v llvm-config)"
linker = "$(command -v gcc)"
cc = "$(command -v gcc)"
cxx = "$(command -v g++)"
ar = "$(command -v ar)"

[target.${RUST_TARGET}]
llvm-config = "$(command -v llvm-config)"
cc = "${CC_WRAPPER}"
cxx = "${CXX_WRAPPER}"
ar = "${AR}"
ranlib = "${RANLIB}"
linker = "${CC_WRAPPER}"
XPYCONF

            LIBRARY_PATH="${DARWIN_HOST_LIBRARY_PATH}" \
                python3 x.py build library/std --target "${RUST_TARGET}"
            LIBRARY_PATH="${DARWIN_HOST_LIBRARY_PATH}" \
                python3 x.py install library/std --target "${RUST_TARGET}"
        else
            echo "  Sysroot already built at ${RUST_SYSROOT_OUT}"
        fi

        # Copy host rustlib into the sysroot (needed for build scripts/proc-macros)
        HOST_RUSTLIB="$(dirname "$(dirname "$(command -v rustc)")")/lib/rustlib/x86_64-unknown-linux-gnu"
        if [[ -d "$HOST_RUSTLIB" && ! -d "${RUST_SYSROOT_OUT}/lib/rustlib/x86_64-unknown-linux-gnu" ]]; then
            cp -a "$HOST_RUSTLIB" "${RUST_SYSROOT_OUT}/lib/rustlib/"
        fi

        RUSTFLAGS="${RUSTFLAGS} --sysroot=${RUST_SYSROOT_OUT}"
        ;;
esac

export RUSTFLAGS

if [[ ! -e "$GIT_ARCHIVE" ]]; then
    echo "ERR: Expected pre-vendored source archive at '$GIT_ARCHIVE'" >&2
    exit 1
fi

rm -rf "$DISTSRC/src" "$INSTALLPATH"
mkdir -p "$DISTSRC/src" "$INSTALLPATH"

tar -C "$DISTSRC/src" --strip-components=1 --no-same-owner -xf "$GIT_ARCHIVE"
cd "$DISTSRC/src"

# Apply cross-compilation patches to vendored crate sources.
# After patching, update the .cargo-checksum.json so cargo accepts the change.
patch_vendored_crate() {
    local crate_dir="$1" file="$2" old="$3" new="$4"
    if [[ ! -f "$crate_dir/$file" ]]; then return; fi
    echo "Patching $crate_dir/$file..."
    if ! grep -qF "${old}" "$crate_dir/$file"; then
        echo "ERR: pattern not found in $crate_dir/$file — patch may be outdated" >&2
        exit 1
    fi
    sed -i "s|${old}|${new}|" "$crate_dir/$file"
    # Update the file's checksum in .cargo-checksum.json
    local new_hash
    new_hash="$(sha256sum "$crate_dir/$file" | cut -d' ' -f1)"
    python3 -c "
import json, sys
with open('$crate_dir/.cargo-checksum.json', 'r') as f:
    data = json.load(f)
data['files']['$file'] = '$new_hash'
with open('$crate_dir/.cargo-checksum.json', 'w') as f:
    json.dump(data, f)
"
}

# Apply cross-compilation patches to vendored crate sources.
case "$HOST" in
    *mingw*|*darwin*)
        # Fix ckb-librocksdb-sys: cfg!(target_os = "windows") checks the
        # build HOST, not the cross-compilation TARGET.  When cross-compiling,
        # build_detect_platform runs on the HOST (Linux) and sets Linux-specific
        # defines that conflict with the actual target (Windows or macOS).
        for rocksdb_dir in guix-vendor/*rocksdb-sys*; do
            patch_vendored_crate "$rocksdb_dir" "build.rs" \
                'if !cfg!(target_os = "windows") {' \
                'if env::var("TARGET").unwrap_or_default().contains("linux") {'
        done
        ;;
esac

case "$HOST" in
    *mingw*)
        # Exclude jemalloc and Linux-only memory tracking on Windows,
        # and fix check_msvc_version() cfg gate.
        echo "Applying ckb-disable-jemalloc-on-windows.patch..."
        patch -p1 --no-backup-if-mismatch < /ckb/contrib/guix/patches/ckb-disable-jemalloc-on-windows.patch
        ;;
esac

cargo_build_args=(
    --locked
    --offline
    --target "$RUST_TARGET"
    --bin ckb
    --profile prod
    --features "with_sentry,with_dns_seeding,portable"
    -j "$JOBS"
)

cargo build "${cargo_build_args[@]}"

# Determine binary name and path.
case "$HOST" in
    *mingw*)  BINARY_NAME="ckb.exe" ;;
    *)        BINARY_NAME="ckb" ;;
esac

binary="${CARGO_TARGET_DIR}/${RUST_TARGET}/prod/${BINARY_NAME}"
if [[ ! -f "$binary" ]]; then
    echo "ERR: Expected built binary at '$binary'" >&2
    exit 1
fi

mkdir -p "${INSTALLPATH}/docs"
cp "$binary" "${INSTALLPATH}/${BINARY_NAME}"

case "$HOST" in
    *linux*)
        patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath "${INSTALLPATH}/${BINARY_NAME}"

        interp="$(readelf -l "${INSTALLPATH}/${BINARY_NAME}" | sed -n 's@.*Requesting program interpreter: \(.*\)]@\1@p')"
        runpath="$(readelf -d "${INSTALLPATH}/${BINARY_NAME}" | sed -n 's@.*Library runpath: \[\(.*\)\]@\1@p')"

        if [[ "${interp}" == /gnu/store/* ]]; then
            echo "ERR: Built binary still references a Guix-store ELF interpreter: ${interp}" >&2
            exit 1
        fi
        if [[ "${runpath}" == *"/gnu/store/"* ]]; then
            echo "ERR: Built binary still references a Guix-store RUNPATH: ${runpath}" >&2
            exit 1
        fi

        python3 /ckb/contrib/guix/symbol-check.py "${INSTALLPATH}/${BINARY_NAME}"
        ;;
    *mingw*)
        # PE binaries: no patchelf, no ELF symbol check.
        ;;
    *darwin*)
        # Mach-O binaries: strip with llvm-strip, no patchelf.
        llvm-strip "${INSTALLPATH}/${BINARY_NAME}"
        ;;
esac

cp README.md CHANGELOG.md COPYING "${INSTALLPATH}/"
cp -R devtools/init "${INSTALLPATH}/"
cp -R docs "${INSTALLPATH}/"
cp rpc/README.md "${INSTALLPATH}/docs/rpc.md"

# Package the release archive.
( cd "$STAGING_BASE"
  case "$HOST" in
      *mingw*)
          # Windows: deterministic zip.
          find "${DISTNAME}" -print0 \
              | xargs -0r touch --no-dereference --date="@${SOURCE_DATE_EPOCH}"
          find "${DISTNAME}" \
              | sort \
              | zip -X@ "${OUTDIR}/${DISTNAME}.zip" \
              || ( rm -f "${OUTDIR}/${DISTNAME}.zip" && exit 1 )
          ;;
      *)
          # Linux: deterministic tar.gz.
          find "${DISTNAME}" -print0 \
              | sort --zero-terminated \
              | tar --create --no-recursion --mode='u+rw,go+r-w,a+X' --null --files-from=- \
              | gzip -9n > "${OUTDIR}/${DISTNAME}.tar.gz"
          ;;
  esac
)

( cd "$OUTDIR"
  find . -maxdepth 1 \( -name '*.tar.gz' -o -name '*.zip' \) -exec sha256sum {} + > SHA256SUMS
)

echo "Built release archive:"
find "${OUTDIR}" -maxdepth 1 \( -name '*.tar.gz' -o -name '*.zip' \) | while read -r f; do
    echo "  $f"
done
