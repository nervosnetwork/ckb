#!/usr/bin/env bash

set -euo pipefail
set +e
${CKB_DIR}/ckb migrate --check
EXIT_CODE="${PIPESTATUS[0]}"
# check_code = `printf '%d\n' $?`
echo "check_code is "${EXIT_CODE}
if [ ${EXIT_CODE} == 0 ]; then
  ${CKB_DIR}/ckb migrate --force
  EXIT_CODE="${PIPESTATUS[0]}"
  echo "migrate exit code is "${EXIT_CODE}
  if [ ${EXIT_CODE} != 0 ]; then
    echo "migrate faile,please try again"
    exit ${EXIT_CODE}
  else
    echo "DB migrate done"
  fi
fi
