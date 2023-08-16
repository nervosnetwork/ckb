#!/usr/bin/env bash

on_push_pkg() {
  BRANCH=$(git symbolic-ref --quiet HEAD)
  BRANCH="${BRANCH#refs/heads/}"
  VERSION="v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
  echo "$BRANCH -> upstream/$BRANCH"
  echo "$BRANCH -> upstream/pkg/$VERSION"

  git push upstream "$BRANCH" "$BRANCH:pkg/$VERSION"
}

on_tag() {
  VERSION="v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
  git tag -s -m "$VERSION" "$VERSION"
}

on_push_tag() {
  VERSION="v$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)"
  git push upstream "$VERSION"
}

case "${1:-help}" in
push-pkg)
  on_push_pkg
  ;;
tag)
  on_tag
  ;;
push-tag)
  on_push_tag
  ;;
*)
  echo "$0 push-pkg|tag|push-tag"
  ;;
esac
