#!/usr/bin/env bash

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

main() {
  local last_tag=$(git describe --abbrev=0)
  local last_version="${last_tag##*-}"
  local current_version=$(( last_version + 1 ))
  local current_tag="${last_tag%-*}-$current_version"
  git tag -a -s $current_tag
}

main "$@"
