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
  make integration

  # Switch to release mode when the running time is much longer than the build time.
  # make integration-release
fi
