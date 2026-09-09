# Tx-pool paired benchmark

Performance evidence has one workload executor and one comparison runner:

- `benches/profile_one_shot.rs` constructs and observes the production service;
- `scripts/cross_version_benchmark.py` freezes two binaries and runs paired A/B
  attempts;
- `scripts/profile.py` profiles the same executor.

Use this guide to run and interpret comparisons. [Profiling](PROFILING.md)
owns CPU, span, allocation and Console diagnostics; [performance](PERFORMANCE.md)
owns the final report. Timing cannot weaken correctness or independent concurrency.

## Prepare a comparison

Run from the repository root on a POSIX host with Python 3.11+, the repository
Rust toolchain and build dependencies. Stop competing owned builds, tests and
profilers. Use separate clean, committed checkouts and keep output outside them.
The runner builds serially with locked dependencies and Cargo profile `prod`.

Each checkout must contain the identical `profile_one_shot.rs` harness and its
`benches/profile_spans/mod.rs` and `benches/relay_batches/mod.rs` helpers. Porting an
older pool also requires a reviewed `cross-version-legacy-bench-adapter` feature;
the runner does not create that adapter. Commit these measurement-only changes
and preserve the underlying production commit and patch. Keep CKB-VM packages,
checksums and enabled features identical on both sides. The runner checks the
top-level harness hash directly; verify helper equality as part of preparing the
shared bundle.

Edit the two checkout paths and run:

```sh
python3 tx-pool/scripts/cross_version_benchmark.py --help

python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /absolute/path/to/prepared-baseline \
  --candidate-root /absolute/path/to/prepared-candidate \
  --output /tmp/txpool-comparison.json \
  --runs 10 --replicates-per-sample 4 \
  --scenario always_success,32000,100,8,4 \
  --scenario dependent_forest_10,32000,100,8,4
```

For a prepared legacy baseline, add
`--baseline-build-features cross-version-legacy-bench-adapter`.
This example demonstrates the interface; choose and freeze the actual matrix,
sample count and margins before collecting a result. A new output path must not
already contain a study; to continue that same study use its unchanged command
with `--resume`.

## Execution and output

The runner binds clean commits, Cargo/CKB-VM identities, the common harness,
host/toolchain, commands and binary hashes. Supplied executables require
`--baseline-binary-profile prod` / `--candidate-binary-profile prod`.
Pilots must agree on transaction bytes/hashes, declared cycles, script preflight
and runtime consensus. Measured attempts alternate AB/BA with balanced replicates.

The output JSON uses schema 10. `attempts[]` retains every command, raw output,
source side, stable attempt ID, corpus, terminal multiset, window and metrics.
The runner atomically checkpoints attempt start and outcome, then verifies source
and binary identity again at completion. Host-load snapshots accompany successes
and failures; load averages neither identify interference nor certify isolation.

`--resume` revalidates configuration and identities, reuses completed attempts and
runs only never-started attempts. An abandoned `running` attempt becomes a retained
failure. Resume cannot replace an interrupted or bad sample.
[measurement_process.py](../scripts/measurement_process.py) owns each POSIX process
group and cleans descendants on success, failure, timeout, Ctrl-C or SIGTERM.
Descendants must stay in that group; process escape and SIGKILL of the runner are
outside this guarantee.

## Measurement scopes

`terminal_completion_v2` measures submission through required callback/relay
terminals. Fixture generation, script preflight, warmup, exact-set validation,
p99 sorting and ordinary shutdown are outside the target window. In-flight reorg
intentionally overlaps it. Wall and monotonic elapsed must agree within
`max(1 ms, elapsed / 10,000)`; a harness change requires rebuilding both binaries.

