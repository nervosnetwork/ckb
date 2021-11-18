#!/usr/bin/env bash
set -euo pipefail
CKB_BENCH_PATH="$GITHUB_WORKSPACE/ckb-integration-test/ckb-bench"
JOB_ID="benchmark-$(date +'%Y-%m-%d')-in-10h"
JOB_DIRECTORY="$CKB_BENCH_PATH/job/$JOB_ID"
ANSIBLE_DIR="$GITHUB_WORKSPACE/ansible"
mkdir $ANSIBLE_DIR
echo "ANSIBLE_DIR=$ANSIBLE_DIR" >> $GITHUB_ENV
function benchmark() {
    $CKB_BENCH_PATH/script/benchmark.sh run
    cp $JOB_DIRECTORY/ansible/ckb-bench.log $ANSIBLE_DIR
    # TODO: copy report.yml to $ANSIBLE_DIR
    $CKB_BENCH_PATH/script/benchmark.sh clean
}

function github_report_error() {
    $CKB_BENCH_PATH/script/ok.sh add_comment nervosnetwork/ckb 2372 "**Benchmark Report**:\nBenchmark crashed"

    # double check
    $CKB_BENCH_PATH/script/benchmark.sh clean
}

function main() {
    benchmark || github_report_error
}
main $*