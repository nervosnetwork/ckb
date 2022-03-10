#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_short() {
  bash -c "ckb -V"
}
_long() {
  bash -c "ckb --version"
}
_help() {
  bash -c "ckb -h"
}
_list_hashes() {
  bash -c "ckb list-hashes -C ${CKB_DIRNAME}"
}
_list_bundle_hashes() {
  bash -c "ckb list-hashes -C ${CKB_DIRNAME} -b"
}
_stats_default() {
  bash -c "ckb stats -C ${CKB_DIRNAME}"
}
_stats_with_range() {
  bash -c "ckb stats -C ${CKB_DIRNAME} --from 1 --to 500"
}
_full_help() {
  bash -c "ckb help"
}

#@test "ckb -V" {
function short_version { #@test
  run _short
  [ "$status" -eq 0 ]
  assert_output --regexp "^ckb [0-9.]+[-]?[a-z]*$"
}

#@test "ckb --version" {
function long_version { #@test
  run _long
  [ "$status" -eq 0 ]
  assert_output --regexp "^ckb [0-9.]+-.*\([0-9a-z-]+ [0-9]{4}-[0-9]{2}-[0-9]{2}\)$"
}

function help { #@test
  run _help
  [ "$status" -eq 0 ]
  assert_output --regexp "USAGE:.*OPTIONS:.*SUBCOMMANDS:.*"

  run _full_help
  [ "$status" -eq 0 ]
  assert_output --regexp "USAGE:.*OPTIONS:.*SUBCOMMANDS:.*"
}

function list_hashes { #@test
  run _list_hashes
  [ "$status" -eq 0 ]
  assert_output --regexp "\# Spec: ckb[_]?[a-z]*.*\[ckb[_]?[a-z]*\].*spec_hash = \"0x[0-9a-z]*\""
}

function list_bundle_hashes { #@test
  run _list_bundle_hashes
  [ "$status" -eq 0 ]
  assert_output --regexp "\# Spec: ckb.*\[ckb\].*spec_hash = \"0x[0-9a-z]*\""
}

function stats_default { #@test
  run _stats_default
  [ "$status" -eq 0 ]
  assert_output --regexp "uncle_rate:.*by_miner_script:.*by_miner_message:.*"
}

function stats_with_range { #@test
  run _stats_with_range
  [ "$status" -eq 0 ]
  assert_output --regexp "uncle_rate:.*by_miner_script:.*by_miner_message:.*"
}
