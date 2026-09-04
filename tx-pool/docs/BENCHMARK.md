# Tx-pool paired benchmark

Performance evidence has one workload executor and one comparison runner:

- `benches/profile_one_shot.rs` constructs and observes the production service;
- `scripts/cross_version_benchmark.py` freezes two binaries and runs paired A/B
  attempts;
- `scripts/profile.py` profiles the same executor.

The benchmark is evidence, not a correctness oracle or permission to rank an
unfrozen candidate. `control/txpool-v8/STATE.json` owns the current source and
decision. Timing cannot weaken correctness or independent concurrency.

## Comparison contract

Inspect the parser for the complete current CLI:

```sh
python3 tx-pool/scripts/cross_version_benchmark.py --help
```

A representative run is:

```sh
python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /private/tmp/txp-base \
  --candidate-root /private/tmp/txp-candidate \
  --output /private/tmp/txp-result.json \
  --replicates-per-sample 4 \
  --scenario always_success,32000,100,8,4 \
  --scenario dependent_forest_10,32000,100,8,4
```

The runner freezes clean commits, Cargo inputs, CKB-VM packages/features,
byte-identical harness, host/toolchain, build commands and binary hashes. A
supplied binary requires `--*-binary-profile prod`; other binaries are built
once with profile `prod` into isolated targets.

Every scenario first runs a candidate/baseline pilot. Pilot corpus identities
must match exactly: transaction bytes/hashes, assigned cycles, script preflight
and runtime consensus digest. Recorded attempts then alternate AB/BA order;
multiple replicates are balanced inside each sample.

`attempts[]` is the sole measurement ledger. It contains the command, raw
output, source side, stable attempt ID, corpus, terminal multiset, build identity,
window and metrics. The runner atomically replaces the JSON checkpoint after
every new attempt. Resume an interrupted run with the same command plus
`--resume`; configuration and all frozen identities are revalidated, completed
attempt IDs are reused, and only missing attempts execute.

Each paired sample retains elapsed time, throughput, target CPU, p99,
allocations, RSS, context switches, reorg and shutdown. Summary
medians, ratios and relative MAD are mechanically derived; timing comparability
uses only the predeclared throughput-ratio MAD threshold.

`--allocation-observation enabled` is a separate allocation experiment. It
still records other observations for audit, but only allocation calls and bytes
may rank that run.

## Workload integrity

The executor covers independent scripts, RBF, dependency shapes, in-flight
reorg and delayed callbacks through the production service. Warm and target
phases end only on exact callback and relay terminal sets. Duplicate terminals,
unexpected rejects, unknown parents outside reverse workloads, generation reset,
corpus drift, invalid windows or incomplete shutdown invalidate the attempt.
Timeouts only bound hangs.

## Maintenance gates

After changing the executor, runner or analyzer, run:

```sh
cargo check -p ckb-tx-pool --bench profile_one_shot --features profiling,allocation-observation
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
```

Final performance decisions additionally require a result-before-frozen
candidate, freshly frozen `develop`, the representative scenario matrix and all
current correctness/concurrency gates.
