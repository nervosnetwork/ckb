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
  bash -c "ckb init -C ${TMP_DIR}/import"
  bash -c "ckb import -C ${TMP_DIR}/import ${TMP_DIR}/ckb*.jsonl"
}

function ckb_import { #@test
  run _import
  [ "$status" -eq 0 ]
}

setup_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
  rm -rvf ${TMP_DIR}/import
}
