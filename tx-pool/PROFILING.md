# Tx-pool profiling contract

Profiling is a maintained, feature-gated development component. It attributes
cost for one exact source, binary, workload and host identity; it does not prove
correctness, a global optimum or product dominance. Only
`control/txpool-v8/STATE.json` may authorize ranking a frozen candidate.

## Entry point and workload

`tx-pool/scripts/profile.py capture` builds Cargo profile `prod` and runs the
production-shaped `profile_one_shot` harness. A reused binary requires an
explicit `--binary-profile prod` attestation and is recorded by SHA-256. The
harness covers independent `always_success` and `secp256k1` transactions,
parent-first and child-first chains, dependent forests, fan-in, fan-out, RBF,
in-flight reorg and delayed-callback scenarios. Reverse dependency scenarios
require `warm=0`.

The workload uses real `submit_remote_txs` batches. One peer contributes one
bounded batch and consumes one controller queue capability; peer batches may be
in flight concurrently. Sequential `submit_remote_tx` calls are not an admitted
production-shape profile. The same harness is used by
`scripts/cross_version_benchmark.py`.

## Artifact and analysis contract

Each capture produces one movable bundle containing the compressed raw Samply
profile, its presymbolication sidecar, CPU-run stdout/stderr, an independent
span run and stdout/stderr, a manifest and a deterministic summary. The
manifest stores bundle-relative artifact paths plus size and SHA-256. Analysis
verifies every artifact before reading it, so the capture binary may be removed
and the bundle may be moved without losing integrity.

The manifest binds:

- Git revision plus tracked-diff and complete-status digests;
- Cargo manifests/lockfile, harness/analyzer sources and binary SHA-256;
- scenario, terminal observation, commands, feature set and `prod` profile;
- Cargo, rustc and Samply versions, platform, machine, CPU identity and build
  flags.

The target window begins immediately before production-shaped submission and
ends after exact terminal callbacks quiesce. Fixture construction, warm-up,
shutdown and ordinary post-window reorg work are outside the CPU window;
`reorg_in_flight` deliberately includes the overlapping reorg.

CPU and span captures are separate executions. The analyzer accepts Samply
absolute or delta coordinates, rejects non-monotonic absolute coordinates and
crops samples to the emitted window. Only complete `threadCPUDelta` intervals
whose previous sample is already inside the window contribute CPU. The stack at
the interval-ending sample is resolved through the sidecar and accumulated into
deterministically sorted leaf and inclusive hotspot tables. These are hotspot
candidates, not exact async attribution.

The span execution reports the producer-owned, sorted span table with start
counts and lifetimes. The summary retains that table and its two totals. A
positive `tx_pool.ingress.remote_batch` span is required, preventing accidental
profiling of sequential per-transaction ingress. Span lifetimes describe the
separate low-overhead execution and must not be combined with CPU samples.

The observation binds exact callback and relay terminal counts, workload
identity, throughput, process CPU, target-completion p99, allocation traffic,
reorg overlap/latency and shutdown latency. Duplicate or generation-reset
terminals are rejected; RBF and reverse-dependency terminal sets are checked
against their scenario.

## Required checks

After changing the analyzer, harness or manifest format, run:

```sh
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
```

The analyzer canaries mechanically cover `prod` binary selection, equivalent
absolute/delta window cropping, deterministic hotspots, sidecar resolution,
movable bundles without binaries, artifact tampering, non-monotonic samples,
production-batch spans and exact terminal observations.

Before treating a profile as causal evidence, require a complete terminal
workload, target-window CPU samples, path-defining spans, deterministic
reanalysis and durable content-addressed storage. Final performance claims also
require a result-before-frozen candidate, freshly frozen `develop`, precommitted
scenarios, a noise rule and practical non-inferiority margins.
