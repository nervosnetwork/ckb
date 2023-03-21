#!/usr/bin/env bash
set -euxo pipefail

export PATH=$PATH:/tmp/bats_testbed
export BATS_LIB_PATH=/tmp/bats_testbed/bats-core/test_helper/
export CKB_DIRNAME=/tmp/bats_testbed
export TMP_DIR=/tmp

for bats_cases in *.bats; do
  bats --trace "$bats_cases"
  ret=$?
  if [ "$ret" -ne "0" ]; then
    exit "$ret"
  fi
done
