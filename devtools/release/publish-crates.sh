#!/usr/bin/env bash

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

CRATES="$(cat Cargo.toml | sed -n -e '1, /^members/d' -e '/^\]$/, $d' -e 's/.*["'\'']\(.*\)["'\''].*/\1/p')"
RUST_VERSION="$(cat rust-toolchain)"

retry_cargo_publish() {
  # Ignore dev dependencies
  rm -f Cargo.toml.bak
  sed -i.bak \
    -e '/^\[dev-dependencies\]/, /^\[/ { /^[^\[]/d }' \
    -e '/# dev-feature$/d' \
    Cargo.toml

  local RETRIES=5
  local INTERVAL=2
  local EXITSTATUS=127
  while [ $RETRIES != 0 ]; do
    cargo publish --allow-dirty "$@" 2>&1 | tee cargo-publish.log
    if [ ${PIPESTATUS[0]} = 0 ]; then
      RETRIES=0
      EXITSTATUS=
    else
      if grep -q 'crate .* is already uploaded' cargo-publish.log; then
        echo "==> Skip already uploaded version"
        RETRIES=0
        EXITSTATUS=
      else
        echo "=> retrying in $INTERVAL seconds ..."
        sleep $INTERVAL
        INTERVAL=$((INTERVAL * 2))
        RETRIES=$((RETRIES - 1))
      fi
    fi
  done

  rm -f cargo-publish.log
  git clean -f README.md
  if [ -f Cargo.toml.bak ]; then
    mv -f Cargo.toml.bak Cargo.toml
  fi

  if [ -n "$EXITSTATUS" ]; then
    mv -f Cargo.lock.bak Cargo.lock
    exit "$EXITSTATUS"
  fi
}

generate_readme() {
  CRATE_DESCRIPTION="$(sed -n -e '/^description\s*=\s*"""/,/^"""/p' -e 's/description\s*=\s*"\([^"].*\)"$/\1/p' Cargo.toml | grep -v '"""$')"
  CRATE_NAME="$(sed -n -e 's/name\s*=\s*"\([^"].*\)"$/\1/p' Cargo.toml)"

  echo "# $CRATE_NAME" >README.md
  echo >>README.md
  echo "This crate is a component of [ckb](https://github.com/nervosnetwork/ckb)." >>README.md
  echo >>README.md
  echo "$CRATE_DESCRIPTION" >>README.md
  echo >>README.md
  echo '## Minimum Supported Rust Version policy (MSRV)' >>README.md
  echo >>README.md
  echo "This crate's minimum supported rustc version is $RUST_VERSION" >>README.md
}

cp -f Cargo.lock Cargo.lock.bak

PUBLISH_FROM="${CKB_PUBLISH_FROM:-}"
YANK="${CKB_YANK:-}"
SKIP=false
if [ -n "$PUBLISH_FROM" ]; then
  SKIP=true
fi

for crate_dir in $CRATES; do
  case "$crate_dir" in
    benches | util/test-chain-utils)
      # ignore
      ;;
    *)
      if [ "$crate_dir" = "$PUBLISH_FROM" ]; then
        SKIP=false
      fi
      if [ "$SKIP" = true ]; then
        echo "=> skip $crate_dir"
      elif [ -n "$YANK" ]; then
        echo "=> yank $crate_dir"
        pushd "$crate_dir"
        cargo yank --vers "$YANK"
        popd
      else
        echo "=> publish $crate_dir"
        pushd "$crate_dir"
        git clean -f README.md
        if ! [ -f README.md ]; then
          generate_readme
        fi
        retry_cargo_publish "$@"
        popd
        rm -rf target/package/*
      fi
      ;;
  esac
done

if [ -n "$YANK" ]; then
  cargo yank --vers "$YANK"
else
  retry_cargo_publish "$@"
  mv -f Cargo.lock.bak Cargo.lock
fi
