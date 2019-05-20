#!/bin/bash
set -eu

TRAVIS_PULL_REQUEST="${TRAVIS_PULL_REQUEST:-false}"
TRAVIS_JOB_NUMBER="${TRAVIS_JOB_NUMBER:-0.1}"
TRAVIS_BUILD_NUMBER="${TRAVIS_BUILD_NUMBER:-0}"
TRAVIS_REPO_SLUG="${TRAVIS_REPO_SLUG:-nervosnetwork/ckb}"
TRAVIS_BRANCH="${TRAVIS_BRANCH:-"$(git rev-parse --abbrev-ref HEAD)"}"

CURRENT_FOLD=

if ! type -f travis_fold &> /dev/null; then
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

# Run test only in master branch and pull requests
RUN_TEST=false
# Run integration only in master, develop and rc branches
RUN_INTEGRATION=false
if [ "$TRAVIS_PULL_REQUEST" != false ]; then
  LAST_COMMIT_MSG="$(git log --max-count 1 --skip 1 --format="%s")"
  echo "Last commit message is \"${LAST_COMMIT_MSG}\""
  if [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[skip\ tests\]\  ]]; then
      :
  elif [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[only\ integration\]\  ]]; then
    RUN_INTEGRATION=true
  elif [[ "${LAST_COMMIT_MSG}" =~ ^[a-z]+:\ \[all\ tests\]\  ]]; then
    RUN_TEST=true
    RUN_INTEGRATION=true
  else
    RUN_TEST=true
  fi
elif [ "$TRAVIS_REPO_SLUG" = "nervosnetwork/ckb" ]; then
  RUN_INTEGRATION=true
  if [ "$TRAVIS_BRANCH" = master ] || [ "$TRAVIS_BUILD_NUMBER" = 0 ]; then
    RUN_TEST=true
  fi
else
  RUN_TEST=true
fi

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

echo "\${RUN_TEST} = ${RUN_TEST}"
echo "\${RUN_INTEGRATION} = ${RUN_INTEGRATION}"
echo "\${FMT} = ${FMT}"
echo "\${CHECK} = ${CHECK}"
echo "\${TEST} = ${TEST}"

if type -f cargo-sweep &> /dev/null; then
  fold cargo-sweep cargo sweep -s
fi

if [ "$RUN_TEST" = true ]; then
  if [ "$FMT" = true ]; then
    fold fmt make fmt
  fi
  if [ "$CHECK" = true ]; then
    fold check make check
    fold clippy make clippy
    fold security-audit make security-audit
  fi
  if [ "$TEST" = true ]; then
    fold test make test
  fi

  git diff --exit-code Cargo.lock
fi

fold_start "integration"
# We'll create PR for develop and rc branches to trigger the integration test.
if [ "$RUN_INTEGRATION" = true ]; then
  echo "Running integration test..."
  make integration

  # Switch to release mode when the running time is much longer than the build time.
  # make integration-release
else
  echo "Skip integration test..."
fi
fold_end

# Publish package for release
if [ -n "${TRAVIS_TAG:-}" -a -n "${GITHUB_TOKEN:-}" -a -n "${REL_PKG:-}" ]; then
  fold_start "package"
  echo "Start packaging..."

  git fetch --unshallow

  make prod
  devtools/ci/package.sh target/release/ckb

  fold_end
fi
