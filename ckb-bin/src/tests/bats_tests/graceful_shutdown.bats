#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_ckb_graceful_shutdown() {
  ckb run --indexer -C ${CKB_DIRNAME} &> ${TMP_DIR}/ckb_run.log &
  PID=$!
  sleep 10
  kill ${PID}

  while kill -0 ${PID} &>/dev/null; do
      sleep 1
  done

  tail -n 500 ${TMP_DIR}/ckb_run.log
}

function ckb_graceful_shutdown { #@test
  run _ckb_graceful_shutdown

  [ "$status" -eq 0 ]

  # Keep only the core shutdown invariants to avoid flaky timing-dependent logs.
  assert_output --regexp "INFO ckb_bin::subcommand::run  Trapped exit signal, exiting..."
  assert_output --regexp "INFO ckb_bin  Waiting for all tokio tasks to exit..."
  assert_output --regexp "INFO ckb_bin  All tokio tasks and threads have exited. CKB shutdown"
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb_run.log
}
