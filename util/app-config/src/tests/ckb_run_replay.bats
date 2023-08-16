#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'

_ckb_run() {
  ckb run -C ${CKB_DIRNAME} &> ${TMP_DIR}/ckb_run.log &
  PID=$!
  sleep 5
  kill ${PID}

  while kill -0 ${PID}; do
      echo "waiting for ckb to exit"
      sleep 1
  done
  tail -n 50 ${TMP_DIR}/ckb_run.log
}

_ckb_replay() {
  # from 1 to 2500 enough to trigger profile action
  CKB_LOG=err ckb replay -C ${CKB_DIRNAME} --tmp-target ${TMP_DIR} --profile 1 2500
}

function ckb_run { #@test
  run _ckb_run
  [ "$status" -eq 0 ]
  # assert_output --regexp "ckb_chain::chain.*block number:.*, hash:.*, size:.*, cycles:.*"
  assert_output --regexp "ckb_bin  ckb shutdown"
}

function ckb_replay { #@test
  run _ckb_replay
  [ "$status" -eq 0 ]
  assert_output --regexp "End profiling, duration:.*, txs:.*, tps:.*"
}

teardown_file() {
  rm -f ${TMP_DIR}/ckb_run.log ${TMP_DIR}/ckb_run.pid
}
