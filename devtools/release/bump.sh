#!/usr/bin/env bash

set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

main() {
  if [ $# != 1 ]; then
    echo "bump.sh version" >&2
    exit 1
  fi
  local v="$1"
  sed -i.bak -e 's/^version = .*/version = "'"$v"'"/' Cargo.toml
  rm -f Cargo.toml.bak
  sed -i.bak 's/badge\/version-.*-orange/badge\/version-'"$(echo "$v" | sed s/-/--/g)"'-orange/' README.md
  rm -f README.md.bak
  cargo check
}

main "$@"
