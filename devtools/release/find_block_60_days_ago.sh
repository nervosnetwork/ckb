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

MAINNET_ASSUME_TARGET_HEIGHT=$(get_60days_ago_block https://mainnet.ckb.dev)
MAINNET_ASSUME_TARGET_HASH=$(get_block_hash https://mainnet.ckb.dev ${MAINNET_ASSUME_TARGET_HEIGHT})
MAINNET_ASSUME_TARGET_TIMESTAMP=$(get_block_timestamp https://mainnet.ckb.dev ${MAINNET_ASSUME_TARGET_HEIGHT})
MAINNET_ASSUME_TARGET_DATE=$(date -d @$((${MAINNET_ASSUME_TARGET_TIMESTAMP} / 1000)))


TESTNET_ASSUME_TARGET_HEIGHT=$(get_60days_ago_block https://testnet.ckb.dev)
TESTNET_ASSUME_TARGET_HASH=$(get_block_hash https://testnet.ckb.dev ${TESTNET_ASSUME_TARGET_HEIGHT})
TESTNET_ASSUME_TARGET_TIMESTAMP=$(get_block_timestamp https://testnet.ckb.dev ${TESTNET_ASSUME_TARGET_HEIGHT})
TESTNET_ASSUME_TARGET_DATE=$(date -d @$((${TESTNET_ASSUME_TARGET_TIMESTAMP} / 1000)))

printf "today: %s\n\n" "$(date)"

printf "mainnet: the 60 days ago block is: %d %s in %s\n" ${MAINNET_ASSUME_TARGET_HEIGHT} ${MAINNET_ASSUME_TARGET_HASH} "${MAINNET_ASSUME_TARGET_DATE}"
printf "testnet: the 60 days ago block is: %d %s in %s\n" ${TESTNET_ASSUME_TARGET_HEIGHT} ${TESTNET_ASSUME_TARGET_HASH} "${MAINNET_ASSUME_TARGET_DATE}"
