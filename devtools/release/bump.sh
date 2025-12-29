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

  # Enable release for the root crate
  sed -i.bak -e 's/^release = false/release = true/' .release-plz.toml
  rm -f .release-plz.toml.bak
  release-plz update -p ckb --git-token "$GITHUB_TOKEN" --allow-dirty --repo-url "https://github.com/nervosnetwork/ckb"
  git checkout .release-plz.toml

  release-plz set-version "ckb@$v"
  cargo check
}

main "$@"
