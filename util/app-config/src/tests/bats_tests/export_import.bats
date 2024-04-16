#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_export() {
  bash -c "ckb export -C ${CKB_DIRNAME} -t ${TMP_DIR}"
}

function export { #@test
  run _export
  [ "$status" -eq 0 ]
  # output is dynamically print on console, skip the content match
}

_import() {
  bash -c "ckb import -C ${CKB_DIRNAME} ${TMP_DIR}/ckb*.json"
}

function ckb_import { #@test
  run _import
  [ "$status" -eq 0 ]
}

setup_file() {
  rm -f ${TMP_DIR}/ckb*.json
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb*.json
}
