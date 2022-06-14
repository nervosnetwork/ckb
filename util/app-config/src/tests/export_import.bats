#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_export_target() {
  bash -c "ckb export -C ${CKB_DIRNAME} -t ${TMP_DIR}"
}
_export_create_directory() {
  bash -c "ckb export -C ${CKB_DIRNAME} -t ${TMP_DIR}/create_directory"
}

function export { #@test
  run _export_target
  [ "$status" -eq 0 ]
  # output is dynamically print on console, skip the content match

  run _export_create_directory
  [ "$status" -eq 0 ]
  ls ${TMP_DIR}/create_directory
}

_import() {
  bash -c "ckb import -C ${CKB_DIRNAME} ${TMP_DIR}/ckb*.json"
}
_import_non_exist_directory() {
  bash -c "ckb import -C ${CKB_DIRNAME} -t ${TMP_DIR}/non_exist_directory"
}

function ckb_import { #@test
  run _import
  [ "$status" -eq 0 ]

  run _import_non_exist_directory
  [ "$status" -ne 0 ]
}

setup_file() {
  rm -f ${TMP_DIR}/ckb*.json
  rm -rf ${TMP_DIR}/create_directory
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb*.json
  rm -rf ${TMP_DIR}/create_directory
}