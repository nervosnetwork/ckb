#!/usr/bin/env bash
#
# Run CI locally for current SHA and submit the result via GitHub statuses API.
#
# Usage:
#   devtools/ci/ci-local.sh
#
# Dependencies: curl
#
# Generate a personal access token and export it via environment variable `GITHUB_ACCESS_TOKEN`.

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

if [ -z "${GITHUB_ACCESS_TOKEN:-}" ]; then
  echo 'GITHUB_ACCESS_TOKEN not set' >&2
  exit 1
fi

github_api() {
  local path="$1"
  shift
  curl https://api.github.com"$path" \
    --user ":$GITHUB_ACCESS_TOKEN" \
    -H 'Content-Type: application/json' \
    -H 'Accept: application/json' \
    -i "$@"
}

make ci
local sha="$(git rev-parse HEAD)"
github_api "/repos/nervosnetwork/ckb/statuses/$sha" -X POST -d'{"state":"success","context":"Travis CI - Pull Request","description":"via devtools/ci/ci-local.sh"}'