| Metric | Scope and replicate reduction |
|---|---|
| Throughput | Target transactions divided by summed target elapsed time |
| Process CPU | Target-window CPU, summed across replicates |
| Peak RSS | Maximum process-lifetime peak, including setup/warmup/shutdown |
| P99 | Maximum of per-attempt p99 values; not a pooled percentile |
| Context switches | Process-lifetime counts, summed across replicates |
| Reorg / stop latency | Separately observed operations, not additive target phases |
| Allocation calls / bytes | Target-window traffic; only these metrics may rank an allocation-enabled study |

The bounded-batch stop observation ends after `service_started` becomes false and
the relay observer joins; service workers and publication can still be joining.
The legacy adapter observes broadcast exit signals plus observer join, then exits
without ordinary destructors; its post-window reorg return observes submission.
These ancillary metrics cannot rank full service join, persistence or completed
reconciliation. Pool budgets and process RSS are different quantities.

## Quality and decision rules

Choose populations, samples, margins and thresholds before measurement.
For a new population, declare a ladder and use `--calibration-only` for pilots;
select the first valid population meeting duration on both sides. Use fresh equal
corpora rather than looping in one pool and changing its cache state.

| Rule | Current CLI default |
|---|---|
| Every individual target window reaches the minimum | `--min-target-seconds 0.25` |
| Paired throughput ratio relative MAD stays within the limit | `--max-paired-mad-percent 1.5` |
| Each primary ratio interval's full width / median stays within the limit | `--max-ratio-interval-width-percent 4` |
| Requested pointwise confidence | `--confidence-level 0.95` |

