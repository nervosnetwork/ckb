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

function _export_range() { #@test
    bash -c "ckb init -C ${TMP_DIR}/import_range"
    bash -c "ckb export -C ${TMP_DIR}/import -t ${TMP_DIR}/import_range --from 1 --to 200"
    bash -c "ckb export -C ${TMP_DIR}/import -t ${TMP_DIR}/import_range --from 200 --to 300"
    bash -c "ckb export -C ${TMP_DIR}/import -t ${TMP_DIR}/import_range --from 300 --to 400"
    bash -c "ckb export -C ${TMP_DIR}/import -t ${TMP_DIR}/import_range --from 400 --to 500"
    bash -c "ckb import -C ${TMP_DIR}/import_range ${TMP_DIR}/import_range/ckb-1-200.jsonl"
    bash -c "ckb import -C ${TMP_DIR}/import_range ${TMP_DIR}/import_range/ckb-200-300.jsonl"
    bash -c "ckb import -C ${TMP_DIR}/import_range ${TMP_DIR}/import_range/ckb-300-400.jsonl --skip-script-verify"
    bash -c "ckb import -C ${TMP_DIR}/import_range ${TMP_DIR}/import_range/ckb-400-500.jsonl --skip-all-verify"
}

setup_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
  rm -rvf ${TMP_DIR}/import
}
