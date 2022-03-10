#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_gen() {
  bash -c "ckb peer-id gen -C ${CKB_DIRNAME} --secret-path ${TMP_DIR}/key"
}
_from_secret() {
  bash -c "ckb peer-id from-secret -C ${CKB_DIRNAME} --secret-path ${TMP_DIR}/key"
}

function peer_id_gen { #@test
  run _gen
  [ "$status" -eq 0 ]
}

function from_secret { #@test
  run _from_secret
  [ "$status" -eq 0 ]
  assert_output --regexp "^peer_id: [a-zA-Z0-9]+$"
}

teardown_file() {
  rm -f ${TMP_DIR}/key
}
