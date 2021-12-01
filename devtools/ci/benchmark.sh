#!/usr/bin/env bash
set -euo pipefail
CKB_BENCH_PATH="$GITHUB_WORKSPACE/ckb-integration-test/ckb-bench/devtools/ci"
JOB_ID="benchmark-$(date +'%Y-%m-%d')-in-10h"
JOB_DIRECTORY="$CKB_BENCH_PATH/job/$JOB_ID"
ANSIBLE_DIR="$GITHUB_WORKSPACE/ansible"
BENCHMARK_ID=$GITHUB_RUN_ID
START_TIME=$(date +%Y-%m-%d' '%H:%M:%S.%6N)
STATE=0 #0:success,1:failed
mkdir $ANSIBLE_DIR
echo "ANSIBLE_DIR=$ANSIBLE_DIR" >> $GITHUB_ENV
function parse_report_and_inster_to_postgres() {
  time=$START_TIME
  ckb_commit_id=`git describe --dirty --always --match _EXCLUDE__ --abbrev=7`
  ckb_commit_time=`git log -1 --date=iso "--pretty=format:%cd" | cut -d ' ' -f 1,2`
  echo $ckb_commit_id
  echo $ckb_commit_time
  if [ -f "$ANSIBLE_DIR/ckb-bench.brief.md" ]; then
    while read -r LINE;
    do
      LINE=$(echo "$LINE" | sed -e 's/\r//g')
      ckb_version=$(echo $LINE | awk -F '|' '{print $2}')
      transactions_per_second=$(echo $LINE | awk -F '|' '{print $3}')
      n_inout=$(echo $LINE | awk -F '|' '{print $4}')
      n_nodes=$(echo $LINE | awk -F '|' '{print $5}')
      delay_time_ms=$(echo $LINE | awk -F '|' '{print $6}')
      average_block_time_ms=$(echo $LINE | awk -F '|' '{print $7}')
      average_block_transactions=$(echo $LINE | awk -F '|' '{print $8}')
      average_block_transactions_size=$(echo $LINE | awk -F '|' '{print $9}')
      from_block_number=$(echo $LINE | awk -F '|' '{print $10}')
      to_block_number=$(echo $LINE | awk -F '|' '{print $11}')
      total_transactions=$(echo $LINE | awk -F '|' '{print $12}')
      total_transactions_size=$(echo $LINE | awk -F '|' '{print $13}')
      transactions_size_per_second=$(echo $LINE | awk -F '|' '{print $14}')

      sql="insert into benchmark_report values("
      if [ -n $BENCHMARK_ID ]; then
        sql=$sql"'$BENCHMARK_ID'"" ,"
      fi
      if [ -n "$time" ]; then
        sql=$sql"'$time'"" ,"
      fi
      if [ -n "$ckb_version" ]; then
        sql=$sql"'$ckb_version'"" ,"
      fi
      if [ -n $ckb_commit_id ]; then
        sql=$sql"'$ckb_commit_id'"" ,"
      fi
      if [ -n "$ckb_commit_time" ]; then
        sql=$sql"'$ckb_commit_time'"" ,"
      fi 
      if [ -n "$transactions_per_second" ]; then
        sql=$sql"'$transactions_per_second'"" "
      fi
      if [ -n "$n_inout" ]; then
        sql=$sql",""'$n_inout'"
      fi
      if [ -n $n_nodes ]; then
        sql=$sql",""'$n_nodes'"
      fi
      if [ -n "$delay_time_ms" ]; then
        sql=$sql",""'$delay_time_ms'"" "
      fi
      if [ -n "$average_block_time_ms" ]; then
        sql=$sql",""'$average_block_time_ms'"
      fi
      if [ -n $average_block_transactions ]; then
        sql=$sql",""'$average_block_transactions'"
      fi
      if [ -n $average_block_transactions_size ]; then
        sql=$sql",""'$average_block_transactions_size'"
      fi
      if [ -n "$from_block_number" ]; then
        sql=$sql",""'$from_block_number'"" "
      fi
      if [ -n "$to_block_number" ]; then
        sql=$sql",""'$to_block_number'"
      fi
      if [ -n "$total_transactions" ]; then
        sql=$sql",""'$total_transactions'"" "
      fi
      if [ -n "$total_transactions_size" ]; then
        sql=$sql",""'$total_transactions_size'"
      fi
      if [ -n "$transactions_size_per_second" ]; then
        sql=$sql",""'$transactions_size_per_second'"
      fi
      psql -h ${PSQL_HOST} -p ${PSQL_PORT} -U $PSQL_USER  -d ${dbname}  -c "$sql);"
    done < "$ANSIBLE_DIR/ckb-bench.brief.md"
  fi
}
function insert_report_to_postgres() {
    END_TIME=$(date +%Y-%m-%d' '%H:%M:%S.%6N)
    dbname="ckbtest"
    BENCHMARK_REPORT="https://github.com/${GITHUB_REPOSITORY}actions/runs/$GITHUB_RUN_ID"
    sql="insert into benchmark values("
    if [ -n $BENCHMARK_ID ]; then
        sql=$sql"'$BENCHMARK_ID'"" ,"
    fi
    if [ -n $STATE ]; then
        sql=$sql"'$STATE'"" ,"

    fi
    if [ -n "$START_TIME" ]; then
        sql=$sql"'$START_TIME'"" "
    fi
    if [ -n "$END_TIME" ]; then
        sql=$sql",""'$END_TIME'"
    fi
    if [ -n $BENCHMARK_REPORT ]; then
        sql=$sql",""'$BENCHMARK_REPORT'"
    fi
    psql -h ${PSQL_HOST} -p ${PSQL_PORT} -U $PSQL_USER  -d ${dbname}  -c "$sql);"
    parse_report_and_inster_to_postgres
}
function benchmark() {
    $CKB_BENCH_PATH/script/benchmark.sh run
    cp $JOB_DIRECTORY/ansible/ckb-bench.log $ANSIBLE_DIR
    cp $JOB_DIRECTORY/ansible/ckb-bench.brief.md $ANSIBLE_DIR
    # TODO: copy report.yml to $ANSIBLE_DIR
    $CKB_BENCH_PATH/script/benchmark.sh clean
    insert_report_to_postgres
}

function github_report_error() {
    STATE=1
    $CKB_BENCH_PATH/script/ok.sh add_comment nervosnetwork/ckb 2372 "**Benchmark Report**:\nBenchmark crashed"

    # double check
    $CKB_BENCH_PATH/script/benchmark.sh clean
    insert_report_to_postgres
}

function main() {
    benchmark || github_report_error
}
main $*