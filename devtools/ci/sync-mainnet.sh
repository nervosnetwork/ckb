#!/usr/bin/env bash
set -euo pipefail
SYNC_MAINNET_PATH="$GITHUB_WORKSPACE/ckb-integration-test/ckb-sync-mainnet"
JOB_ID="sync-mainnet-$(date +'%Y-%m-%d')-in-10h"
ANSIBLE_DIRECTORY="$SYNC_MAINNET_PATH/job/$JOB_ID/ansible"
sync_mainnet_id=$GITHUB_RUN_ID
start_time=$(date +%Y-%m-%d' '%H:%M:%S.%6N)
state=0 #0:success,1:failed
GITHUB_BRANCH=${GITHUB_BRANCH:-"$GITHUB_REF_NAME"}
function parse_report_and_inster_to_postgres() {
  time=$start_time
  ckb_commit_id=`git describe --dirty --always --match _EXCLUDE__ --abbrev=7`
  ckb_commit_time=`git log -1 --date=iso "--pretty=format:%cd" | cut -d ' ' -f 1,2`
  if [ -f "$ANSIBLE_DIRECTORY/ckb-bench.brief.md" ]; then
    while read -r LINE;
    do
      LINE=$(echo "$LINE" | sed -e 's/\r//g')
      ckb_version=$(echo $LINE | awk -F '|' '{print $2}')
      time_s=$(echo $LINE | awk -F '|' '{print $3}')
      speed=$(echo $LINE | awk -F '|' '{print $4}')
      tip=$(echo $LINE | awk -F '|' '{print $5}')
      hostname=$(echo $LINE | awk -F '|' '{print $6}')

      sql="insert into sync_mainnet_report values("
      if [ -n $sync_mainnet_id ]; then
        sql=$sql"'$sync_mainnet_id'"" ,"
      fi
      if [ -n "$time" ]; then
        sql=$sql"'$time'"" ,"
      fi
      if [ -n "$GITHUB_BRANCH" ]; then
        sql=$sql"'$GITHUB_BRANCH'"" ,"
      fi
      if [ -n $GITHUB_EVENT_NAME ]; then
        sql=$sql"'$GITHUB_EVENT_NAME'"" ,"
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
      if [ -n "$time_s" ]; then
        sql=$sql"'$time_s'"" "
      fi
      if [ -n "$speed" ]; then
        sql=$sql",""'$speed'"
      fi
      if [ -n $tip ]; then
        sql=$sql",""'$tip'"
      fi
      if [ -n "$hostname" ]; then
        sql=$sql",""'$hostname'"" "
      fi
      psql -h ${PSQL_HOST} -p ${PSQL_PORT} -U $PSQL_USER  -d ${dbname}  -c "$sql);"
    # done < "$ANSIBLE_DIR/ckb-bench.brief.md"
    done < "$ANSIBLE_DIRECTORY/ckb-bench.brief.md"
  fi
}
function insert_report_to_postgres() {
    end_time=$(date +%Y-%m-%d' '%H:%M:%S.%6N)
    dbname="ckbtest"
    BENCHMARK_REPORT="https://github.com/${GITHUB_REPOSITORY}actions/runs/$GITHUB_RUN_ID"
    sql="insert into sync_mainnet values("
    if [ -n $sync_mainnet_id ]; then
        sql=$sql"'$sync_mainnet_id'"" ,"
    fi
    if [ -n $state ]; then
        sql=$sql"'$state'"" ,"

    fi
    if [ -n "$start_time" ]; then
        sql=$sql"'$start_time'"" "
    fi
    if [ -n "$end_time" ]; then
        sql=$sql",""'$end_time'"
    fi
    if [ -n "$GITHUB_BRANCH" ]; then
        sql=$sql",""'$GITHUB_BRANCH'"
    fi
    if [ -n $GITHUB_EVENT_NAME ]; then
        sql=$sql",""'$GITHUB_EVENT_NAME'"
    fi
    if [ -n $BENCHMARK_REPORT ]; then
        sql=$sql",""'$BENCHMARK_REPORT'"
    fi
    psql -h ${PSQL_HOST} -p ${PSQL_PORT} -U $PSQL_USER  -d ${dbname}  -c "$sql);"
    parse_report_and_inster_to_postgres
}
function sync_mainnet() {
    $SYNC_MAINNET_PATH/script/sync-mainnet.sh run
    $SYNC_MAINNET_PATH/script/sync-mainnet.sh clean
    insert_report_to_postgres
}

function github_report_error() {
    $SYNC_MAINNET_PATH/script/ok.sh add_comment nervosnetwork/ckb 2372 "**Sync-Mainnet Report**:\nSync-Mainnet crashed"

    # double check
    $SYNC_MAINNET_PATH/script/sync-mainnet.sh clean
    insert_report_to_postgres
}

function main() {
    # sync_mainnet || github_report_error
    insert_report_to_postgres
}
main $*