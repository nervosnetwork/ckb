#!/usr/bin/env bash

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

CRATES="$(cat Cargo.toml| sed -n -e '0, /^members/d' -e '/^\]$/, $d' -e 's/.*["'\'']\(.*\)["'\''].*/\1/p')"

retry_cargo_publish() {
  # Ignore dev dependencies
  rm -f Cargo.toml.bak
  sed -i.bak -e '/^\[dev-dependencies\]/, /^\[/ { /^[^\[]/d }' Cargo.toml

  local RETRIES=3
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
  if [ -f Cargo.toml.bak ]; then
    mv -f Cargo.toml.bak Cargo.toml
  fi

  if [ -n "$EXITSTATUS" ]; then
    mv -f Cargo.lock.bak Cargo.lock
    exit "$EXITSTATUS"
  fi
}

cp -f Cargo.lock Cargo.lock.bak
for crate_dir in $CRATES; do
  case "$crate_dir" in
    benches | util/test-chain-utils | util/instrument | util/metrics-service | ckb-bin)
      # ignore
      ;;
    *)
      echo "=> publish $crate_dir"
      pushd "$crate_dir"
      retry_cargo_publish "$@"
      popd
      rm -rf target/package/*
      ;;
  esac
done
mv -f Cargo.lock.bak Cargo.lock
