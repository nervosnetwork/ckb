#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_reset_network_secret_key() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --network-secret-key"
}
_reset_network_peer_store() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --network-peer-store"
}
_reset_network() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --network"
}
_reset_logs() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --logs"
}
_reset_database() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --database"
}
_reset_all() {
  bash -c "ckb -C ${CKB_DIRNAME} reset-data -f --all"
}

function ckb_reset { #@test
  run _reset_network_secret_key
  [ "$status" -eq 0 ]
  line=$(ls data/network/ | grep "secret_key" | wc -l) && [ $line -eq 0 ]

  run _reset_network_peer_store
  [ "$status" -eq 0 ]
  line=$(ls data/network/ | grep "peer_store" | wc -l) && [ $line -eq 0 ]

  run _reset_network
  [ "$status" -eq 0 ]
  line=$(ls data/ | grep "network" | wc -l) && [ $line -eq 0 ]

  run _reset_logs
  [ "$status" -eq 0 ]
  line=$(ls data/ | grep "logs" | wc -l) && [ $line -eq 0 ]

  run _reset_database
  [ "$status" -eq 0 ]
  line=$(ls data/ | grep "db" | wc -l) && [ $line -eq 0 ]

  run _reset_all
  [ "$status" -eq 0 ]
  line=$(ls -d | grep "data" | wc -l) && [ $line -eq 0 ]
}

teardown_file() {
  ckb import -C ${CKB_DIRNAME} ${CKB_DIRNAME}/ckb_mainnet_4000.json
}