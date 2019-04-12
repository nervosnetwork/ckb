#!/bin/bash
set -ev

echo "TRAVIS_BRANCH=$TRAVIS_BRANCH"

cargo sweep -s

# Run test only in master branch and pull requests
RUN_TEST=false
# Run integration only in master, develop and rc branches
RUN_INTEGRATION=false
if [ "$TRAVIS_PULL_REQUEST" != false ]; then
  RUN_TEST=true
else
  RUN_INTEGRATION=true
  if [ "$TRAVIS_BRANCH" = master ]; then
    RUN_TEST=true
  fi
fi

if [ "$RUN_TEST" = true ]; then
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
fi

# We'll create PR for develop and rc branches to trigger the integration test.
if [ "$RUN_INTEGRATION" = true ]; then
  echo "Running integration test..."
  cargo build --verbose
  cd test && cargo run ../target/debug/ckb

  # Switch to release mode when the running time is much longer than the build time.
  # cargo build --release
  # cargo run --release -p ckb-test target/release/ckb
else
  echo "Skip integration test..."
fi
