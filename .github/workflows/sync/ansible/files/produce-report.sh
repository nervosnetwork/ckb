#!/usr/bin/env bash

# Usage
#
# ```
# ./produce-report.sh /path/to/instance-0.run.log
# ```

case "$OSTYPE" in
    darwin*)
        if ! type gsed &> /dev/null || ! type ggrep &> /dev/null; then
            echo "GNU sed and grep not found! You can install via Homebrew" >&2
            echo >&2
            echo "    brew install grep gnu-sed" >&2
            exit 1
        fi

        SED=gsed
        GREP=ggrep
        ;;
    *)
        SED=sed
        GREP=grep
        ;;
esac


ckb_run_log="$1"

[[ $ckb_run_log == *"run.log" ]] || { echo "Wrong Usage"; exit 1; }

hostname=$(basename "$ckb_run_log" | $SED "s/.run.log//g")
version=$($GREP -m 1 "ckb version: " "$ckb_run_log" | $SED -r 's/.*ckb version: (.*)$/\1/g')
fln=$($GREP 'ChainService INFO ckb_chain::chain  block:' "$ckb_run_log" | head -n 1)
lln=$($GREP 'ChainService INFO ckb_chain::chain  block:' "$ckb_run_log" | tail -n 1)
fbn=$(echo $fln | $SED -r 's/.*block: ([0-9]+), .*/\1/g')
lbn=$(echo $lln | $SED -r 's/.*block: ([0-9]+), .*/\1/g')
fts=$(date "+%s" -d "${fln:0:30}")
lts=$(date "+%s" -d "${lln:0:30}")
cost=$[$lts - $fts]
speed=$[($lbn - $fbn) / $cost]

echo "# '${hostname}' synchronized from block-${fbn} to block-${lbn}, taking ${cost} seconds, averaging ${speed} blocks per second."
echo "- Version:                ${version}"
echo "  Network:                ${CKB_NETWORK_NAME:-"unknown"}"
echo "  Hostname:               ${hostname}"
echo "  TimeCostSeconds:        ${cost}"
echo "  AverateBlocksPerSecond: ${speed}"
echo "  FromBlockNumber:        ${fbn}"
echo "  ToBlockNumber:          ${lbn}"
echo "  FromDataTime:           ${fln:0:30}"
echo "  ToDataTime:             ${lln:0:30}"
