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
  find . -name 'Cargo.toml' -print0 | xargs -0 sed -i.bak \
    -e 's/^version = .*/version = "'"$v"'"/' \
    -e 's/\({.*path = ".*",.* version = "= \)[^"]*/\1'"$v"'/'
  find . -name 'Cargo.toml.bak' -exec rm -f {} \;
  sed -i.bak 's/badge\/version-.*-orange/badge\/version-'"$(echo "$v" | sed s/-/--/g)"'-orange/' README.md
  rm -f README.md.bak
  cargo check
}

main "$@"
