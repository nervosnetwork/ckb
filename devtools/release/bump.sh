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
  find . -name 'Cargo.toml' -print0 | xargs -0 sed -i.bak 's/^version = .*/version = "'"$v"'"/'
  find . -name 'Cargo.toml.bak' -exec rm -f {} \;
  cargo check
}

main "$@"
