#!/usr/bin/env bash
set -euxo pipefail

CKB_BATS_TESTBED=/tmp/ckb_bats_testbed
mkdir -p ${CKB_BATS_TESTBED}

function cleanup {
  echo "Removing ${CKB_BATS_TESTBED}"
  rm -rf ${CKB_BATS_TESTBED}
}

trap cleanup EXIT

cp target/release/ckb ${CKB_BATS_TESTBED}
cp util/app-config/src/tests/*.bats ${CKB_BATS_TESTBED}
cp util/app-config/src/tests/*.sh ${CKB_BATS_TESTBED}

if [ ! -d "/tmp/ckb_bats_assets/" ]; then
  git clone --depth=1 https://github.com/nervosnetwork/ckb-assets /tmp/ckb_bats_assets
fi
cp /tmp/ckb_bats_assets/cli_bats_env/ckb_mainnet_4000.json ${CKB_BATS_TESTBED}

CKB_BATS_CORE_DIR=/tmp/ckb_bats_core
if [ ! -d "${CKB_BATS_CORE_DIR}/bats" ]; then
  git clone --depth 1 --branch v1.9.0 https://github.com/bats-core/bats-core.git    ${CKB_BATS_CORE_DIR}/bats
  ${CKB_BATS_CORE_DIR}/bats/install.sh /tmp/ckb_bats_bin/tmp_install
fi

if [ ! -d "${CKB_BATS_CORE_DIR}/bats-support" ]; then
  git clone --depth 1 --branch v0.3.0 https://github.com/bats-core/bats-support.git ${CKB_BATS_CORE_DIR}/bats-support
fi
bash ${CKB_BATS_CORE_DIR}/bats-support/load.bash

if [ ! -d "${CKB_BATS_CORE_DIR}/bats-assert" ]; then
  git clone --depth 1 --branch v2.1.0 https://github.com/bats-core/bats-assert.git  ${CKB_BATS_CORE_DIR}/bats-assert
fi
bash ${CKB_BATS_CORE_DIR}/bats-assert/load.bash

cd ${CKB_BATS_TESTBED}

./ckb init --force && ./ckb import ckb_mainnet_4000.json

export PATH=${CKB_BATS_TESTBED}:/tmp/ckb_bats_bin/tmp_install/bin:${PATH}
export BATS_LIB_PATH=${CKB_BATS_CORE_DIR}
export CKB_DIRNAME=${CKB_BATS_TESTBED}
export TMP_DIR=${CKB_BATS_TESTBED}/tmp_dir
mkdir ${TMP_DIR}

for bats_cases in *.bats; do
  bats --trace "$bats_cases"
  ret=$?
  if [ "$ret" -ne "0" ]; then
    exit "$ret"
  fi
done
