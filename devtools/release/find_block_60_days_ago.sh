#!/usr/bin/env bash
set -euo pipefail

function get_block_timestamp() {
	local host=$1
	local block_number=$2
    header=$(curl -s -X POST ${host} -H 'Content-Type: application/json' -d '{ "id": 42, "jsonrpc": "2.0", "method": "get_header_by_number", "params": [ "'"$block_number"'" ] }')
	echo ${header} | jq -r .result.timestamp
}

function get_block_hash() {
	local host=$1
	local block_number=$2
    header=$(curl -s -X POST ${host} -H 'Content-Type: application/json' -d '{ "id": 42, "jsonrpc": "2.0", "method": "get_header_by_number", "params": [ "'"$block_number"'" ] }')
	echo ${header} | jq -r .result.hash
}

function get_60days_ago_block(){
	local host=$1

	TIP_HEADER=$(curl -s -X POST ${host} -H 'Content-Type: application/json' -d '{ "id": 42, "jsonrpc": "2.0", "method": "get_tip_header", "params": [] }')
	TIP_NUMBER=$(echo ${TIP_HEADER} | jq -r .result.number )
	TIP_TIMESTAMP=$(echo ${TIP_HEADER} | jq -r .result.timestamp )

	START_NUMBER=$(printf "0x%x\n" $(( $(printf "%d\n" ${TIP_NUMBER}) - 700000 )))
	END_NUMBER=$(printf "0x%x\n" $(( $(printf "%d\n" ${TIP_NUMBER}) - 500000 )))


	# Binary search
	while [[ $(($END_NUMBER - $START_NUMBER)) -gt 1 ]]
	do
		MID_NUMBER=$(printf "0x%x\n" $(( ($START_NUMBER + $END_NUMBER) / 2 )))
		MID_TIMESTAMP=$(get_block_timestamp ${host} ${MID_NUMBER})

		if [[ $(($MID_TIMESTAMP + ((61 * 24 * 60 * 60 * 1000)) )) -gt ${TIP_TIMESTAMP} ]]; then
			END_NUMBER=${MID_NUMBER}
		else
			START_NUMBER=${MID_NUMBER}
		fi
	done

	echo ${START_NUMBER}
}

function print_60_days_ago_block(){
  local host=$1
  ASSUME_TARGET_HEIGHT=$(get_60days_ago_block ${host})
  ASSUME_TARGET_HASH=$(get_block_hash ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_TIMESTAMP=$(get_block_timestamp ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_DATE=$(date -d @$((${ASSUME_TARGET_TIMESTAMP} / 1000)))
  printf "the 60 days ago block is: %d %s in %s\n" ${ASSUME_TARGET_HEIGHT} ${ASSUME_TARGET_HASH} "${ASSUME_TARGET_DATE}"
}

printf "Now: %s\n\n" "$(date)"
printf "Finding the 60 days ago block..., this script may take 1 minute\n\n"
printf "MainNet:\n"
print_60_days_ago_block https://mainnet.ckb.dev
printf "TestNet:\n"
print_60_days_ago_block https://testnet.ckb.dev
