# Allocator Benchmark

This directory contains a repeatable harness for comparing CKB allocator builds.
It is intended to collect evidence before changing the production default
allocator.

The minimum comparison is:

- `jemalloc`: current Linux production allocator baseline.
- `mimalloc`: candidate allocator.

Run the in-process block/tx-pool workload repeatedly:

```sh
REPEAT=5 TXS_SIZE=500 ROUNDS=10 devtools/bench-allocator/run-allocator-memory.sh
```

The script writes raw logs, a CSV file, and a Markdown summary under
`target/allocator-bench/<timestamp>/`.

When GNU `time` is available as `/usr/bin/time` or `gtime`, the CSV also
includes CPU time, max RSS, and page fault counters from the process wrapper.
Without GNU `time`, the benchmark still runs and those fields are left empty.

For a production decision, also collect node-level profiles for both allocator
builds:

- Block import or sync from the same data set.
- Tx-pool churn with repeated submit, eviction, and block template generation.
- Long-running node steady state, including RSS after idle periods.
- RPC/indexer/rich-indexer scenarios if those services are part of the target
  deployment.

Record at least wall time, CPU time, peak RSS, steady RSS, page faults,
throughput, and p95/p99 latency where the workload has request latency.
