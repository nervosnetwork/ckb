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

function export_range() { #@test
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

function export_to_stdout() { #@test
    bash -c "ckb init -C ${TMP_DIR}/export_to_stdout"
    bash -c "ckb export -C ${TMP_DIR}/import --from 1 --to 200 -t - >${TMP_DIR}/export_to_stdout/ckb.jsonl"
    wc -l ${TMP_DIR}/export_to_stdout/ckb.jsonl
    stat ${TMP_DIR}/export_to_stdout/ckb.jsonl
}

function import_from_stdin() { #@test
    bash -c "ckb init -C ${TMP_DIR}/import_from_stdin"
    bash -c "cat ${TMP_DIR}/export_to_stdout/ckb.jsonl | ckb import -C ${TMP_DIR}/import_from_stdin - "
}


# test export to pipe and use gzip to compress
function export_to_pipe() { #@test
    bash -c "ckb init -C ${TMP_DIR}/export_to_pipe"
    bash -c "ckb export -C ${TMP_DIR}/import --from 1 --to 200 -t - | gzip >${TMP_DIR}/export_to_pipe/ckb.jsonl.gz"
    wc -l ${TMP_DIR}/export_to_pipe/ckb.jsonl.gz
    stat ${TMP_DIR}/export_to_pipe/ckb.jsonl.gz
    # import from pipe and use gzip to decompress
    bash -c "ckb init -C ${TMP_DIR}/import_from_pipe"
    bash -c "gzip -dc ${TMP_DIR}/export_to_pipe/ckb.jsonl.gz | ckb import -C ${TMP_DIR}/import_from_pipe -"
}

setup_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb*.jsonl
  rm -rvf ${TMP_DIR}/import
}
