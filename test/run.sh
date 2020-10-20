#!/usr/bin/env bash

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"


set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

cd "$DIR"

if [ -z "${CI:-}" ]; then
  exec cargo run "$@"
fi

set +e

export CKB_INTEGRATION_FAILURE_FILE="$(pwd)/integration.failure"
echo "Unknown integration error" > "$CKB_INTEGRATION_FAILURE_FILE"
cargo run "$@" 2>&1 | tee integration.log
EXIT_CODE="${PIPESTATUS[0]}"
set -e

if [ "$EXIT_CODE" != 0 ] && [ "${TRAVIS_REPO_SLUG:-nervosnetwork/ckb}" = "nervosnetwork/ckb" ]; then
  if ! command -v sentry-cli &> /dev/null; then
    curl -sL https://sentry.io/get-cli/ | bash
  fi
  export SENTRY_DSN="https://15373165fbf2439b99ba46684dfbcb12@sentry.nervos.org/7"
  CKB_BIN="../target/debug/ckb"

  while [[ "$#" > 1 ]]; do
    case "$1" in
      --bin)
        CKB_BIN="$2"
        break
        ;;
      *)
        ;;
    esac
    shift
  done

  CKB_RELEASE="$("$CKB_BIN" --version)"
  cat "$CKB_INTEGRATION_FAILURE_FILE" | xargs -t -L 1 -I '%' sentry-cli send-event -m '%' -r "$CKB_RELEASE" --logfile integration.log
fi

exit "$EXIT_CODE"
