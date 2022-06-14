#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_init() {
  bash -c "ckb init -C ${CKB_DIRNAME} -f"
}

_init_mainnet() {
  bash -c "ckb init -f -C ${CKB_DIRNAME} -c mainnet"
}

_reset_all() {
  bash -c "ckb reset-data -C ${CKB_DIRNAME} --all -f"
}

_init_ba_1() {
  # full args
  bash -c "ckb init --ba-arg 0xabcdef \
        --ba-code-hash 0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8 \
        --ba-hash-type type \
        --ba-message 0x -C ${CKB_DIRNAME} -f"
}
_init_ba_2() {
  # ba-code-hash should coop with ba-arg and ba-message
  bash -c "ckb init --ba-code-hash 0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8 -C ${CKB_DIRNAME} -f"
}

_init_ba_3() {
  bash -c "ckb init --ba-code-hash 0x9bd7e06f3ecf4be0f2fcd2188b23f1b9fcc88e5d4b65a8637b17723bbda3cce8 \
        --ba-arg 0xabcdef --ba-message 0x -C ${CKB_DIRNAME} -f"
}
_init_ba_4() {
  # ba-arg and ba-message should be even length
  bash -c "ckb init --ba-arg 0xabc -C ${CKB_DIRNAME} -f"
}
_init_ba_5() {
  # ba-arg and ba-message should be even length
  bash -c "ckb init --ba-message 0x0 -C ${CKB_DIRNAME} -f"
}
_init_ba_6() {
  bash -c "ckb init --ba-arg 0xabcd --ba-message 0x -C ${CKB_DIRNAME} -f"
}
_init_ba_7() {
  # ba-hash-type set, but should coop with ba-arg and message, otherwise ckb-miner part disabled
  bash -c "ckb init --ba-hash-type type -C ${CKB_DIRNAME} -f"
}
_init_ba_8() {
  bash -c "ckb init --ba-arg 0xc8328aabcd9b9e8e64fbc566c4385c3bdeb219d7 --ba-message 0x --ba-hash-type type -C ${CKB_DIRNAME} -f"
}

_init_list_chain_1() {
  # avaiable chains
  bash -c "ckb init -l"
}
_init_list_chain_2() {
  bash -c "ckb init --list-chains"
}

_init_genesis_message_1() {
  # only for dev chain
  bash -c "ckb init -C ${CKB_DIRNAME} --genesis-message test_genesis_message"
}
_init_genesis_message_2() {
  # message set in message section of specs/dev.toml
  bash -c "ckb init -C ${CKB_DIRNAME} -f -c dev --genesis-message test_genesis_message"
}
_init_import_spec_1() {
  # make spec/dev.toml
  bash -c "ckb init -C ${CKB_DIRNAME} -f -c dev"
  mv specs/dev.toml ${CKB_DIRNAME}/dev

  # import dev spec
  bash -c "ckb init -C ${CKB_DIRNAME} -f -c dev --import-spec ${CKB_DIRNAME}/dev"
}
_init_import_spec_2() {
  # import not found spec
  bash -c "ckb init -C ${CKB_DIRNAME} -f -c dev --import-spec not_found_file.toml"
}

_init_log_1() {
  # default log to both: file and stdout
  bash -c "ckb init -C ${CKB_DIRNAME} -f"
}
_init_log_2() {
  # log to file
  bash -c "ckb init -C ${CKB_DIRNAME} -f --log-to file"
}
_init_log_3() {
  # log to stdout
  bash -c "ckb init -C ${CKB_DIRNAME} -f --log-to stdout"
}

_init_p2p_port() {
  # change to 3115
  bash -c "ckb init -C ${CKB_DIRNAME} -f --p2p-port 3115"
}
_init_rpc_port() {
  # change to 3114
  bash -c "ckb init -C ${CKB_DIRNAME} -f --rpc-port 3114"
}

