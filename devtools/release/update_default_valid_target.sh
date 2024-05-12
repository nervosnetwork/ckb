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

MIN_WORK_NOT_REACH_HEIGHT=500000

function get_ten_percentile_block_hashes(){
  local host=$1
  local lasttest_height=$2
  local current=$MIN_WORK_NOT_REACH_HEIGHT
  
  while [ $current -lt $lasttest_height ]; do
      current=${current%.*}
      current=$(( $current - (( $current % 10000 )) ))
      current_hex=$(printf "0x%x" $current)
      block_hash=$(get_block_hash $host $current_hex)
      >&2 printf "add multiple assume target: %10d %s\n" $current  $block_hash
      printf "            \"%s\", // height: %d \n" $block_hash $current
      current=$(bc <<< "scale=0; $current * 1.3")
      current=${current%.*}
  done
}

function print_60_days_ago_block(){
  local network=$1
  local host=$2
  local explorer_url=$3

  ASSUME_TARGET_HEIGHT=$(get_60days_ago_block ${host})
  ASSUME_TARGET_HEIGHT_DECIMAL=$(printf "%d" ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_HASH=$(get_block_hash ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_TEN_PERCENTILE_HASHES=$(get_ten_percentile_block_hashes ${host} ${ASSUME_TARGET_HEIGHT_DECIMAL})
  ASSUME_TARGET_TIMESTAMP=$(get_block_timestamp ${host} ${ASSUME_TARGET_HEIGHT})
  ASSUME_TARGET_DATE=$(date -d @$((${ASSUME_TARGET_TIMESTAMP} / 1000)))
  EXPLORER_URL=${explorer_url}/block/${ASSUME_TARGET_HASH}
  printf "the 60 days ago block is: %d %s in %s\n" ${ASSUME_TARGET_HEIGHT_DECIMAL} ${ASSUME_TARGET_HASH} "${ASSUME_TARGET_DATE}" >&2
  printf "you can view this block in ${EXPLORER_URL}\n\n" >&2
  ASSUME_TARGET_TEN_PERCENTILE_HASHES_LENGTH=$(wc -l <<< ${ASSUME_TARGET_TEN_PERCENTILE_HASHES})
  
  ALL_HASHES_LENGTH=$((1 + $ASSUME_TARGET_TEN_PERCENTILE_HASHES_LENGTH))

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
    pub const DEFAULT_ASSUME_VALID_TARGETS: [&str; ${ALL_HASHES_LENGTH}] =
        [
            ${ASSUME_TARGET_TEN_PERCENTILE_HASHES}
            "${ASSUME_TARGET_HASH}" // height: ${ASSUME_TARGET_HEIGHT_DECIMAL}
        ];
}
END_HEREDOC
)

    echo "${TEXT}"
}

IFS='' read -r -d '' TEXT_HEADER <<'EOF' || true
/// The mod mainnet and mod testnet's codes are generated
/// by script: ./devtools/release/update_default_valid_target.sh
/// Please don't modify them manually.
///
EOF

printf "Now: %s\n\n" "$(date)"
printf "Finding the 60 days ago block..., this script may take 1 minute\n\n"

echo "${TEXT_HEADER}" > util/constant/src/default_assume_valid_target.rs

printf "MainNet:\n"
TEXT_MAINNET=$(print_60_days_ago_block mainnet https://mainnet.ckb.dev https://explorer.nervos.org)
echo "${TEXT_MAINNET}" >> util/constant/src/default_assume_valid_target.rs

printf "TestNet:\n"
TEXT_TESTNET=$(print_60_days_ago_block testnet https://testnet.ckb.dev https://pudge.explorer.nervos.org)
echo "${TEXT_TESTNET}" >> util/constant/src/default_assume_valid_target.rs
echo

rustfmt util/constant/src/default_assume_valid_target.rs

echo this script has overwrite file: util/constant/src/default_assume_valid_target.rs
echo Please review the changes
