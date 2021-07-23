#!/bin/bash

set -euo pipefail

set -e
function get_tip_block_number() {
    tip_block_json=` echo '{
        "id": 2,
        "jsonrpc": "2.0",
        "method": "get_tip_block_number",
        "params": [
        ]
    }' \
    | tr -d '\n' \
    | curl -H 'content-type: application/json' -d @- \
    http://127.0.0.1:8114 `

   TIP_BLOCK_NUMBER=`echo $tip_block_json | jq --raw-output '.result'`
   TIP_BLOCK_NUMBER=$(printf %d $TIP_BLOCK_NUMBER)
   echo $TIP_BLOCK_NUMBER
}
FIRST_TIP_BLOCK_NUMBER=`get_tip_block_number`
echo "Fsirt tip block number is "$FIRST_TIP_BLOCK_NUMBER
sleep 600
SECOND_TIP_BLOCK_NUMBER=`get_tip_block_number`
echo "Second tip block number is "$SECOND_TIP_BLOCK_NUMBER

if [ $FIRST_TIP_BLOCK_NUMBER == $SECOND_TIP_BLOCK_NUMBER ]; then
   echo "Tip block number No update in 10mins"
   exit 1
fi
