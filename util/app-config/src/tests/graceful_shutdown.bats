#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_ckb_graceful_shutdown() {
  ckb run --indexer -C ${CKB_DIRNAME} &> ${TMP_DIR}/ckb_run.log &
  PID=$!
  sleep 10
  kill ${PID}

  while kill -0 ${PID}; do
      sleep 1
  done

  tail -n 500 ${TMP_DIR}/ckb_run.log
}

function ckb_graceful_shutdown { #@test
  run _ckb_graceful_shutdown

  [ "$status" -eq 0 ]

  assert_output --regexp "INFO ckb_bin::subcommand::run  Trapped exit signal, exiting..."
  assert_output --regexp "INFO ckb_chain::chain  ChainService received exit signal, exit now"
  assert_output --regexp "INFO ckb_sync::synchronizer  BlockDownload received exit signal, exit now"
  assert_output --regexp "INFO ckb_tx_pool::chunk_process  TxPool chunk_command service received exit signal, exit now"
  assert_output --regexp "INFO ckb_tx_pool::service  TxPool is saving, please wait..."
  assert_output --regexp "INFO ckb_tx_pool::service  TxPool reorg process service received exit signal, exit now"
  assert_output --regexp "INFO ckb_indexer::service  Indexer received exit signal, exit now"
  assert_output --regexp "INFO ckb_notify  NotifyService received exit signal, exit now"
  assert_output --regexp "INFO ckb_block_filter::filter  BlockFilter received exit signal, exit now"
  assert_output --regexp "INFO ckb_sync::types::header_map  HeaderMap limit_memory received exit signal, exit now"
  assert_output --regexp "INFO ckb_network::network  NetworkService receive exit signal, start shutdown..."
  assert_output --regexp "INFO ckb_network::network  NetworkService shutdown now"
  assert_output --regexp "INFO ckb_tx_pool::process  TxPool saved successfully"
  assert_output --regexp "INFO ckb_tx_pool::service  TxPool process_service exit now"
  assert_output --regexp "INFO ckb_stop_handler::stop_register  Waiting thread ChainService done"
  assert_output --regexp "INFO ckb_stop_handler::stop_register  Waiting thread BlockDownload done"
  assert_output --regexp "INFO ckb_bin  Waiting for all tokio tasks to exit..."
  assert_output --regexp "INFO ckb_bin  All tokio tasks and threads have exited. CKB shutdown"
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb_run.log
}
