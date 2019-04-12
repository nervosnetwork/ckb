#!/bin/bash
set -ev

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

# We'll create PR for develop and rc branches to trigger the integration test.
if [ "$TRAVIS_BRANCH" = master ]; then
  echo "Running integration test..."
  make integration

  # Switch to release mode when the running time is much longer than the build time.
  # make integration-release
else
  echo "Skip integration test..."
fi

# Publish package for release
if [ -n "$TRAVIS_TAG" -a -n "$GITHUB_TOKEN" -a -n "$REL_PKG" ]; then
  git fetch --unshallow
  make build
  rm -rf releases
  mkdir releases
  PKG_NAME="ckb_${TRAVIS_TAG}_${REL_PKG%%.*}"
  mkdir "releases/$PKG_NAME"
  mv target/release/ckb "releases/$PKG_NAME"
  cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
  cp -R devtools/init "releases/$PKG_NAME"
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
