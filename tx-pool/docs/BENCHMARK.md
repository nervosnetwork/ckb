# Tx-pool benchmarks

Admission performance evidence uses one workload executor and one comparison runner:

- `benches/profile_one_shot.rs` constructs and observes the production service;
- `scripts/cross_version_benchmark.py` freezes two binaries and runs paired A/B
  attempts;
- `scripts/profile.py` profiles the same executor.

Use this guide to run and interpret comparisons. [Profiling](PROFILING.md)
owns CPU, span, allocation and Console diagnostics; [performance](PERFORMANCE.md)
owns the final report. Timing cannot weaken correctness or independent concurrency.

For repeated work within one service generation, resource-release checkpoints,
latency tails and deterministic state-sequence replay, use the
[composed validation runs](MAINTENANCE.md#repeat-composed-workloads-and-replay-sequences).
They share the OS residency probe under `benches/resource_phases/memory.rs` but
remain separate from the uninstrumented performance comparison below.

## Prepare a comparison

Run from the repository root on a POSIX host with Python 3.11+, the repository
Rust toolchain and build dependencies. Stop competing owned builds, tests and
profilers. Use separate clean, committed checkouts and keep output outside them.
The runner builds serially with locked dependencies and Cargo profile `prod`.

Each checkout must contain the identical `profile_one_shot.rs` harness and all
Rust helpers under `benches/`. Porting an
older pool also requires a reviewed `cross-version-legacy-bench-adapter` feature;
the runner does not create that adapter. Commit these measurement-only changes
and preserve the underlying production commit and patch. Keep CKB-VM packages,
checksums and enabled features identical on both sides. The runner hashes the
complete Rust harness bundle, including newly added modules, and independently
freezes its process runner and shared measurement-window parser.

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
and runtime consensus. Measured attempts use randomized, balanced AB/BA blocks.
`--order-seed` and the complete resulting schedule are part of the frozen
configuration; changing either requires a new study.

The output JSON uses schema 11. `attempts[]` retains every command, raw output,
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

`terminal_completion_v3` measures submission through required callback/relay
terminals. Fixture generation, script preflight, warmup, exact-set validation,
p99 sorting and ordinary shutdown are outside the target window. In-flight reorg
intentionally overlaps it. Duration uses one pair of monotonic `Instant` reads;
process CPU reads immediately surround those boundaries. Bracketed wall-clock
anchors outside the window map it onto the profiler's Unix timeline. The mapped
end minus start must equal the monotonic duration exactly. An independent end
anchor reports wall adjustments and read uncertainty. These qualify profile
alignment, not throughput: a wall-clock adjustment cannot invalidate monotonic
elapsed time. A harness change requires rebuilding both binaries.

| Metric | Scope and replicate reduction |
|---|---|
| Throughput | Target transactions divided by summed target elapsed time |
| Process CPU | Target-window CPU, summed across replicates |
| Mean peak RSS, primary | Arithmetic mean of every replicate's process-lifetime peak, including setup/warmup/shutdown |
| Maximum peak RSS, diagnostic | Maximum of those same retained peaks |
| P99 | Maximum of per-attempt p99 values; not a pooled percentile |
| Context switches | Process-lifetime counts, summed across replicates |
| Reorg / stop latency | Separately observed operations, not additive target phases |
| Allocation calls / bytes | Target-window traffic; only these metrics may rank an allocation-enabled study |

The bounded-batch stop observation ends after `service_started` becomes false and
the relay observer joins; service workers and publication can still be joining.
The legacy adapter observes broadcast exit signals plus observer join, then exits
without ordinary destructors; its post-window reorg return observes submission.
These ancillary metrics cannot rank full service join, persistence or completed
reconciliation. Pool budgets and process RSS are different quantities. The RSS
mean includes every replicate and its outliers; it estimates typical peak memory
per execution. The maximum remains available to expose observed tails. Neither
statistic establishes a universal memory bound. Schema 11 is a prospective
estimand change and cannot retroactively qualify a schema 10 study.

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

The three primary metrics are throughput, target process CPU and mean peak RSS.
Overall qualification remains conjunctive. `metric_quality` also preserves each
metric's precision and A/A disposition, so an unresolved RSS interval does not
erase the measured throughput evidence or become an overall pass.

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

Single-parent fanout requires at least a parent and child, valid funding and a parent within
the 512,000-byte transaction limit. The current fixture maximum is 5,752 total
transactions; its parent is 511,992 bytes. Forward fanout waits for parent
acceptance before children. `fanout_reverse` is a separate capacity/recovery
stress workload with `warm=0`: it reports `BENCH_STRESS_RESULT` with accepted,
rejected and unresolved hashes, duplicate/ownership checks, and whether reset
made relay history incomplete. It emits no accepted-throughput result. The
legacy pool's 100-entry orphan capacity does not support an all-accepted contract
for 5,751 missing-parent children.

Use `fanout_ready_64_reverse` for paired submission/readiness/recovery throughput.
Each independent cohort contains 64 children and their parent. Before submitting
children, the public `get_tx_pool_info` query must observe `orphan_size == 0`;
after submitting only the children, it must observe exactly 64 before the parent
is submitted. The next cohort waits for all previous callbacks and an empty
orphan pool. Queries yield between attempts and have a 30-second deadline.
Their cost is inside the target window. `BENCH_READINESS` records the fixed
policy, query count, completed barriers, and cumulative query/wait duration.
The pool-info implementations have different costs in the two versions, so this
measures the complete workload including public queries, not isolated orphan
recovery or ingress throughput. No sleep pads the duration.

Target and warm counts must each be multiples of 65. Peer partitioning restarts
for each 64-child submission and for the single parent (peer 1). Exact relay peer
identity and actual input-parent sets remain checked, and any generation reset
invalidates the exact-stream timing attempt. The previously attempted ungated
cohort did not ensure orphan registration before parent recovery; its failed
native evidence is retained and is not comparable with this new workload.
Other reverse workloads require `warm=0`. RBF requires equal target and warm
counts. Functional validity does not establish timing duration or repeatability.

The dedicated relay observer drains on the production signal and retains a
bounded sparse-flow fallback. Early failures preserve partial callback and relay
terminals instead of reporting only the acceptance timeout.

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

## Measure template transaction selection

`benches/packing_one_shot.rs` directly executes the production transaction
selection used while building a block template. Its fixture adapter lives in
`benches/packing/current_adapter.rs`. The default-off `packing-bench` feature
compiles that adapter inside the private authority module and exposes it through
one hidden export so the separate benchmark executable can call the real selector.
Ordinary builds exclude the adapter; the feature enables no tracing. Build an
uninstrumented executable from a frozen checkout:

```sh
cargo build --locked -p ckb-tx-pool --bench packing_one_shot \
  --profile prod --features packing-bench --message-format=json-render-diagnostics \
  > /tmp/packing-build.jsonl
```

Select the `executable` from Cargo's `packing_one_shot` compiler-artifact record,
preserve the build log, and supply its absolute path:

```sh
python3 tx-pool/scripts/packing_benchmark.py capture \
  --root /absolute/path/to/frozen-checkout \
  --binary /absolute/path/to/packing_one_shot --binary-profile prod \
  --shape mixed --fees cpfp --count 4096 --limit budget:595000:3500000000 \
  --repeats 5 --warm 2 --output /tmp/packing-mixed-cpfp

python3 tx-pool/scripts/packing_benchmark.py replay /tmp/packing-mixed-cpfp
```

The direct executable arguments are `SHAPE FEES COUNT LIMIT REPEATS WARM`.
The example's explicit byte value is a declared **remaining transaction budget**;
it leaves 2,000 bytes relative to the default 597,000-byte maximum. Replace it
with the actual remainder after cellbase, extension, proposals and uncles when
studying a specific template. This benchmark does not establish that those
parts fit the allowance, or execute DAO calculation and final serialization.

| Parameter | Contract |
|---|---|
| Shapes | `independent`; `chain` in cohorts of depth 64; `fanout` in cohorts of one producer and 64 children; `diamond` in cohorts of four; `mixed` in cohorts of eight with input and read-only cell-dep edges |
| Fees | `equal`, `varied`, `cpfp`; current selection always uses its production ancestor score, with arrival and hash as tie-breaks. `verify_ordering` is not a packing strategy. |
| Population | 1–16,384 accepted transactions; a final partial cohort is allowed; all proposals are present in the real `ProposalView` |
| Limits | `budget:BYTES:CYCLES` supplies explicit transaction limits. `all` uses corpus totals; `bytes` or `cycles` reduces the corresponding total to one third; `both` uses two thirds of each; `zero` tests rejection. Fractional/all regimes are algorithm stress cases and may exceed consensus limits. |
| Repetition | 1–128 measured calls and 0–32 warm calls; two untimed preflight calls additionally prove full-capacity eligibility and establish the repeated-result reference |

The v2 corpus uses `max_ancestors = 64`; the production default is 1000. Its
depth-64 chain results do not establish performance at the production ancestry
boundary. A depth-1000 or proposal-phase study needs a separately identified
corpus/contract, preserving the original captures and timing gates.

Fixtures contain real serialized transactions and resolved dependency points.
Fees and cycles are controlled accepted metadata, without admission or VM work.
The harness verifies all entries fit at full corpus capacity, then checks every
result for membership, duplicate selection, parent-first ordering, exact byte/
cycle/fee metadata and limits. Current output order must repeat exactly. The old
selector's HashSet-driven equal-rank iteration can change partial selections.
Each call must still pass all semantic checks; order, set and fee/capacity
variation are recorded against the untimed preflight result. A small-fixture exhaustive oracle reports maximum obtainable fee
for at most 16 entries. Large-fixture optimality is not inferred from utilization.

The `template_selection_v2` window includes `Selection::new`,
`pack_transactions`, and selection-state destruction. The current adapter borrows
its prepared owner slice; older source receipts may instead clone that vector.
Every current call compiles the graph inside the window, including cold
construction and destruction. This adapter does not use the template loop's graph
cache. Cache-hit, invalidation and retained-memory studies need a separately
identified adapter and contract, with source renewal inside the stated window.
The v2 window excludes
fixture construction, source preparation, capture/store locks, optional-content
packing, result validation/serialization/destruction, and final DAO/template
work. Each call has a separate monotonic window with the shared bracketed clock
anchors. Wall corrections affect profile alignment, not selection elapsed time.

Capture reports templates/s, per-call selected transaction/byte/cycle/fee totals, capacity
utilization and their min/median/max ranges, ordered/set digests, fixture/source
preparation nanoseconds and
logical retained entries/causal edges/cell-dep edges. These counts do not estimate
retained allocation bytes or whole-process RSS. Returned entries retain the same
resolved payload ownership as production. Admission TPS is a separate contract.

For develop, place the same shared harness in a reviewed, frozen baseline and
register `benches/packing/develop_adapter.rs` as its hidden, default-off
`packing_bench` module. That adapter populates the actual `PoolMap` during reported
source preparation and calls `TxSelector::new(...).txs_to_commit(...)` for each
window, matching `TxPool::package_txs`. It rejects fixture eviction or duplicate
insertion. Retain this source patch and necessary benchmark dev-dependency/lock
changes. Do not rebuild the old maintained index inside each selection window,
or move the current derived-index construction outside it: report the different
cost placement alongside selection results.

Both versions must use identical fixture digest, shape/fees/count, limits,
ancestor policy and runtime/build configuration. Selected fee and capacity usage
must accompany speed comparisons; equal-rate ties can choose different valid
sets. Predeclare an A/A then balanced A/B process schedule, preserving every
attempt and failure. Repeats within one process are not independent samples.
The single-capture median is descriptive and supplies no confidence interval or
performance acceptance verdict. `--min-selection-ns` freezes a per-call timing
floor (default 1 ms); shorter or empty selections are `functional_only` and their
templates/s field is absent (`null`). Increase the declared workload for timing
rather than treating a tiny/zero-budget canary as a performance result.
Admission/profile analyzers intentionally reject
these separate markers. For external CPU profiling, use the saved per-selection
windows and require their individual wall-alignment checks.

The capture directory contains raw logs and a source/binary/host/harness receipt.
Failed and interrupted attempts remain failed; replay verifies log sizes/hashes
and reproduces the observation without the original binary or checkout. Process
lifetime/cleanup uses the same bounded POSIX helper as other benchmarks.

To observe allocations, build a separate executable with
`--features packing-bench,allocation-observation`, then capture with
`--observation allocation`. The shared admission allocator counts process-wide
`alloc`/`realloc` requests and requested bytes, with separate windows for fixture
construction, source preparation and each selection. Allocation output is marked
`allocation_only` and has no templates/s value. These are cumulative requests,
including full realloc sizes and possible failed requests, not retained bytes or
RSS; other process threads can contribute. Returned-result destruction and result
validation remain outside the selection window. Allocation instrumentation must
not be enabled for timing comparisons.

After editing the packing harness or adapter, run its production-path contract
matrix and parser/artifact rejection tests, plus the applicable maintenance
gates below. Native small and sustained runs must exercise both adapters before
using comparison results:

```sh
cargo nextest run -p ckb-tx-pool --features packing-bench --test packing_contract
cargo clippy -p ckb-tx-pool --all-targets --features packing-bench -- -D warnings
python3 -m unittest discover -s tx-pool/scripts -p test_packing_benchmark.py
```

## Maintenance gates

After changing the executor, runner or analyzer, run:

```sh
cargo clippy -p ckb-tx-pool --all-targets --features profiling,allocation-observation -- -D warnings
cargo nextest run -p ckb-tx-pool --features profiling --test profile_spans --test measurement_clock --test relay_batches
python3 -m unittest tx-pool/scripts/test_profile.py
python3 -m unittest tx-pool/scripts/test_cross_version_benchmark.py
python3 -m unittest tx-pool/scripts/test_measurement_process.py
```

Final performance decisions require the candidate and baseline frozen before
measurement, the declared scenario matrix and all affected correctness and
concurrency gates.

After workload or observer changes, exercise both version adapters with small
and sustained populations, malformed cohort boundaries, and the single-parent
capacity stress. Parser tests alone cannot establish native workload validity.

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
measurement-tool changes, and a reviewed, explicitly recorded develop basis.
Inspect upstream changes before freezing both sides and account for dependency
differences as well as production Rust. Record the actual prepared checkout
commits and complete input maps; the production basis alone is not a binary identity.
Before collecting paired results, calibrate the following population ladders in
ascending order on both frozen binaries. Select the first population whose two
pilots satisfy all functional gates and reach 0.40 seconds individually. This
prospective calibration margin leaves room above the unchanged 0.25-second
measurement minimum. Keep every calibration result; a short or failed formal
attempt cannot select a new population or be replaced. Repeat `--scenario` for
all eight selected rows in both A/A and develop A/B:

| Scenario | Target ladder; warm | Workers / peers |
|---|---|---|
| `always_success` | 16,000 → 32,000 → 64,000; 1,000 warm | 1 / 4 and 8 / 4 |
| `secp256k1` | 4,000 → 8,000 → 16,000; 100 warm | 1 / 4 and 8 / 4 |
| `dependent_forest_10` | 16,000 → 32,000 → 64,000; 1,000 warm | 8 / 4 |
| `fanout_ready_64_reverse` | 1,040 → 2,080 → 4,160 → 8,320 → 16,640 → 33,280; 0 warm | 8 / 4 |
| `rbf_pairs` | 16,384 → 32,768; warm equals target | 8 / 4 |
| `reorg_in_flight` | 2,000 → 4,000 → 8,000; 100 warm | 8 / 4 |

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
