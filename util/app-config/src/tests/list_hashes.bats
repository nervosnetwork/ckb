#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_list_hashes() {
  bash -c "ckb -C ${CKB_DIRNAME} list-hashes"
}
_list_hashes_bundle() {
  bash -c "ckb -C ${CKB_DIRNAME} list-hashes -b"
}

function ckb_import { #@test
  run _list_hashes
  [ "$status" -eq 0 ]
  assert_output --regexp ".*# Spec: ckb.*ckb.system_cells.*ckb.dep_groups"

  run _list_hashes_bundle
  [ "$status" -eq 0 ]
  assert_output --regexp ".*# Spec: ckb.*ckb.system_cells.*ckb.dep_groups.*# Spec: ckb_testnet.*# Spec: ckb_staging.*# Spec: ckb_dev"
}