#!/bin/sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: sh devtools/freezer/profile.sh OUTPUT_DIR" >&2
    exit 2
fi

mkdir -p "$1"
output=$(cd "$1" && pwd)
cd "$(dirname "$0")/../.."

{
    git rev-parse HEAD
    rustc -Vv
    cargo -V
    uname -a
} > "$output/environment.txt"

baseline="freezer-profile-$(date +%s)-$$"
printf 'criterion baseline: %s\n' "$baseline" >> "$output/environment.txt"
cargo bench --locked -p ckb-freezer --bench archive_read -- --noplot \
    --save-baseline "$baseline" \
    | tee "$output/bench.txt"

binary=$(cargo bench --locked -p ckb-freezer --bench archive_read \
    --no-run --message-format=json | python3 -c '
import json
import sys
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "compiler-artifact" and event["target"]["name"] == "archive_read":
        print(event["executable"])
' )
if [ -z "$binary" ]; then
    echo "Cargo did not report the archive_read benchmark binary" >&2
    exit 1
fi

case "$(uname -s)" in
    Darwin)
        "$binary" --bench --profile-time 20 freezer/transaction/16x4KiB \
            > "$output/workload.txt" 2>&1 &
        pid=$!
        trap 'kill "$pid" 2>/dev/null || true' HUP INT TERM EXIT
        sleep 2
        sample "$pid" 10 10 -file "$output/freezer.sample" \
            > "$output/profiler.txt" 2>&1
        wait "$pid"
        trap - HUP INT TERM EXIT
        ;;
    Linux)
        perf record -g -o "$output/freezer.perf.data" -- \
            "$binary" --bench --profile-time 20 freezer/transaction/16x4KiB \
            > "$output/workload.txt" 2>&1
        perf report --stdio -i "$output/freezer.perf.data" \
            > "$output/profiler.txt"
        ;;
    *)
        echo "CPU sampling is supported here on macOS and Linux" >&2
        exit 1
        ;;
esac
