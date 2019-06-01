#!/bin/bash
set -eu

TRAVIS_JOB_NUMBER="${TRAVIS_JOB_NUMBER:-0.1}"
TRAVIS_BUILD_NUMBER="${TRAVIS_BUILD_NUMBER:-0}"

CURRENT_FOLD=

if ! type travis_fold &> /dev/null; then
  travis_fold() {
    local action="$1"
    shift
    if [ "$action" = start ]; then
      echo "----(( $*"
    else
      echo "----)) $*"
      echo
    fi
  }
  travis_time_start() {
    :
  }
  travis_time_finish() {
    :
  }
fi

fold_start() {
  CURRENT_FOLD="script.$1"
  travis_fold start "$CURRENT_FOLD"
  travis_time_start
}

fold_end() {
  travis_time_finish
  travis_fold end "$CURRENT_FOLD"
}

fold() {
  local title="$1"
  shift
  fold_start "$title"
  echo "\$ $*"
  "$@"
  fold_end
}

SUB_JOB_NUMBER="${TRAVIS_JOB_NUMBER##*.}"
# Run fmt and check evenly between osx and linux
if (( TRAVIS_BUILD_NUMBER % 2 == SUB_JOB_NUMBER - 1 )); then
  FMT=true
  CHECK=true
  TEST=true
else
  FMT=false
  CHECK=false
  TEST=true
fi

echo "\${FMT} = ${FMT}"
echo "\${CHECK} = ${CHECK}"
echo "\${TEST} = ${TEST}"

if [ "$FMT" = true ]; then
  cargo fmt --version || rustup component add rustfmt

  fold fmt make fmt
fi
if [ "$CHECK" = true ]; then
  cargo clippy --version || rustup component add clippy
  cargo audit --version || cargo install cargo-audit

  fold check make check
  fold clippy make clippy
  fold security-audit make security-audit
fi
if [ "$TEST" = true ]; then
  fold test make test
fi

git diff --exit-code Cargo.lock
