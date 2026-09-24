# Tx-pool benchmarks

[cross_version_benchmark.py](../scripts/cross_version_benchmark.py) compares two
frozen builds of the production-service executor, `benches/profile_one_shot.rs`.
Use [profiling](PROFILING.md) for CPU, span, allocation and task diagnostics, or
[composed validation](MAINTENANCE.md#repeat-composed-workloads-and-replay-sequences)
for resource-release checks and state-sequence replay. Keep study plans, raw
captures and source-bound results together outside the checkout.

## Prepare a comparison

Run from the repository root on a POSIX host with Python 3.11+, the repository
Rust toolchain and build dependencies. Stop competing owned builds, tests and
profilers. Use separate clean, committed checkouts and keep output outside them.
The runner builds serially with locked dependencies and Cargo profile `prod`.

Both checkouts need identical Rust harness sources under `benches/` and matching
CKB-VM packages, checksums and features. An older pool also needs a reviewed,
committed `cross-version-legacy-bench-adapter`; preserve its production basis and
patch. The runner does not create the adapter. It freezes the complete harness,
process runner, window parser, observation decoder, rejection verifier and
scenario validator. Current comparison records use schema 15; schema 14 and
earlier evidence retain their original runner and source attribution.

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

## A/A comparison

Build each arm once from its clean checkout. The shared build tool records the
source, Cargo command, features, toolchain and selected compiler artifact:

```sh
python3 tx-pool/scripts/benchmark_build.py \
  --root /absolute/path/to/prepared-candidate --bench profile_one_shot \
  --target-dir /tmp/candidate-bench-target --output /tmp/candidate-build.json
```

Use that build receipt on both sides of its A/A control:

```sh
python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /absolute/path/to/prepared-candidate \
  --candidate-root /absolute/path/to/prepared-candidate \
  --baseline-build-receipt /tmp/candidate-build.json \
  --candidate-build-receipt /tmp/candidate-build.json \
  --comparison aa --aa-equivalence-margin-percent 2 \
  --output /tmp/txpool-aa.json \
  --runs 24 --replicates-per-sample 4 \
  --initial-cooldown-seconds 30 --cooldown-seconds 5 \
  --scenario always_success,16000,1000,8,4
```

Choose and freeze the workload and margins using the
[quality rules](#quality-and-decision-rules) before A/A and A/B.
Repeat for the baseline, including its adapter features in both the build and
A/A commands. A/B then uses the two build receipts and their controls:

```sh
python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /absolute/path/to/prepared-baseline \
  --candidate-root /absolute/path/to/prepared-candidate \
  --baseline-build-receipt /tmp/baseline-build.json \
  --candidate-build-receipt /tmp/candidate-build.json \
  --baseline-aa-result /tmp/baseline-aa.json --candidate-aa-result /tmp/txpool-aa.json \
  --output /tmp/txpool-ab.json --runs 24 --replicates-per-sample 4 \
  --initial-cooldown-seconds 30 --cooldown-seconds 5 \
  --scenario always_success,16000,1000,8,4
```

Keep the corresponding `--baseline-build-features` / `--candidate-build-features`
when an arm needs an adapter. The control must match its own arm's source,
binary, build contract, host, corpus, observation contract, replicate count,
cooldowns, timeout and quality thresholds. A/A and A/B may use different sample
counts and order seeds. A/B production sources may differ. Each control is frozen
by file hash before A/B, and its scheduled raw attempt outputs are reparsed and
its statistics recomputed; its saved summary is not the authority.
Checkout paths may differ when the source inputs and executable bytes are identical.

Without both applicable controls, A/B still saves diagnostic observations and
operational quality results, but does not authorize timing rankings. Missing
scenario rows or unresolved controls remain visible. An incompatible supplied
control is rejected before collection.

## Execution and output

The runner records source/build/host identities, commands and binary hashes.
Supplied builds require `--baseline-build-receipt` / `--candidate-build-receipt`.
Optional `--baseline-binary` / `--candidate-binary` select a copied executable;
its hash and size must match the receipt. Automatic A/B builds use the same Cargo
build operation. A receipt establishes the recorded producer-to-artifact link;
it is not a signature against deliberate falsification of the evidence.
Pilots must agree on transaction bytes/hashes, declared cycles, script preflight
and runtime consensus. Measured attempts use randomized, balanced AB/BA blocks.
`--order-seed` and the complete resulting schedule are part of the frozen
configuration; changing either requires a new study.

Schema 15 `attempts[]` retains commands, raw output, source side, attempt ID,
corpus, window and metrics; terminal evidence remains in the raw observation.
Start/outcome checkpoints are atomic; completion rechecks source and binary
identity. Host-load snapshots accompany
every outcome but cannot certify isolation. Host identity includes node name and
CPU model alongside the software/toolchain fields.

`--resume` revalidates configuration and identities, reuses completed attempts and
runs only never-started attempts. An abandoned `running` attempt becomes a retained
failure. Resume cannot replace an interrupted or bad sample.
[measurement_process.py](../scripts/measurement_process.py) owns each POSIX process
group and cleans descendants on success, failure, timeout, Ctrl-C or SIGTERM.
Descendants must stay in that group; process escape and SIGKILL of the runner are
outside this guarantee.

The executor streams committed rejections and warnings/errors.
`BENCH_FAILURE_TERMINALS` retains partial terminals with corpus index, phase and
original peer; `BENCH_REJECTION_CAPTURE` closes the log after runtime cleanup.
Missing records, count disagreement, output failure, service ERROR or unexpected
timing-workload rejection invalidates the evidence. An early fixture failure can
precede corpus construction.

The common logger enables Debug on both sides, so other enabled Debug call sites
can evaluate arguments even when their records are not formatted. This is part
of the harness cost. Older sources without rejection producers retain an explicit
diagnostic gap; later successful runs cannot supply missing causes.

## Measurement scopes

`terminal_completion_v3` measures submission through required callback/relay
terminals. Fixture generation, script preflight, warmup, exact-set validation,
p99 sorting and ordinary shutdown are outside the target window. In-flight reorg
intentionally overlaps it. Monotonic `Instant` reads bound elapsed time; process
CPU reads surround them. Separate wall anchors qualify
[profile alignment](PROFILING.md#read-the-result-correctly), without changing
monotonic throughput validity. Harness changes require rebuilding both binaries.

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

Shutdown latency includes releasing caller handles, joining runtime guards and
the relay observer, network shutdown and persistence to an isolated temporary file.
It is outside the target window and differs from historical stop-request timing.
The legacy post-window reorg return observes submission, not reconciliation.
RSS includes all replicates and outliers; neither its mean nor maximum is a memory
bound. Use the definitions recorded by each study's schema.

The executor emits one schema-3 `TX_POOL_PROFILE_OBSERVATION` for a successful
workload. Both the paired runner and profile analyzer use the
[observation decoder](../scripts/measurement_observation.py) for exact fields,
typed metrics, CPU totals, throughput and callback/relay terminal identity.
Unknown-parent evidence must be a canonical peer/hash multiset. The validated
adapter decides whether RBF victims produce rejection notices. Structured clock,
corpus, build, process-resource and failure records retain their own scopes.
The two tools apply their timing and profile qualification rules separately.

This contract replaces the duplicate text result, terminal summary and bare
clock marker. Full-precision JSON throughput is checked against target/elapsed
with relative tolerance `1e-12`; the former three-decimal allowance is removed.
Malformed-record diagnostics may therefore differ. Both comparison arms must
use the new frozen harness; deleted post-window serialization can also affect
process-lifetime resource observations. Historical captures require their
matching immutable decoder and runner and cannot resume under the new schema.

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
| Interval excludes 1 | Supports that metric's pointwise direction only when its `metric_quality.ranking_permitted` is true |
| Interval includes 1 | Direction unresolved; does not establish equivalence |
| `aa_equivalent` | All quality gates pass and all three complete primary intervals fit the declared A/A margin |
| `aa_equivalence_unresolved` | The margin is not established; not proof of inequivalence |
| `noisy`, `imprecise`, `short_target_window`, failure | Preserve the result and exclusion; no ranking |

A/A uses two build receipts for the same binary hash, may use one clean checkout,
and requires `--comparison aa --aa-equivalence-margin-percent 2` for measurement.
Allocation instrumentation must be disabled; duration-only calibration can omit
the margin. A/A never ranks implementations. Practical equivalence/non-inferiority
requires the whole interval inside prospectively chosen margins.

The three primary metrics are throughput, target process CPU and mean peak RSS.
Overall qualification remains conjunctive. `metric_quality` also preserves each
metric's precision and A/A disposition, so an unresolved RSS interval does not
erase the measured throughput evidence or become an overall pass.
For A/B, `production_ranking_permitted` requires operational quality and both
applicable A/A controls for all three primary metrics. Allocation studies have
no timing ranking permission; only allocation metrics with a sufficiently narrow
interval can receive their own ranking permission.

Exit 2 means at least one row did not pass. `--allow-noncomparable` changes only
the exit status, never the recorded decision. Keep all attempts; do not select
favorable reruns, change populations on resume or widen thresholds after inspection.
For diagnostic A/B without A/A, a zero exit status certifies the operational
quality gates only; inspect the separate ranking permissions before drawing a
directional conclusion.

## Workload integrity

Each peer's contiguous range is split by production relay count/byte bounds,
preserving order, peer and declared cycles. The legacy adapter uses its own
per-transaction ingress. Both sides require the same prepared workload bundle.
Warm and target phases validate exact callback/relay terminal sets. Unexpected
rejects, duplicates, missing terminals, corpus drift and invalid windows fail the
attempt; permitted reacceptance/unknown-parent cases are scenario-specific.

The native executor parses each scenario into one workload shape used by fixture
construction, cycle preflight and submission policy. Both Python entry points
share the same CLI gates: target + warm ≤ 65,536, positive target/workers/peers,
nonnegative warm, complete forward-forest chains in each phase, and the reverse,
RBF and fanout rules below. A reverse forest may retain an incomplete final chain.
Fan-in must be positive and its transaction must fit 512,000 bytes (at most
11,631 inputs with this fixture). Small native fixtures verify the encoded size
model before constructing the requested population.

Funding limits belong to the native fixture producer: one real funding output
establishes its initial DAO capacity and encoded genesis size; checked growth
rejects overflow before allocating the full output vector. This uses the actual
system cells and initial issuance, which the Python validator does not duplicate.
It is an encoding/capacity check, not a guarantee against allocation failure.

Single-parent fanout supports 2–5,752 transactions; its largest parent is 511,992
bytes, below the 512,000-byte limit. Forward fanout waits for parent acceptance.
`fanout_reverse` uses `warm=0` and reports `BENCH_STRESS_RESULT`: accepted,
rejected and unresolved hashes, ownership checks and reset-induced relay gaps.
It is a capacity/recovery diagnostic, with no throughput result; the legacy
100-entry orphan pool cannot accept all 5,751 missing-parent children.

Use `fanout_ready_64_reverse` for paired submission/readiness/recovery throughput.
Each independent cohort contains 64 children and their parent. Before submitting
children, the public `get_tx_pool_info` query must observe `orphan_size == 0`;
after submitting only the children, it must observe exactly 64 before the parent
is submitted. The next cohort waits for all previous callbacks and an empty
orphan pool. Queries yield between attempts and have a 30-second deadline.
Queries count inside the target window and have version-dependent cost;
`BENCH_READINESS` records their count, barriers and duration. The result measures
the complete workload, including those queries. No sleep pads the duration.

Target and warm counts must each be multiples of 65. Peer partitioning restarts
for each 64-child submission and for the single parent (peer 1). Exact relay peer
identity and actual input-parent sets remain checked, and any generation reset
invalidates the exact-stream timing attempt. Other reverse workloads require
`warm=0`; RBF requires equal target and warm counts.

`rbf_pairs_windowed` preserves peer partitions and submits at most 1,024 additional
transactions per peer before awaiting acceptance. Both adapters include these
waits and the partial final window in timing. The `rbf_pairs` burst can exceed
pipeline capacity; ingress completion is not acceptance, and refusal must leave
the accepted victim intact. Windowed and burst results measure different workloads.

For secp workloads, declared cycles come from an isolated canonical script
verifier. Preflighting every target through the measured controller's
`test_accept_tx` can change script-cache state unequally. Warmup uses separate
transaction keys. Matching corpora and low timing MAD cannot repair unequal
initial cache populations.

## Refusal and retry preflight

Before timing a changed collector or admission workload, exercise a real refusing
service with the uninstrumented production-profile executable:

```sh
/absolute/path/to/profile_one_shot rbf_pressure 32768 32768 8 4 \
  > /tmp/txpool-rbf-pressure.log 2>&1
python3 tx-pool/scripts/rejection_diagnostics.py \
  /tmp/txpool-rbf-pressure.log --pressure
```

This current-controller diagnostic pauses computation after warmup, fills real
peer ingress, observes refusal before resume, then waits for all first-offer
terminals. It checks every refused victim remains, retries using the same peers
in bounded batches, and verifies exact final callback/relay/live populations.
Every refusal needs a committed cause, source, phase, original peer/index and
resource observation. Resolution may refuse further candidates after resume;
the complete first-offer population determines the expected survivors. The run
emits `BENCH_RBF_PRESSURE` and no throughput result.

Retain a negative native check as well: disable or omit the rejection producer
in an isolated, identified binary and confirm the collector rejects missing
causes. Preserve that source, executable, command and original log. A synthetic
parser fixture alone does not verify the native producer-to-log path. Keep
pressure diagnostics separate from paired timing, and preserve all failed
preflights before correcting their cause.

## Measure template transaction selection

`benches/packing_one_shot.rs` calls the production selector through
`benches/packing/current_adapter.rs`. The default-off `packing-bench` feature
enables this adapter without tracing. Build from a frozen checkout:

```sh
python3 tx-pool/scripts/benchmark_build.py \
  --root /absolute/path/to/frozen-checkout --bench packing_one_shot \
  --target-dir /tmp/packing-target --output /tmp/packing-build.json
```

The builder automatically includes `packing-bench`. Supply its build receipt:

```sh
python3 tx-pool/scripts/packing_benchmark.py capture \
  --root /absolute/path/to/frozen-checkout \
  --build-receipt /tmp/packing-build.json \
  --shape mixed --fees cpfp --count 4096 --limit budget:595000:3500000000 \
  --repeats 5 --warm 2 --output /tmp/packing-mixed-cpfp

python3 tx-pool/scripts/packing_benchmark.py replay /tmp/packing-mixed-cpfp
```

The direct executable arguments are `SHAPE FEES COUNT LIMIT REPEATS WARM`.
The byte limit is the **remaining transaction budget**, after cellbase, extension,
proposals and uncles. The example leaves 2,000 of the default 597,000 bytes for
those parts without checking their fit. DAO and final serialization are excluded.

| Parameter | Contract |
|---|---|
| Shapes | `independent`; `chain` in cohorts of depth 64; `fanout` in cohorts of one producer and 64 children; `diamond` in cohorts of four; `mixed` in cohorts of eight with input and read-only cell-dep edges |
| Fees | `equal`, `varied`, `cpfp`; current selection always uses its production ancestor score, with arrival and hash as tie-breaks. `verify_ordering` is not a packing strategy. |
| Population | 1–16,384 accepted transactions; a final partial cohort is allowed; all proposals are present in the real `ProposalView` |
| Limits | `budget:BYTES:CYCLES` supplies explicit transaction limits. `all` uses corpus totals; `bytes` or `cycles` reduces the corresponding total to one third; `both` uses two thirds of each; `zero` tests rejection. Fractional/all regimes are algorithm stress cases and may exceed consensus limits. |
| Repetition | 1–128 measured calls and 0–32 warm calls; two untimed preflight calls additionally prove full-capacity eligibility and establish the repeated-result reference |

The v2 corpus uses `max_ancestors = 64`, below the production default of 1000.
Depth-1000 or proposal-phase studies need a separately identified corpus.

Fixtures use real transactions and dependency points with controlled fee/cycle
metadata, without admission or VM work. Preflight checks full-capacity fit;
every result checks membership, uniqueness, parent-first order, exact totals and
limits. Current order must repeat exactly. Legacy equal-rank iteration may vary;
each call still passes semantic checks and records order/set/fee/capacity changes.
An exhaustive maximum-fee oracle covers at most 16 entries, not larger fixtures.

Each `template_selection_v2` window includes `Selection::new`, `pack_transactions`
and selection-state destruction, including cold graph construction/destruction.
The adapter borrows prepared owners and bypasses the template graph cache.
Fixture/source preparation, Store capture/locks, optional packing, result
validation/serialization/destruction and DAO/template work are excluded.
Cache-hit or invalidation studies need a separate adapter/contract; older receipts
may also include owner-vector cloning. Each call has monotonic timing and wall
anchors for profiling.

Capture reports templates/s, selected transaction/byte/cycle/fee totals and
utilization ranges, result digests, preparation time and logical graph size.
Logical entries/edges do not measure allocation bytes or RSS. Returned entries
retain production payload ownership; admission TPS is a separate measurement.

For develop, register `benches/packing/develop_adapter.rs` as a hidden, default-off
`packing_bench` module in the frozen baseline. It prepares the real `PoolMap`,
rejects eviction/duplicate insertion, and times `TxSelector::new(...).txs_to_commit(...)`.
Keep the adapter and dependency patches. Preserve and report the cost placement:
develop maintains its index before selection; current cold selection derives it
inside the window.

Match fixture digest, shape/fees/count, limits, ancestor policy and build/runtime
configuration. Report selected fees and capacity usage alongside speed: equal-rate
ties can select different valid sets. Predeclare A/A and balanced A/B process
schedules; repeated calls in one process are not independent samples, and a
single-capture median gives no confidence interval or acceptance verdict.
`--min-selection-ns` freezes a per-call floor (default 1 ms); short/empty selections
are `functional_only` with `templates/s = null`. Admission/profile analyzers reject
packing markers; external CPU profiles must validate each selection's wall alignment.

The capture directory contains raw logs and a source/binary/host/harness receipt,
including the Cargo build receipt. New captures use schema 2; schema 1 captures
remain replayable as historical observations without upgrading their provenance.
Failed and interrupted attempts remain failed; replay verifies log sizes/hashes
and reproduces the observation without the original binary or checkout. Process
lifetime/cleanup uses the same bounded POSIX helper as other benchmarks.

For allocations, build separately with `--allocation-observation enabled`
and capture with `--observation allocation`. Both tools derive the same effective
features; extra adapter features must match via the builder's `--features` and
capture's `--build-features`. Fixture, source preparation and each
selection have separate windows; output is `allocation_only` without templates/s.
Counts cover process-wide `alloc`/`realloc` requests, including full realloc sizes,
failed requests and other threads. They measure neither retained bytes nor RSS.
Result validation/destruction stays outside selection. Do not use these timings
to rank production builds.

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

### Calibration observations

Run calibration holdouts alone in fresh processes with the production VM backend,
recording source, binary, toolchain and CPU identities. `calibration-observation`
keeps their wall-clock assertions out of ordinary concurrent unit suites.

```sh
cargo nextest run --locked --cargo-profile prod -p ckb-tx-pool \
  --features calibration-observation \
  -E 'test(observe_calibration_against_holdout_workloads)' --run-ignored only \
  --test-threads 1 --retries 0 --success-output immediate
```

Holdouts cover different instructions, program sizes and cycle counts. They do
not bound every script, provider delay or later machine load. Retain raw results
and machine identities, including failed attempts, when comparing hosts.

### Harness checks

After changing the executor, runner or analyzer, run:

```sh
make clippy ALL_FEATURES=profiling,ckb-tx-pool/allocation-observation
cargo nextest run -p ckb-tx-pool --features profiling,allocation-observation,packing-bench \
  --test profile_contract --test packing_contract
python3 -m unittest discover -s tx-pool/scripts -p 'test_*.py'
```

Final performance decisions require the candidate and baseline frozen before
measurement, the declared scenario matrix and all affected correctness and
concurrency gates.

After workload or observer changes, exercise both version adapters with small
and sustained populations, malformed cohort boundaries, and the single-parent
capacity stress. Parser tests alone cannot establish native workload validity.
