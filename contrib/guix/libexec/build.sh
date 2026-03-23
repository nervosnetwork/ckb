#!/usr/bin/env bash
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

export HOME="${DISTSRC}/home"
mkdir -p "$HOME"

export CARGO_TARGET_DIR="$TARGET_DIR"
export OPENSSL_NO_VENDOR=1
export GUIX_LD_WRAPPER_DISABLE_RPATH=yes
export CC="${CC:-gcc}"
export CXX="${CXX:-g++}"

libgcc_dir="$(dirname "$("$CC" -print-libgcc-file-name)")"
libstdcpp_dir="$(dirname "$("$CXX" -print-file-name=libstdc++.so.6)")"
export LD_LIBRARY_PATH="${libgcc_dir}:${libstdcpp_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"

export CFLAGS="-O2 -g -ffile-prefix-map=/gnu/store=/usr -fdebug-prefix-map=${DISTSRC}=."
export CXXFLAGS="${CFLAGS}"

rustflags=(
    "--remap-path-prefix=${DISTSRC}=."
    "--remap-path-prefix=/gnu/store=/usr"
    "-Clink-arg=-Wl,--as-needed"
    "-Clink-arg=-Wl,-O2"
)

case "$HOST" in
    x86_64-unknown-linux-gnu)
        rustflags+=("-Clink-arg=-static-libstdc++")
        rustflags+=("-Clink-arg=-static-libgcc")
        ;;
    *)
        echo "ERR: Unsupported HOST '$HOST'" >&2
        exit 1
        ;;
esac

if [[ -n "${RUSTFLAGS:-}" ]]; then
    rustflags+=("${RUSTFLAGS}")
fi
export RUSTFLAGS="${rustflags[*]}"

if [[ ! -e "$GIT_ARCHIVE" ]]; then
    echo "ERR: Expected pre-vendored source archive at '$GIT_ARCHIVE'" >&2
    exit 1
fi

rm -rf "$DISTSRC/src" "$INSTALLPATH"
mkdir -p "$DISTSRC/src" "$INSTALLPATH"

tar -C "$DISTSRC/src" --strip-components=1 -xf "$GIT_ARCHIVE"
cd "$DISTSRC/src"

cargo build \
    --locked \
    --offline \
    --bin ckb \
    --profile prod \
    --features "with_sentry,with_dns_seeding,portable" \
    -j "$JOBS"

binary="${CARGO_TARGET_DIR}/prod/ckb"
if [[ ! -x "$binary" ]]; then
    echo "ERR: Expected built binary at '$binary'" >&2
    exit 1
fi

mkdir -p "${INSTALLPATH}/docs"
cp "$binary" "${INSTALLPATH}/ckb"
patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 "${INSTALLPATH}/ckb"
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

python3 contrib/guix/symbol-check.py "${INSTALLPATH}/ckb"

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