Intervals use conservative exact-binomial order statistics for the median.
Their [statistical basis](https://itl.nist.gov/div898/software/dataplot/refman1/auxillar/mediancl.htm)
assumes stable independent sampling; balanced order and low MAD do not prove it.
Pointwise intervals do not establish simultaneous confidence across the matrix.
Summing short replicates cannot satisfy the individual-window rule. Allocation
studies omit the timing-duration gate; calibration always checks it.

| Decision | Meaning |
|---|---|
| `comparable` | The operational quality bounds passed |
| Interval excludes 1 | Supports that metric's pointwise direction, subject to A/A eligibility |
| Interval includes 1 | Direction unresolved; does not establish equivalence |
| `aa_equivalent` | All quality gates pass and all three complete primary intervals fit the declared A/A margin |
| `aa_equivalence_unresolved` | The margin is not established; not proof of inequivalence |
| `noisy`, `imprecise`, `short_target_window`, failure | Preserve the result and exclusion; no ranking |

A/A uses two explicit paths to the same binary hash, may use one clean checkout,
and requires `--comparison aa --aa-equivalence-margin-percent 2` for measurement.
Allocation instrumentation must be disabled; duration-only calibration can omit
the margin. A/A never ranks implementations. Practical equivalence/non-inferiority
requires the whole interval inside prospectively chosen margins.

Exit 2 means at least one row did not pass. `--allow-noncomparable` changes only
the exit status, never the recorded decision. Keep all attempts; do not select
favorable reruns, change populations on resume or widen thresholds after inspection.

## Workload integrity

Each peer's contiguous range is split by production relay count/byte bounds,
preserving order, peer and declared cycles. The legacy adapter uses its own
per-transaction ingress. Both sides require the same prepared workload bundle.
Warm and target phases validate exact callback/relay terminal sets. Unexpected
rejects, duplicates, missing terminals, corpus drift and invalid windows fail the
attempt; permitted reacceptance/unknown-parent cases are scenario-specific.

Fanout requires at least a parent and child, valid funding and a parent within
the 512,000-byte transaction limit. The current fixture maximum is 5,752 total
transactions; its parent is 511,992 bytes. Forward fanout waits for parent
acceptance before children; reverse workloads submit children first and require
`warm=0`. RBF requires equal target and warm counts. Functional validity does not
establish timing duration or repeatability.

For secp workloads, declared cycles come from an isolated canonical script
verifier. Preflighting every target through the measured controller's
`test_accept_tx` can change script-cache state unequally. Warmup uses separate
transaction keys. Matching corpora and low timing MAD cannot repair unequal
initial cache populations.

## RBF contract diagnostic

`--comparison-contract rbf-victim-notice-ablation` is an RBF-only A/B diagnostic.
Its candidate must be separately built with victim effects omitted; the flag
changes observer expectations, not production behavior. Keep that exact source
patch and both binary identities. Normal `protocol` comparisons retain all
required effects regardless of inherited diagnostic environment settings.

Successful diagnostic rows use `contract_diagnostic` and
`production_ranking_permitted=false`. A different-architecture baseline has other
source differences, so its ratio cannot isolate the cost of one omitted effect.
Only a verified single-change comparison can support that attribution.

## Maintenance gates

After changing the executor, runner or analyzer, run:

```sh
cargo check -p ckb-tx-pool --bench profile_one_shot --features profiling,allocation-observation
cargo test -p ckb-tx-pool --test profile_spans --features profiling
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
python3 -m unittest tx-pool/scripts/test_measurement_process.py
```

Final performance decisions require the candidate and baseline frozen before
measurement, the declared scenario matrix and all affected correctness and
concurrency gates.

## A/A and final delivery protocol

From the clean candidate root, build one uninstrumented binary:

```sh
cargo build --locked -p ckb-tx-pool --bench profile_one_shot \
  --profile prod --message-format=json-render-diagnostics
```

Select the `executable` from Cargo's `profile_one_shot` compiler-artifact record.
Use that exact binary on both sides (substitute its path below):

```sh
python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /absolute/path/to/prepared-candidate \
  --candidate-root /absolute/path/to/prepared-candidate \
  --baseline-binary /absolute/path/to/profile_one_shot \
  --candidate-binary /absolute/path/to/profile_one_shot \
  --baseline-binary-profile prod --candidate-binary-profile prod \
  --comparison aa --aa-equivalence-margin-percent 2 \
  --output /tmp/txpool-aa.json \
  --runs 24 --replicates-per-sample 4 \
  --initial-cooldown-seconds 30 --cooldown-seconds 5 \
  --scenario always_success,16000,1000,8,4
```

The final delivery study uses a clean candidate frozen after material source and
measurement-tool changes, and the reviewed develop basis
`cdfde29e45dfbe9443be66083241ebc1acca9fe6`. Record the actual prepared checkout
commits and complete input maps; the production basis alone is not a binary identity.
Repeat `--scenario` for all eight rows below in both A/A and develop A/B:

| Scenario | Target / warm | Workers / peers |
|---|---|---|
| `always_success` | 16,000 / 1,000 | 1 / 4 and 8 / 4 |
| `secp256k1` | 4,000 / 100 | 1 / 4 and 8 / 4 |
| `dependent_forest_10` | 16,000 / 1,000 | 8 / 4 |
| `fanout_reverse` | 5,752 / 0 | 8 / 4 |
| `rbf_pairs` | 16,384 / 16,384 | 8 / 4 |
| `reorg_in_flight` | 2,000 / 100 | 8 / 4 |

Use 24 pairs with four fresh replicates per side, one pilot per side per row,
30 seconds initial cooldown, five seconds after every attempt and a 180-second
attempt timeout. Keep the default 0.25-second individual-window minimum, 1.5%
throughput MAD limit, 95% pointwise intervals and 4% interval-width limit.
A/A requires all three primary intervals within [0.98, 1.02]. Formal A/B direction
requires that row's new A/A qualification, complete A/B quality and an interval
excluding one. Disable profiling, Console and allocation observation.

Retain all eight rows and all failed/short/noisy/imprecise attempts. Failed pilots
prevent pairing; the first paired failure ends its row. Do not replace samples,
extend a favorable subset or relax thresholds. The planned maximum is 1,552
attempts per study. Preserve actual exit status: exit 2 can carry valid evidence
of quality failure and is not a performance pass. Audit retained outputs, source
and binary identities before filling the [report](PERFORMANCE.md).
