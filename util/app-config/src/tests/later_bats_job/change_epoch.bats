#!/usr/bin/env bats
set -e

bats_load_library 'bats-assert'
bats_load_library 'bats-support'

NUMBER_OFFSET=0
NUMBER_BITS=24
NUMBER_MAXIMUM_VALUE=$((1 << NUMBER_BITS))
NUMBER_MASK=$((NUMBER_MAXIMUM_VALUE - 1))

INDEX_OFFSET=$((NUMBER_BITS))
INDEX_BITS=16
INDEX_MAXIMUM_VALUE=$((1 << INDEX_BITS))
INDEX_MASK=$((INDEX_MAXIMUM_VALUE - 1))

LENGTH_OFFSET=$((NUMBER_BITS + INDEX_BITS))
LENGTH_BITS=16
LENGTH_MAXIMUM_VALUE=$((1 << LENGTH_BITS))
LENGTH_MASK=$((LENGTH_MAXIMUM_VALUE - 1))

function extract_epoch_number() {
    local value=$1
    echo $(( (value >> NUMBER_OFFSET) & NUMBER_MASK ))
}

function extract_epoch_index() {
    local value=$1
    echo $(( (value >> INDEX_OFFSET) & INDEX_MASK ))
}

function extract_epoch_length() {
    local value=$1
    echo $(( (value >> LENGTH_OFFSET) & LENGTH_MASK ))
}

function tip_header_epoch() {
  curl -s -X POST http://127.0.0.1:8114 \
  -H 'Content-Type: application/json' \
  -d '{ "id": 42, "jsonrpc": "2.0", "method": "get_tip_header", "params": [ ] }' \
  | jq .result.epoch | xargs -I{} printf "%d\n" {}
}

function tip_header_number() {
  curl -s -X POST http://127.0.0.1:8114 \
  -H 'Content-Type: application/json' \
  -d '{ "id": 42, "jsonrpc": "2.0", "method": "get_tip_header", "params": [ ] }' \
  | jq .result.number | xargs -I{} printf "%d\n" {}
}

function block_kill() {
  kill $1
  while kill -0 $1; do
      echo "waiting for $1 to exit"
      sleep 1
  done
}

function ckb_change_epoch_length_for_dumm_mode { #@test
  ckb run -C ${CKB_DIRNAME} &> /dev/null &

  CKB_NODE_PID=$!
  sleep 5


  TIP_EPOCH=$(tip_header_epoch)

  TIP_EPOCH_NUMBER=$(extract_epoch_number ${TIP_EPOCH})
  TIP_EPOCH_INDEX=$(extract_epoch_index ${TIP_EPOCH})
  TIP_EPOCH_LENGTH=$(extract_epoch_length ${TIP_EPOCH})
  TIP_NUMBER=$(tip_header_number)

  echo tip_number is ${TIP_NUMBER}
  echo tip_epoch_number is ${TIP_EPOCH_NUMBER}, tip_epoch_index is ${TIP_EPOCH_INDEX}, tip_epoch_length is ${TIP_EPOCH_LENGTH}

  kill ${CKB_NODE_PID}

  block_kill ${CKB_NODE_PID}

  wget https://raw.githubusercontent.com/nervosnetwork/ckb/develop/resource/specs/mainnet.toml

  ckb init -c dev --import-spec mainnet.toml --force

  sed -i 's/Eaglesong/Dummy/g' specs/dev.toml
  sed -i '/genesis_epoch_length = 1743/a permanent_difficulty_in_dummy = true\nepoch_length_in_dummy = 25\n' specs/dev.toml

  sed -i 's/poll_interval = 1000/poll_interval = 1/g' ckb-miner.toml
  sed -i 's/value = 5000/value = 1/g' ckb-miner.toml

  sed -i 's/# \[block_assembler\]/\[block_assembler\]/g' ckb.toml
  sed -i 's/# code_hash =/code_hash =/g' ckb.toml
  sed -i 's/# args = "ckb-cli util blake2b --prefix-160 <compressed-pubkey>"/args = "0xc8328aabcd9b9e8e64fbc566c4385c3bdeb219d7"/g' ckb.toml
  sed -i 's/# hash_type =/hash_type =/g' ckb.toml
  sed -i 's/# message = "A 0x-prefixed hex string"/message = "0x"/g' ckb.toml



  ckb run --skip-spec-check --overwrite-spec -C ${CKB_DIRNAME} &> /dev/null &
  CKB_NODE_PID=$!

  ckb miner -C ${CKB_DIRNAME} &> /dev/null &
  CKB_MINER_PID=$!

  sleep 5

  while [ $(tip_header_number) -lt $(( ${TIP_NUMBER} + ${TIP_EPOCH_LENGTH} )) ]; do
    echo waiting for tip_number to be $(( ${TIP_NUMBER} + ${TIP_EPOCH_LENGTH} ))
    sleep 1
  done

  echo latest tip_header_number is $(tip_header_number)
  echo latest tip_header_epoch length is $(extract_epoch_length $(tip_header_epoch))
  echo latest tip_header_epoch number is $(extract_epoch_number $(tip_header_epoch))

  assert [ $(extract_epoch_length $(tip_header_epoch)) -eq 25 ]

  block_kill ${CKB_NODE_PID}
  block_kill ${CKB_MINER_PID}
}
