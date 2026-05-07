#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
REPEAT="${REPEAT:-5}"
TXS_SIZE="${TXS_SIZE:-500}"
ROUNDS="${ROUNDS:-10}"
OUT_DIR="${OUT_DIR:-"$ROOT/target/allocator-bench/$(date -u +%Y%m%dT%H%M%SZ)"}"
CSV="$OUT_DIR/allocator-memory.csv"
TIME_BIN=""

if [[ -x /usr/bin/time ]]; then
  TIME_BIN="/usr/bin/time"
elif command -v gtime >/dev/null 2>&1; then
  TIME_BIN="$(command -v gtime)"
fi

mkdir -p "$OUT_DIR"

cat >"$OUT_DIR/metadata.txt" <<EOF
commit=$(git -C "$ROOT" rev-parse HEAD)
date_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)
repeat=$REPEAT
txs_size=$TXS_SIZE
rounds=$ROUNDS
rustc=$(rustc --version)
cargo=$(cargo --version)
uname=$(uname -a)
time_bin=${TIME_BIN:-unavailable}
EOF

printf 'allocator,run,txs_size,rounds,elapsed_ms,before_rss_bytes,after_setup_rss_bytes,after_workload_rss_bytes,after_drop_rss_bytes,peak_rss_bytes,virtual_memory_bytes,time_elapsed_seconds,time_user_seconds,time_system_seconds,time_max_rss_kb,time_minor_faults,time_major_faults\n' >"$CSV"

for allocator in jemalloc mimalloc; do
  (
    cd "$ROOT/benches"
    cargo bench --bench allocator_memory --no-default-features --features "$allocator" --no-run
  )
done

run_one() {
  local allocator="$1"
  local run="$2"
  local log="$OUT_DIR/${allocator}-${run}.log"
  local time_log="$OUT_DIR/${allocator}-${run}.time"

  echo "allocator=$allocator run=$run"
  if [[ -n "$TIME_BIN" ]]; then
    (
      cd "$ROOT/benches"
      "$TIME_BIN" -v -o "$time_log" \
        cargo bench --bench allocator_memory --no-default-features --features "$allocator" -- "$TXS_SIZE" "$ROUNDS"
    ) >"$log" 2>&1
  else
    : >"$time_log"
    (
      cd "$ROOT/benches"
      cargo bench --bench allocator_memory --no-default-features --features "$allocator" -- "$TXS_SIZE" "$ROUNDS"
    ) >"$log" 2>&1
  fi

  "$ROOT/devtools/bench-allocator/report.py" append \
    --allocator "$allocator" \
    --run "$run" \
    --log "$log" \
    --time-log "$time_log" \
    --csv "$CSV"
}

for run in $(seq 1 "$REPEAT"); do
  run_one jemalloc "$run"
  run_one mimalloc "$run"
done

"$ROOT/devtools/bench-allocator/report.py" summarize --csv "$CSV" --output "$OUT_DIR/summary.md"

echo "wrote $CSV"
echo "wrote $OUT_DIR/summary.md"
