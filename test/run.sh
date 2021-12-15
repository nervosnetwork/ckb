#!/usr/bin/env bash

DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null 2>&1 && pwd )"


set -e
set -u
[ -n "${DEBUG:-}" ] && set -x || true

cd "$DIR"

if [ -z "${CI:-}" ]; then
  exec cargo run --release --features "deadlock_detection" "$@"
fi

set +e

test_id=$(date +"%Y%m%d-%H%M%S")
test_tmp_dir=${CKB_INTEGRATION_TEST_TMP:-$(pwd)/target/ckb-test/${test_id}}
export CKB_INTEGRATION_TEST_TMP="${test_tmp_dir}"
echo "CKB_INTEGRATION_TEST_TMP=$CKB_INTEGRATION_TEST_TMP" >> $GITHUB_ENV
mkdir -p "${test_tmp_dir}"
test_log_file="${test_tmp_dir}/integration.log"

export CKB_INTEGRATION_FAILURE_FILE="${test_tmp_dir}/integration.failure"
echo "Unknown integration error" > "$CKB_INTEGRATION_FAILURE_FILE"
cargo run "$@" 2>&1 | tee "${test_log_file}"
EXIT_CODE="${PIPESTATUS[0]}"
set -e

if [ "$EXIT_CODE" != 0 ] && [ "${TRAVIS_REPO_SLUG:-nervosnetwork/ckb}" = "nervosnetwork/ckb" ]; then
  if ! command -v sentry-cli &> /dev/null; then
    curl -sL https://sentry.io/get-cli/ | bash
  fi
  export SENTRY_DSN="https://15373165fbf2439b99ba46684dfbcb12@sentry.nervos.org/7"
  CKB_BIN="../target/debug/ckb"

  while [[ "$#" > 1 ]]; do
    case "$1" in
      --bin)
        CKB_BIN="$2"
        break
        ;;
      *)
        ;;
    esac
    shift
  done
  CKB_RELEASE="$("$CKB_BIN" --version)"

  if [ -n "${TRAVIS_BUILD_ID:-}" ] && [ -n "${LOGBAK_SERVER:-}" ]; then
    upload_id="travis-${test_id}-${TRAVIS_BUILD_ID:-0}-${TRAVIS_JOB_ID:-0}-${TRAVIS_OS_NAME:-unknown}"
    cd "${test_tmp_dir}"/..
    tar -czf "${upload_id}.tgz" "${test_id}"
    expect <<EOF
spawn sftp -o "StrictHostKeyChecking=no" "${LOGBAK_USER}@${LOGBAK_SERVER}"
expect "assword:"
send "${LOGBAK_PASSWORD}\r"
expect "sftp>"
send "put ${upload_id}.tgz ci/travis/\r"
expect "sftp>"
send "bye\r"
EOF
    cd -
  fi
# upload github actions log if test failed
  if [ -n "${BUILD_BUILDID:-}" ] && [ -n "${LOGBAK_SERVER:-}" ]; then
    upload_id="github-actions-${test_id}-${BUILD_BUILDID:-0}-${ImageOS:-unknown}"
    cd "${test_tmp_dir}"/..
    tar -czf "${upload_id}.tgz" "${test_id}"
    expect <<EOF
spawn sftp -o "StrictHostKeyChecking=no" "${LOGBAK_USER}@${LOGBAK_SERVER}"
expect "assword:"
send "${LOGBAK_PASSWORD}\r"
expect "sftp>"
send "put ${upload_id}.tgz ci/travis/\r"
expect "sftp>"
send "bye\r"
EOF
    cd -
  fi
  unset LOGBAK_USER LOGBAK_PASSWORD LOGBAK_SERVER

  unset encrypted_82dff4145bbf_iv encrypted_82dff4145bbf_key GITHUB_TOKEN GPG_SIGNER QINIU_ACCESS_KEY QINIU_SECRET_KEY
  cat "$CKB_INTEGRATION_FAILURE_FILE" | xargs -t -L 1 -I '%' sentry-cli send-event -m '%' -r "$CKB_RELEASE" --logfile "${test_log_file}"
fi

exit "$EXIT_CODE"