function init_ba { #@test
  run _init_ba_1
  [ "$status" -eq 0 ]
  assert_output --regexp "WARN: the block assembler arg is not a valid secp256k1 pubkey hash.*It will require \`ckb run --ba-advanced\` to enable this block assembler.*Reinitialized CKB directory.*create.*Genesis Hash: .*"

  run _init_ba_2
  [ "$status" -ne 0 ]

  run _init_ba_3
  [ "$status" -eq 0 ]
  assert_output --regexp "WARN: the block assembler arg is not a valid secp256k1 pubkey hash.*It will require \`ckb run --ba-advanced\` to enable this block assembler.*Reinitialized CKB directory.*create.*Genesis Hash: .*"

  run _init_ba_4
  [ "$status" -ne 0 ]

  run _init_ba_5
  [ "$status" -ne 0 ]

  run _init_ba_6
  [ "$status" -eq 0 ]
  assert_output --regexp "WARN: the block assembler arg is not a valid secp256k1 pubkey hash.*It will require \`ckb run --ba-advanced\` to enable this block assembler.*Reinitialized CKB directory.*create.*Genesis Hash: .*"

  run _init_ba_7
  [ "$status" -eq 0 ]
  assert_output --regexp "WARN: mining feature is disabled because of lacking the block assembler config options.*Reinitialized CKB directory.*create.*Genesis Hash: .*"

  run _init_ba_8
  [ "$status" -eq 0 ]
  assert_output --regexp "Reinitialized CKB directory.*create.*Genesis Hash: .*"
}

function init_list_chain { #@test
  run _init_list_chain_1
  [ "$status" -eq 0 ]
  assert_output --regexp "mainnet.*testnet.*staging.*dev"
  run _init_list_chain_2
  [ "$status" -eq 0 ]
  assert_output --regexp "mainnet.*testnet.*staging.*dev"
}

function init_chain { #@test
  run _init_chain
}

function init_genesis_message { #@test
  run _init_genesis_message_1
  [ "$status" -ne 0 ]
  assert_output --regexp "Customizing consensus parameters for chain spec only works for dev chains"

  run _init_genesis_message_2
  [ "$status" -eq 0 ]
  $(grep -q "message = \"test_genesis_message\""  "${CKB_DIRNAME}"/specs/dev.toml)
}

function init_import_spec { #@test
  run _init_import_spec_1
  [ "$status" -eq 0 ]
  assert_output --regexp "create ckb.toml.*create ckb-miner.toml.*create default.db-options.*Genesis Hash:.*"

  run _init_import_spec_2
  [ "$status" -ne 0 ]
  assert_output --regexp "Reinitialized CKB directory in .*cp.*IO Error.*NotFound"
}

function init_log { #@test
  run _init_log_1
  [ "$status" -eq 0 ]
  $(grep -q "log_to_file = true" "${CKB_DIRNAME}"/ckb.toml)
  $(grep -q "log_to_stdout = true" "${CKB_DIRNAME}"/ckb.toml)

  run _init_log_2
  [ "$status" -eq 0 ]
  $(grep -q "log_to_file = true" "${CKB_DIRNAME}"/ckb.toml)
  $(grep -q "log_to_stdout = false" "${CKB_DIRNAME}"/ckb.toml)

  run _init_log_3
  [ "$status" -eq 0 ]
  $(grep -q "log_to_file = false" "${CKB_DIRNAME}"/ckb.toml)
  $(grep -q "log_to_stdout = true" "${CKB_DIRNAME}"/ckb.toml)
}

function init_port { #@test
  run _init_p2p_port
  [ "$status" -eq 0 ]
  $(grep -q "listen_addresses = \[\"/ip4/0.0.0.0/tcp/3115\"\]" "${CKB_DIRNAME}"/ckb.toml)

  run _init_rpc_port
  [ "$status" -eq 0 ]
  $(grep -q "listen_address = \"127.0.0.1:3114\"" "${CKB_DIRNAME}"/ckb.toml)
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
  cp ${CKB_DIRNAME}/ckb.toml ${TMP_DIR}/.
  cp ${CKB_DIRNAME}/ckb-miner.toml ${TMP_DIR}/.
  cp ${CKB_DIRNAME}/default.db-options ${TMP_DIR}/.
  cp -rf ${CKB_DIRNAME}/data ${TMP_DIR}/.
}

teardown_file() {
  # recover test bed files/dirs: ckb.toml, ckb-miner.toml, default.db-options, data
  cp ${TMP_DIR}/ckb.toml ${CKB_DIRNAME}/.
  cp ${TMP_DIR}/ckb-miner.toml ${CKB_DIRNAME}/.
  cp ${TMP_DIR}/default.db-options ${CKB_DIRNAME}/.
  cp -rf ${TMP_DIR}/data ${CKB_DIRNAME}/.
}
