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

cat <<EOF
Required environment variables inside container:
  HOST=${HOST:?not set}
  VERSION=${VERSION:?not set}
  JOBS=${JOBS:?not set}
  SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:?not set}
  DISTSRC=${DISTSRC:?not set}
  OUTDIR=${OUTDIR:?not set}
  DIST_ARCHIVE_BASE=${DIST_ARCHIVE_BASE:?not set}
EOF

mkdir -p "$DIST_ARCHIVE_BASE" "$OUTDIR"

DISTNAME="ckb_${VERSION}_${HOST}"
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
# Use the OpenSSL from the Guix profile, which was rebuilt against
# glibc-2.31 via package-with-c-toolchain in manifest.scm.
export OPENSSL_NO_VENDOR=1
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
    x86_64-unknown-linux-gnu)
        GNU_HOST="x86_64-linux-gnu"
        DYNAMIC_LINKER="/lib64/ld-linux-x86-64.so.2"
        TARGET_ENV_SUFFIX="X86_64_UNKNOWN_LINUX_GNU"
        ;;
    *)
        echo "ERR: Unsupported HOST '$HOST'" >&2
        exit 1
        ;;
esac

CROSS_GLIBC="$(store_path "glibc-cross-${GNU_HOST}")"
CROSS_GLIBC_STATIC="$(store_path "glibc-cross-${GNU_HOST}" static)"
CROSS_KERNEL="$(store_path "linux-libre-headers-cross-${GNU_HOST}")"
CROSS_GCC="$(store_path "gcc-cross-${GNU_HOST}")"
CROSS_GCC_LIB_STORE="$(store_path "gcc-cross-${GNU_HOST}" lib)"

if [[ -z "$CROSS_GLIBC" || -z "$CROSS_GLIBC_STATIC" || -z "$CROSS_KERNEL" || -z "$CROSS_GCC" || -z "$CROSS_GCC_LIB_STORE" ]]; then
    echo "ERR: Missing staged cross-toolchain paths for ${GNU_HOST} in GUIX_ENVIRONMENT" >&2
    exit 1
fi

