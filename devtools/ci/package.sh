#!/bin/bash
set -eu

TRAVIS_TAG="${TRAVIS_TAG:-"$(git describe)"}"
if [ -z "${REL_PKG:-}" ]; then
  if [ "$(uname)" = Darwin ]; then
    REL_PKG=x86_64-apple-darwin.zip
  else
    REL_PKG=x86_64-unknown-linux-gnu.tar.gz
  fi
fi

PKG_NAME="ckb_${TRAVIS_TAG}_${REL_PKG%%.*}"

rm -rf releases
mkdir releases

mkdir "releases/$PKG_NAME"
cp "$1" "releases/$PKG_NAME"
cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
cp -R devtools/init "releases/$PKG_NAME"
cp -R docs "releases/$PKG_NAME"
cp rpc/README.md "releases/$PKG_NAME/docs/rpc.md"

pushd releases
if [ "${REL_PKG#*.}" = "tar.gz" ]; then
  tar -czf $PKG_NAME.tar.gz $PKG_NAME
else
  zip -r $PKG_NAME.zip $PKG_NAME
fi
popd
