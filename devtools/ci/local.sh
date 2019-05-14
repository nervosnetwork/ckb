#!/usr/bin/env bash
#
# Run CI locally for current SHA.
#
# Usage:
#   devtools/ci/local.sh [--integration]
#
# You have first checkout the PR locally first, for example, via GitHub cli:
#
#   hub pr checkout <pr_number>
#
# When the script completes, please post the output to the PR as a comment manually.
#
# You must have the write permission to the repository, otherwise @nervos-bot will ignore your comment.

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

main() {
  local ci_pass=false
  local integration_pass=skip
  if [[ "$#" > 0 && "$1" == "--integration" ]]; then
    shift
    integration_pass=false
  fi

  if make ci; then
    ci_pass=true
  fi
  if [ "$integration_pass" = false ]; then
    if make integration; then
      integration_pass=true
    fi
  fi

  local sha="$(git rev-parse HEAD)"
  echo "You can post the text below dash lines to PR as a comment"
  echo "---------------------------------------------------------"
  echo "@nervos-bot ci-status ${sha}"
  echo
  if [ $ci_pass = true ]; then
    echo "CI: success ✅"
  else
    echo "CI: failure ❌"
  fi
  if [ $integration_pass = true ]; then
    echo "Integration: success ✅"
  elif [ "$integration_pass" = false ]; then
    echo "Integration: failure ❌"
  fi
}

main "$@"
