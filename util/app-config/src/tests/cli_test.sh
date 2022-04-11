#!/usr/bin/env bash
export PATH=$PATH:/tmp/bats_testbed
export BATS_LIB_PATH=/usr/lib
export CKB_DIRNAME=/tmp/bats_testbed
export TMP_DIR=/tmp

for bats_cases in *.bats; do
  bats "$bats_cases"
  ret=$?
  if [ "$ret" -ne "0" ]; then
    exit "$ret"
  fi
done
