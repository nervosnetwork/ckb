#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_ckb_graceful_shutdown() {
  ckb run -C ${CKB_DIRNAME} &> ${TMP_DIR}/ckb_run.log &
  PID=$!
  sleep 10
  kill ${PID}

  while kill -0 ${PID}; do
      echo "waiting for ckb to exit"
      sleep 1
  done

  tail -n 500 ${TMP_DIR}/ckb_run.log
}

function ckb_graceful_shutdown { #@test
  run _ckb_graceful_shutdown

  [ "$status" -eq 0 ]
  assert_output --regexp "INFO ckb_bin::subcommand::run  Trapped exit signal, exiting..."
  assert_output --regexp "DEBUG ckb_stop_handler::stop_register  received exit signal, broadcasting exit signal to all threads"
  assert_output --regexp "DEBUG ckb_tx_pool::chunk_process  TxPool received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_sync::types::header_map  HeaderMap limit_memory received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_chain::chain  ChainService received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_sync::synchronizer  thread BlockDownload received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_network::network  NetworkService receive exit signal, start shutdown..."
  assert_output --regexp "INFO ckb_tx_pool::service  TxPool is saving, please wait..."
  assert_output --regexp "DEBUG ckb_tx_pool::service  TxPool received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_block_filter::filter  BlockFilter received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_network::services::dump_peer_store  dump peer store before exit"
  assert_output --regexp "DEBUG ckb_notify  NotifyService received exit signal, exit now"
  assert_output --regexp "DEBUG ckb_stop_handler::stop_register  wait thread ChainService done"
  assert_output --regexp "DEBUG ckb_stop_handler::stop_register  wait thread BlockDownload done"
  assert_output --regexp "DEBUG ckb_stop_handler::stop_register  all ckb threads have been stopped"
  assert_output --regexp "DEBUG ckb_bin  waiting all tokio tasks done"
  assert_output --regexp "INFO ckb_tx_pool::process  TxPool save successfully"
  assert_output --regexp "INFO ckb_bin  ckb shutdown"
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb_run.log
}
