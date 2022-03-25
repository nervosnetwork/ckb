#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_init() {
  bash -c "ckb init -C ${CKB_DIRNAME} -f "
}
_init_mainnet() {
  bash -c "ckb init -f -C ${CKB_DIRNAME} -c mainnet"
}
_reset_all() {
  bash -c "ckb reset-data -C ${CKB_DIRNAME} --all -f"
}
function init { #@test
  run _init
  [ "$status" -eq 0 ]
  assert_output --regexp "[iI]nitialized CKB directory.*create.*Genesis Hash: 0x92b197aa1fba0f63633922c61c92375c9c074a93e85963554f5499fe1450d0e5"
}
function init_mainnet { #@test
  run _init_mainnet
  [ "$status" -eq 0 ]
  assert_output --regexp "Reinitialized CKB directory.*create.*Genesis Hash: 0x92b197aa1fba0f63633922c61c92375c9c074a93e85963554f5499fe1450d0e5"
}

function reset_all { #@test
  run _reset_all
  [ "$status" -eq 0 ]
  assert_output --regexp "deleting .*data"
}

setup_file() {
  # backup test bed files/dirs: ckb.toml, ckb-miner.toml, default.db-options, data
  mv ${CKB_DIRNAME}/ckb.toml ${TMP_DIR}/.
  mv ${CKB_DIRNAME}/ckb-miner.toml ${TMP_DIR}/.
  mv ${CKB_DIRNAME}/default.db-options ${TMP_DIR}/.
  mv ${CKB_DIRNAME}/data ${TMP_DIR}/.
}

teardown_file() {
  # recover test bed files/dirs: ckb.toml, ckb-miner.toml, default.db-options, data
  mv ${TMP_DIR}/ckb.toml ${CKB_DIRNAME}/.
  mv ${TMP_DIR}/ckb-miner.toml ${CKB_DIRNAME}/.
  mv ${TMP_DIR}/default.db-options ${CKB_DIRNAME}/.
  mv ${TMP_DIR}/data ${CKB_DIRNAME}/.
}
