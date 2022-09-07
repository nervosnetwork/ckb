#!/bin/bash
set -eu

GIT_TAG_NAME="${GIT_TAG_NAME:-"$(git describe)"}"
CKB_CLI_VERSION="${CKB_CLI_VERSION:-"$GIT_TAG_NAME"}"
if [ -z "${REL_PKG:-}" ]; then
  if [ "$(uname)" = Darwin ]; then
    REL_PKG=x86_64-apple-darwin.zip
  else
    REL_PKG=x86_64-unknown-linux-gnu.tar.gz
  fi
fi

PKG_NAME="ckb_${GIT_TAG_NAME}_${REL_PKG%%.*}"
ARCHIVE_NAME="ckb_${GIT_TAG_NAME}_${REL_PKG}"
echo "ARCHIVE_NAME=$ARCHIVE_NAME"

rm -rf releases
mkdir releases
mkdir "releases/$PKG_NAME"
cp "$1" "releases/$PKG_NAME"
cp README.md CHANGELOG.md COPYING "releases/$PKG_NAME"
cp -R devtools/init "releases/$PKG_NAME"
cp -R docs "releases/$PKG_NAME"
cp rpc/README.md "releases/$PKG_NAME/docs/rpc.md"

if [ ! "${SKIP_CKB_CLI:-false}" == "true" ]; then
  CKB_CLI_REL_PKG="$(echo "$REL_PKG" | sed 's/-portable//')"
  curl -LO "https://github.com/nervosnetwork/ckb-cli/releases/download/${CKB_CLI_VERSION}/ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG}"
  if [ "${CKB_CLI_REL_PKG##*.}" = "zip" ]; then
    unzip "ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG}"
  else
    tar -xzf "ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG}"
  fi
  mv "ckb-cli_${CKB_CLI_VERSION}_${CKB_CLI_REL_PKG%%.*}/ckb-cli" "releases/$PKG_NAME/ckb-cli"
fi

pushd releases
if [ "${REL_PKG#*.}" = "tar.gz" ]; then
  tar -czf $PKG_NAME.tar.gz $PKG_NAME
else
  zip -r $PKG_NAME.zip $PKG_NAME
fi
if [ -n "${GPG_SIGNER:-}" ]; then
  gpg -u "$GPG_SIGNER" -ab "$ARCHIVE_NAME"
fi
popd
