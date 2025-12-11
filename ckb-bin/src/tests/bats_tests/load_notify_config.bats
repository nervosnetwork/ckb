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
  ckb run -C ${CKB_DIRNAME} &>${TMP_DIR}/ckb_notify.log &
  PID=$!
  sleep 3
  kill ${PID} 2>/dev/null || true

  # Wait for process to terminate, with timeout and error suppression
  # The process may already be dead, so we ignore "No such process" errors
  local timeout=10
  local count=0
  while kill -0 ${PID} 2>/dev/null && [ $count -lt $timeout ]; do
    sleep 1
    count=$((count + 1))
  done

  # Ensure process is really dead
  kill -0 ${PID} 2>/dev/null && kill -9 ${PID} 2>/dev/null || true

  grep -q "CKB shutdown" ${TMP_DIR}/ckb_notify.log
}

_log_no_error() {
  # Filter out expected shutdown errors that occur during graceful termination
  # These are normal when channels/connections are closed during shutdown
  # Use grep with inverted patterns to exclude expected errors
  local unexpected_errors=$(grep -i error ${TMP_DIR}/ckb_notify.log |
    grep -v "unverified_block_rx err: receiving on an empty and disconnected channel" |
    grep -v "nc.send_message GetHeaders, error: P2P(Send(BrokenPipe))" || true)

  # If there are unexpected errors after filtering, report them
  if [ -n "$unexpected_errors" ]; then
    echo "error found in log: " "$unexpected_errors"
    return 1
  fi

  return 0
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
