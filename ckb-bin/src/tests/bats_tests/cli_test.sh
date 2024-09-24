#!/usr/bin/env bash
set -euxo pipefail

CKB_BATS_TESTBED=/tmp/ckb_bats_testbed
mkdir -p ${CKB_BATS_TESTBED}

function cleanup {
  echo "Removing ${CKB_BATS_TESTBED}"
  rm -rf ${CKB_BATS_TESTBED}
}

git_clone_repo_with_retry() {
    local branch=$1
    local repo_address=$2
    local dir_name=$3
    local retry_count=5
    local retry_delay=5

    for i in $(seq 1 $retry_count); do
        git clone --depth 1 --branch "$branch" "$repo_address" "$dir_name" && break
        echo "Attempt $i failed. Retrying in $retry_delay seconds..."
        sleep $retry_delay
    done

    if [ $i -eq $retry_count ]; then
        echo "Failed to clone repository after $retry_count attempts."
        exit 1
    fi
}

trap cleanup EXIT

cp target/prod/ckb ${CKB_BATS_TESTBED}
cp ckb-bin/src/tests/bats_tests/*.bats ${CKB_BATS_TESTBED}
cp -r ckb-bin/src/tests/bats_tests/later_bats_job ${CKB_BATS_TESTBED}
cp ckb-bin/src/tests/bats_tests/*.sh ${CKB_BATS_TESTBED}

if [ ! -d "/tmp/ckb_bats_assets/" ]; then
    git_clone_repo_with_retry "main" "https://github.com/nervosnetwork/ckb-assets" "/tmp/ckb_bats_assets"
fi
cp /tmp/ckb_bats_assets/cli_bats_env/ckb_mainnet_4000.json ${CKB_BATS_TESTBED}

CKB_BATS_CORE_DIR=/tmp/ckb_bats_core
if [ ! -d "${CKB_BATS_CORE_DIR}/bats" ]; then
    git_clone_repo_with_retry "v1.9.0" "https://github.com/bats-core/bats-core.git" "${CKB_BATS_CORE_DIR}/bats"
    ${CKB_BATS_CORE_DIR}/bats/install.sh /tmp/ckb_bats_bin/tmp_install
fi

if [ ! -d "${CKB_BATS_CORE_DIR}/bats-support" ]; then
    git_clone_repo_with_retry "v0.3.0" "https://github.com/bats-core/bats-support.git" "${CKB_BATS_CORE_DIR}/bats-support"
fi
bash ${CKB_BATS_CORE_DIR}/bats-support/load.bash

if [ ! -d "${CKB_BATS_CORE_DIR}/bats-assert" ]; then
    git_clone_repo_with_retry "v2.1.0" "https://github.com/bats-core/bats-assert.git" "${CKB_BATS_CORE_DIR}/bats-assert"
fi

bash ${CKB_BATS_CORE_DIR}/bats-assert/load.bash

cd ${CKB_BATS_TESTBED}

./ckb init --force && sed -i 's/filter = "info"/filter = "debug"/g' ckb.toml && ./ckb import ckb_mainnet_4000.json

export PATH=${CKB_BATS_TESTBED}:/tmp/ckb_bats_bin/tmp_install/bin:${PATH}
export BATS_LIB_PATH=${CKB_BATS_CORE_DIR}
export CKB_DIRNAME=${CKB_BATS_TESTBED}
export TMP_DIR=${CKB_BATS_TESTBED}/tmp_dir
mkdir ${TMP_DIR}

for bats_cases in *.bats; do
  bats --verbose-run --print-output-on-failure --show-output-of-passing-tests "$bats_cases"
  ret=$?
  if [ "$ret" -ne "0" ]; then
    exit "$ret"
  fi
done

bats --verbose-run --print-output-on-failure --show-output-of-passing-tests ./later_bats_job/change_epoch.bats
ret=$?
if [ "$ret" -ne "0" ]; then
  exit "$ret"
fi
