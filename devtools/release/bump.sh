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

  # Reset version to the last stable release so release-plz will generate
  # a changelog. Without this, release-plz skips changelog generation when the
  # local version already exceeds the registry version (e.g. after an rc bump).
  local last_stable
  last_stable="$(git tag -l 'v*' --sort=-v:refname --merged HEAD | grep -v '-' | head -1 | sed 's/^v//')"
  release-plz set-version "ckb@$last_stable"
  # Use the last stable version's changelog otherwise release-plz will complain
  # duplicate entries in the changelog.
  git restore -s "v$last_stable" -- CHANGELOG.md

  # Enable release for the root crate
  sed -i.bak -e 's/^release = false/release = true/' .release-plz.toml
  rm -f .release-plz.toml.bak
  release-plz update -p ckb --git-token "$GITHUB_TOKEN" --allow-dirty --repo-url "https://github.com/nervosnetwork/ckb"
  git checkout .release-plz.toml

  release-plz set-version "ckb@$v"
  cargo check
}

main "$@"
