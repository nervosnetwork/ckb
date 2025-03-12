#!/usr/bin/env bats
bats_load_library 'bats-assert'
bats_load_library 'bats-support'


_init() {
  bash -c "ckb init -C ${CKB_DIRNAME} -f "
}

_uncomment_notify_config() {
  sed -i 's/# \[notify\]/\[notify\]/g' ${CKB_DIRNAME}/ckb.toml
}


_run() {
  ckb run -C ${CKB_DIRNAME} &> ${TMP_DIR}/ckb_notify.log &
  PID=$!
  sleep 3
  kill ${PID}

  while kill -0 ${PID}; do
      sleep 1
  done

  grep -q "CKB shutdown" ${TMP_DIR}/ckb_notify.log
}

_log_no_error() {
  if grep -q -i error ${TMP_DIR}/ckb_notify.log; then
    echo "error found in log: " $(grep -i error ${TMP_DIR}/ckb_notify.log)
    return 1
  fi
}

function run_with_uncomment_notify_config { #@test
  run _init
  [ "$status" -eq 0 ]

  run _uncomment_notify_config
  [ "$status" -eq 0 ]

  run _run
  [ "$status" -eq 0 ]

  run _log_no_error
  [ "$status" -eq 0 ]

  cat ${TMP_DIR}/ckb_notify.log
}
