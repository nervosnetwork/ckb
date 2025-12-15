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
  local network=$1
  local host=$2
  local explorer_url=$3

  ASSUME_TARGET_HEIGHT=$(get_60days_ago_block ${host})
  ASSUME_TARGET_HEIGHT_DECIMAL=$(printf "%d" ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_HASH=$(get_block_hash ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_TIMESTAMP=$(get_block_timestamp ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_DATE=$(date -d @$((${ASSUME_TARGET_TIMESTAMP} / 1000)))
  EXPLORER_URL=${explorer_url}/block/${ASSUME_TARGET_HASH}
  printf "the 60 days ago block is: %d %s in %s\n" ${ASSUME_TARGET_HEIGHT_DECIMAL} ${ASSUME_TARGET_HASH} "${ASSUME_TARGET_DATE}" >&2
  printf "you can view this block in ${EXPLORER_URL}\n\n" >&2

  TEXT=$(cat <<END_HEREDOC
/// sync config related to ${network}
pub mod ${network} {
    /// Default assume valid target for ${network}, expect to be a block 60 days ago.
    ///
    /// Need to update when CKB's new release
    /// in ${network}: the 60 days ago block is:
    /// height: ${ASSUME_TARGET_HEIGHT_DECIMAL}
    /// hash: ${ASSUME_TARGET_HASH}
    /// date: ${ASSUME_TARGET_DATE}
    /// you can view this block in ${EXPLORER_URL}
    pub const DEFAULT_ASSUME_VALID_TARGET: &str =
        "${ASSUME_TARGET_HASH}";
}
END_HEREDOC
)

    echo "${TEXT}"
}

IFS='' read -r -d '' TEXT_HEADER <<'EOF' || true
//! The mod mainnet and mod testnet's codes are generated
//! by script: ./devtools/release/update_default_valid_target.sh
//! Please don't modify them manually.

EOF

printf "Now: %s\n\n" "$(date)"
printf "Finding the 60 days ago block..., this script may take 1 minute\n\n"

printf "MainNet:\n"
TEXT_MAINNET=$(print_60_days_ago_block mainnet https://mainnet.ckb.dev https://explorer.nervos.org)

printf "TestNet:\n"
TEXT_TESTNET=$(print_60_days_ago_block testnet https://testnet.ckb.dev https://testnet.explorer.nervos.org)
echo "${TEXT_HEADER}" > util/constant/src/latest_assume_valid_target.rs
echo "${TEXT_MAINNET}" >> util/constant/src/latest_assume_valid_target.rs
echo "${TEXT_TESTNET}" >> util/constant/src/latest_assume_valid_target.rs
echo
echo this script has overwrite file: util/constant/src/latest_assume_valid_target.rs
echo Please review the changes
