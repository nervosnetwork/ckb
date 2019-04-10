#!/bin/bash
set -ev

echo "TRAVIS_BRANCH=$TRAVIS_BRANCH"

cargo sweep -s

if [ "$FMT" = true ]; then
  make fmt
fi
if [ "$CHECK" = true ]; then
  make check
  make clippy
fi
if [ "$TEST" = true ]; then
  make test
fi

git diff --exit-code Cargo.lock

if [ "$TRAVIS_BRANCH" = master -o "$TRAVIS_BRANCH" = staging -o "$TRAVIS_BRANCH" = trying ]; then
  cargo build
  cd test && cargo run ../target/debug/ckb

  # Switch to release mode when the running time is much longer than the build time.
  # cargo build --release
  # cargo run --release -p ckb-test target/release/ckb
fi

if [ -n "$TRAVIS_TAG" -a -n "$GITHUB_TOKEN" -a -n "$REL_PKG" ]; then
  make build
  rm -rf releases
  mkdir releases
  PKG_NAME="ckb_${TRAVIS_TAG}_${REL_PKG%%.*}"
  mkdir "releases/$PKG_NAME"
  mv target/release/ckb "releases/$PKG_NAME"
  cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
  cp -R devtools/init/ "releases/$PKG_NAME"
  if [ -d docs ]; then
    cp -R docs "releases/$PKG_NAME"
  fi

  pushd releases
  if [ "${REL_PKG#*.}" = "tar.gz" ]; then
    tar -czf $PKG_NAME.tar.gz $PKG_NAME
  else
    zip -r $PKG_NAME.zip $PKG_NAME
  fi
  popd
fi