CROSS_GCC_LIBS=( "${CROSS_GCC_LIB_STORE}/lib/gcc/${GNU_HOST}"/* )
CROSS_GCC_LIB="${CROSS_GCC_LIBS[0]}"

# Resolve the native gcc-toolchain library paths.  We do NOT export
# LIBRARY_PATH globally because it would leak into cross-gcc invocations
# (e.g., jemalloc's configure) and cause library version mismatches.
# Instead, rust-linker.sh applies these paths only for host (build-script)
# linking via CKB_RUST_HOST_LIBRARY_PATH.
NATIVE_GCC="$(store_path gcc-toolchain)"
NATIVE_GCC_STATIC="$(store_path gcc-toolchain static)"
unset LIBRARY_PATH
export CKB_RUST_HOST_LIBRARY_PATH="${NATIVE_GCC}/lib:${NATIVE_GCC_STATIC}/lib"

# Resolve real paths for the cross-compilers, then create thin wrappers
# that inject -Wl,-rpath pointing to the cross-glibc's lib.  This allows
# autoconf test programs (e.g., jemalloc's configure) to execute inside
# the Guix container: the dynamic linker finds libc.so.6 via rpath rather
# than needing LD_LIBRARY_PATH or standard paths (neither exists here).
# The rpath is stripped from the final release binary by patchelf later.
REAL_CC="$(command -v "${GNU_HOST}-gcc")"
REAL_CXX="$(command -v "${GNU_HOST}-g++")"

BASH_PATH="$(command -v bash)"
CC_WRAPPER="${DISTSRC}/cross-cc"
CXX_WRAPPER="${DISTSRC}/cross-cxx"
cat > "${CC_WRAPPER}" << CCEOF
#!${BASH_PATH}
for arg in "\$@"; do
    case "\$arg" in -c|-E|-S) exec "${REAL_CC}" "\$@" ;; esac
done
exec "${REAL_CC}" "\$@" -static-libgcc -Wl,-rpath="${CROSS_GLIBC}/lib"
CCEOF
chmod +x "${CC_WRAPPER}"
cat > "${CXX_WRAPPER}" << CXXEOF
#!${BASH_PATH}
for arg in "\$@"; do
    case "\$arg" in -c|-E|-S) exec "${REAL_CXX}" "\$@" ;; esac
done
exec "${REAL_CXX}" "\$@" -static-libgcc -static-libstdc++ -Wl,-rpath="${CROSS_GLIBC}/lib"
CXXEOF
chmod +x "${CXX_WRAPPER}"

export CC="${CC_WRAPPER}"
export CXX="${CXX_WRAPPER}"
export AR="${GNU_HOST}-gcc-ar"
export RANLIB="${GNU_HOST}-gcc-ranlib"
export NM="${GNU_HOST}-gcc-nm"
export STRIP="${GNU_HOST}-strip"

export "CC_${HOST//-/_}=${CC}"
export "CXX_${HOST//-/_}=${CXX}"
export "AR_${HOST//-/_}=${AR}"
export "RANLIB_${HOST//-/_}=${RANLIB}"

export CROSS_C_INCLUDE_PATH="${CROSS_GCC_LIB}/include:${CROSS_GCC_LIB}/include-fixed:${CROSS_GLIBC}/include:${CROSS_KERNEL}/include"
export CROSS_CPLUS_INCLUDE_PATH="${CROSS_GCC}/include/c++:${CROSS_GCC}/include/c++/${GNU_HOST}:${CROSS_GCC}/include/c++/backward:${CROSS_C_INCLUDE_PATH}"
export CROSS_LIBRARY_PATH="${CROSS_GCC_LIB_STORE}/lib:${CROSS_GCC_LIB}:${CROSS_GLIBC}/lib:${CROSS_GLIBC_STATIC}/lib"

IFS=':' read -r -a cross_paths <<< "${CROSS_C_INCLUDE_PATH}:${CROSS_CPLUS_INCLUDE_PATH}:${CROSS_LIBRARY_PATH}"
for path in "${cross_paths[@]}"; do
    if [[ -n "$path" && ! -d "$path" ]]; then
        echo "ERR: Expected cross-toolchain path '$path' to exist" >&2
        exit 1
    fi
done

clang_root="$(dirname "$(dirname "$(command -v clang)")")"
export LIBCLANG_PATH="${clang_root}/lib"
export LLVM_CONFIG_PATH="$(command -v llvm-config)"

native_libgcc_dir="$(dirname "$(gcc -print-file-name=libgcc_s.so.1)")"
native_libstdcpp_dir="$(dirname "$(g++ -print-file-name=libstdc++.so.6)")"
export LD_LIBRARY_PATH="${native_libgcc_dir}:${native_libstdcpp_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"

export CKB_RUST_HOST_LINKER="$(command -v gcc)"
# Use the REAL cross-gcc as the target linker (not the rpath wrapper),
# because rust-linker.sh handles linker flags for the final binary.
export CKB_RUST_TARGET_LINKER="${REAL_CC}"
export CKB_RUST_TARGET_TRIPLE="${HOST}"
export CKB_RUST_DYNAMIC_LINKER="${DYNAMIC_LINKER}"
export "CARGO_TARGET_${TARGET_ENV_SUFFIX}_LINKER=${runtime_linker}"

bindgen_clang_args=(
    "--target=${GNU_HOST}"
    "-isystem${CROSS_GCC_LIB}/include"
    "-isystem${CROSS_GCC_LIB}/include-fixed"
    "-isystem${CROSS_GLIBC}/include"
    "-isystem${CROSS_KERNEL}/include"
    "-isystem${CROSS_GCC}/include/c++"
    "-isystem${CROSS_GCC}/include/c++/${GNU_HOST}"
    "-isystem${CROSS_GCC}/include/c++/backward"
)
export BINDGEN_EXTRA_CLANG_ARGS="${bindgen_clang_args[*]}"
export "BINDGEN_EXTRA_CLANG_ARGS_${HOST//-/_}=${bindgen_clang_args[*]}"

export CFLAGS="-O2 -g -ffile-prefix-map=/gnu/store=/usr -fdebug-prefix-map=${DISTSRC}=."
export CXXFLAGS="${CFLAGS}"
export RUSTFLAGS="--remap-path-prefix=${DISTSRC}=. --remap-path-prefix=/gnu/store=/usr"

if [[ ! -e "$GIT_ARCHIVE" ]]; then
    echo "ERR: Expected pre-vendored source archive at '$GIT_ARCHIVE'" >&2
    exit 1
fi

rm -rf "$DISTSRC/src" "$INSTALLPATH"
mkdir -p "$DISTSRC/src" "$INSTALLPATH"

tar -C "$DISTSRC/src" --strip-components=1 --no-same-owner -xf "$GIT_ARCHIVE"
cd "$DISTSRC/src"

cargo build \
    --locked \
    --offline \
    --target "$HOST" \
    --bin ckb \
    --profile prod \
    --features "with_sentry,with_dns_seeding,portable" \
    -j "$JOBS"

binary="${CARGO_TARGET_DIR}/${HOST}/prod/ckb"
if [[ ! -x "$binary" ]]; then
    echo "ERR: Expected built binary at '$binary'" >&2
    exit 1
fi

mkdir -p "${INSTALLPATH}/docs"
cp "$binary" "${INSTALLPATH}/ckb"
patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 --remove-rpath "${INSTALLPATH}/ckb"
cp README.md CHANGELOG.md COPYING "${INSTALLPATH}/"
cp -R devtools/init "${INSTALLPATH}/"
cp -R docs "${INSTALLPATH}/"
cp rpc/README.md "${INSTALLPATH}/docs/rpc.md"

interp="$(readelf -l "${INSTALLPATH}/ckb" | sed -n 's@.*Requesting program interpreter: \(.*\)]@\1@p')"
runpath="$(readelf -d "${INSTALLPATH}/ckb" | sed -n 's@.*Library runpath: \[\(.*\)\]@\1@p')"

if [[ "${interp}" == /gnu/store/* ]]; then
    echo "ERR: Built binary still references a Guix-store ELF interpreter: ${interp}" >&2
    exit 1
fi

if [[ "${runpath}" == *"/gnu/store/"* ]]; then
    echo "ERR: Built binary still references a Guix-store RUNPATH: ${runpath}" >&2
    exit 1
fi

python3 /ckb/contrib/guix/symbol-check.py "${INSTALLPATH}/ckb"

( cd "$STAGING_BASE"
  find "${DISTNAME}" -print0 \
      | sort --zero-terminated \
      | tar --create --no-recursion --mode='u+rw,go+r-w,a+X' --null --files-from=- \
      | gzip -9n > "${OUTDIR}/${DISTNAME}.tar.gz"
)

( cd "$OUTDIR"
  sha256sum "${DISTNAME}.tar.gz" > SHA256SUMS
)

echo "Built release archive:"
echo "  ${OUTDIR}/${DISTNAME}.tar.gz"
