#!/usr/bin/env bash
#
# Run CI locally for current SHA and submit the result via GitHub statuses API.
#
# Usage:
#   devtools/ci/local.sh [--integration] <pr_number>
#
# Dependencies: curl
#
# You have first checkout the PR locally first, for example, via GitHub cli:
#
#   hub pr checkout <pr_number>
#
# Generate a personal access token here
#
#   https://github.com/settings/tokens
#
# and export it via environment variable `GITHUB_ACCESS_TOKEN`.
#
# **Protect the access key carefully!**

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

main() {
  local ci_pass=false
  local integration_pass=skip
  if [[ "$#" > 0 && "$1" == "--integration" ]]; then
    shift
    integration_pass=false
  fi
  if [[ "$#" > 1 && "$2" == "--integration" ]]; then
    integration_pass=false
  fi

  if [[ "$#" == 0 ]]; then
    echo "usage: devtools/ci/local.sh [--integration] <pr_number>" >&2
    exit 1
  fi

  local pr="$1"

  if make ci; then
    ci_pass=true
  fi
  if [ "$integration_pass" = false ]; then
    if make integration; then
      integration_pass=true
    fi
  fi

  local sha="$(git rev-parse HEAD)"
  local body="@nervos-bot ci-status ${sha}\\n\\n"
  if [ $ci_pass = true ]; then
    body="${body}CI: success ✅\\n"
  else
    body="${body}CI: failure ❌\\n"
  fi
  if [ $integration_pass = true ]; then
    body="${body}Integration: success ✅\\n"
  elif [ "$integration_pass" = false ]; then
    body="${body}Integration: failure ❌\\n"
  fi
  github_api "/repos/nervosnetwork/ckb/issues/$pr/comments" -X POST -d'{"body":"'"$body"'"}'
}

main "$@"
