# tx-pool Criterion Benchmark

The benchmark is evidence for, not an alternative definition of, the canonical
[`architecture-contract.json#/optimization_goal`](../architecture-contract.json#/optimization_goal).
When an owning static gate admits candidates, it measures their empirical
coordinates on one frozen workload/environment matrix. The current
open-architecture static frontier is not closed, so these commands are
diagnostic and do not presently authorize candidate ranking. A result from
another binary, matrix or environment cannot select `empirical_survivors`, and
timing cannot weaken the hard constraints or the minimum unique-authority
coupling cut.

This module measures the throughput of the CKB tx-pool pipeline.

## Running

### Manually

```bash
cargo bench -p ckb-tx-pool --features internal
```

### Using the comparison script

```bash
# default medium matrix (~10-15 minutes)
python3 tx-pool/scripts/benchmark.py

# small quick matrix (~5 minutes)
python3 tx-pool/scripts/benchmark.py --quick

# focused quick diagnosis (one matching scenario family)
python3 tx-pool/scripts/benchmark.py --quick --filter always_success_100

# preferred quick A/B: pair each exact scenario adjacently
python3 tx-pool/scripts/benchmark.py --quick --runs 6 \
  --filter always_success_100 \
  --baseline-worktree /tmp/ckb-txpool-bench-baseline \
  --save-baseline-json /tmp/tx-pool-baseline.json \
  --save-json /tmp/tx-pool-candidate.json \
  --fail-on-regression

# override the post-build thermal settling interval when needed
python3 tx-pool/scripts/benchmark.py --quick --cooldown-seconds 10

# override the thermal settling interval between paired measurements
python3 tx-pool/scripts/benchmark.py --quick --paired-cooldown-seconds 15

# full matrix (~1 hour)
python3 tx-pool/scripts/benchmark.py --full

# save the median of three complete runs
python3 tx-pool/scripts/benchmark.py --runs 3 \
  --save-json /tmp/tx-pool-baseline.json

# compare and fail on any measured regression
python3 tx-pool/scripts/benchmark.py --runs 3 \
  --compare-json /tmp/tx-pool-baseline.json \
  --save-json /tmp/tx-pool-candidate.json \
  --fail-on-regression
```

The script streams progress for independent/manual runs, aggregates repeated
runs by median, records the commit/dirty state/toolchain/platform and raw run
medians in JSON, and can enforce the architecture's strict non-regression gate.
Paired A/B prints one marker per exact scenario and the final summaries but
keeps Criterion's verbose sample log off the terminal/UI; rendering that log
can otherwise compete with the measured workers. Raw output is retained in
memory for parsing and is printed if a benchmark process fails. A failing gate
requires the baseline and candidate to come from the same recorded
host/toolchain and to use the same repetition count of at least three; a
one-run smoke record is never accepted as one side of a release decision.
`--quick` sets `QUICK_BENCH=1`, `--full` sets `FULL_BENCH=1`, and the default uses the medium matrix.

Each checkout builds exactly once into its own
`<workspace>/target/tx-pool-bench` directory; an externally supplied shared
`CARGO_TARGET_DIR` is deliberately ignored. The runner resolves and hashes each
compiled executable, waits for the configured post-build cooldown (15 seconds
by default), and then invokes those unchanged binaries directly for every
repetition. Before the cooldown, each side receives one unrecorded short
preflight of the selected workload (10 flat samples, one-second warm-up and
one-second measurement). This symmetrically populates VM, code-page and crypto
paths that Criterion's first ordinary warm-up does not reliably cover. This
prevents Cargo from reusing a baseline worktree's same-named
executable for the candidate, avoids repeated freshness checks, and keeps
compilation heat outside the measured A/B pairs. The hash is verified again
after measurement. Strict comparisons also require a byte-identical SHA-256
fingerprint of the Python runner and Rust benchmark harness.

Both builds disable incremental compilation and remap their distinct worktree
roots to the same logical source prefix. This keeps checkout paths from
perturbing source-derived generated-code identity/layout. Rust dependencies may
still embed compile-time `CARGO_MANIFEST_DIR` strings, which remapping cannot
rewrite, so strict A/B also requires baseline and candidate worktree paths to
have the same byte length. Use equally sized names such as `/tmp/txpool-base`
and `/tmp/txpool-cand`. The normalized effective Rust flags are recorded and
must match across a strict comparison. The runner also binds the comparison to
the same Rust/Cargo/Python versions, logical CPU count, `Cargo.lock`, and
workspace manifest (including the bench profile). Missing command metadata is
an error, not an `unknown == unknown` match.

Criterion's own implicit on-disk baseline and plot generation are disabled by
the runner, and every process receives an empty temporary `CRITERION_HOME`.
The script is the sole A/B authority; Criterion otherwise compares against an
unrelated prior invocation even in discard mode, producing misleading `change`
output, while rendering reports adds CPU work between scenarios.

Every report includes the max-min throughput spread across complete runs. With
`--fail-on-regression`, either side exceeding
`--max-run-spread-percent` is rejected as an invalid/noisy measurement rather
than mislabeled as a code regression. Quick diagnostics default to a 4%
independent-run spread ceiling, a 2% paired ratio MAD ceiling and a 2%
regression threshold; medium/full retain the 5% independent-run spread ceiling,
a 1.5% paired ratio MAD ceiling and the strict 0% architectural gate. `--filter`
runs only matching benchmark IDs,
which is useful for a fast follow-up on a suspicious workload. Quick remains a
development diagnostic; medium/full repeated records are the architectural
acceptance evidence. Strict records must also come from clean tracked trees,
so the exact measured source can be reconstructed from `git_commit` (untracked
local notes do not invalidate a run).

`--baseline-worktree` is the preferred quick comparison mode. After listing
and verifying that both binaries expose the same selected matrix, it invokes
one exact scenario at a time: baseline/candidate measurements for that
scenario are adjacent instead of separated by an entire matrix. All balanced
AB/BA repetitions for that scenario finish before the runner changes workload,
so a secp-heavy scenario cannot perturb later repetitions of a lightweight
scenario. It reverses side order on every second repetition. This prevents a
slow host-frequency drift from being misclassified as a code delta and avoids
giving either revision a privileged time slot. A strict paired gate
therefore requires an even repetition count of at least six; an odd number
would give one revision more first-run slots. Paired mode evaluates the median
of each adjacent candidate/baseline ratio and bounds the ratios' relative
median absolute deviation (MAD). Unlike a maximum-deviation rule, MAD does not
give one isolated host-scheduling outlier veto power; six balanced pairs keep
the median valid even if two isolated samples are contaminated. Each worktree
still uses its isolated Cargo target, and both records retain independent
commit/dirty metadata plus the common harness fingerprint.

### Cross-version one-shot A/B

Use the one-shot runner when two frozen current-goal checkpoints cannot expose
the same Criterion harness API. The benchmark
fixture and its Cargo bench declaration must be committed in both measurement
worktrees; `profile_one_shot.rs` must be byte-identical. A compatibility
feature may adapt only the benchmark's service-construction call to an older
API. It must not change the workload, completion condition or measured window.
Each revision retains its production relay transport: the legacy builder uses
its unbounded channel while the authority candidate uses the bounded receiver
returned by its builder. A byte-identical, ready-before-start consumer drains
both at a fixed one-millisecond cadence, so an unconsumed synthetic channel
cannot become a benchmark-only backpressure limit.

```bash
python3 tx-pool/scripts/cross_version_benchmark.py \
  --baseline-root /private/tmp/txp-medium-base0 \
  --candidate-root /private/tmp/txp-medium-cand0 \
  --baseline-target-dir /private/tmp/txp-target-base0 \
  --candidate-target-dir /private/tmp/txp-target-cand0 \
  --baseline-build-features cross-version-legacy-bench-adapter \
  --output /private/tmp/txp-medium-result.json \
  --replicates-per-sample 4 \
  --scenario always_success,32000,100,8,1 \
  --scenario always_success,32000,100,8,4 \
  --scenario secp256k1,8000,50,8,4 \
  --scenario dependent_forest_10,32000,100,8,4
```

The default path builds each side exactly once with `--locked`, incremental
compilation disabled, an isolated Cargo target and a common remapped logical
source prefix. It then records ten balanced samples from the immutable binary
hashes, with a 15-second initial cooldown and ten seconds between attempts. A
scenario is comparable only when its paired-ratio relative MAD is at most
1.5%. The JSON checkpoint is rewritten atomically after every attempt, so an
interrupted run retains all completed evidence.

One ordinary sample contains one adjacent baseline/candidate pair, with the
first side reversed on alternating samples. On a host where the bounded 32k
one-shot window is still too short for the 1.5% gate,
`--replicates-per-sample 4` defines each sample as four attempts per side. The
order alternates inside each sample (`AB`, `BA`, `AB`, `BA`) and reverses for
the next sample, so neither revision receives more first slots. A side's
sample throughput is computed as total target transactions divided by the sum
of its target elapsed times; it is not an average of reported rates. CPU and
context-switch diagnostics likewise aggregate raw counts before deriving
ratios. Every constituent attempt must independently pass the binary,
scenario, accepted-count and clock-window checks, and all remain in the JSON.
Values above one must be even and are bounded to eight. The replicate count
must be chosen before measurement; regrouping an already observed artifact is
diagnostic only and cannot create release evidence.

Warmup is complete only after both the callback hash set and the relay
`(tx_hash, original_peer)` set exactly match the warm workload with no
callback in flight. The measured window ends only after the corresponding
target sets also match. Duplicate relay results, rejects, unexpected-parent
results and generation resets invalidate an ordinary sample; reverse
dependency workloads may emit the expected intermediate unknown-parent
results, and reorg workloads may repeat callbacks while still requiring the
exact final hash set. The observer preallocates its complete set before the
target allocation window. This is a non-backpressured tx-pool pipeline
measurement, not a claim about the production relayer's network-send cadence;
relay saturation must be measured as a separately named workload.

The harness reports the measured duration from Rust's monotonic `Instant` and
the profiling crop in Unix wall-clock nanoseconds. These are different clock
domains and cannot be read atomically. The monotonic duration is authoritative
for throughput. Every attempt records the wall-window delta and labels the crop
`aligned` when it is within `max(1 ms, elapsed * 100 ppm)`. Preemption between
the end `Instant` and wall-clock reads may legitimately make the crop wider;
that is recorded as `scheduler_widened` and the crop must not be used for
profile attribution, but it does not invalidate the monotonic throughput
sample. A non-monotonic wall window or one materially shorter than the measured
target interval is contradictory temporal evidence and still fails the
attempt. This keeps optional profiling coordinates from becoming a second
timing authority.

Cross-version release batches must sustain the measured window for roughly one
second or longer on the comparison host. A calibration with 400 cheap/dependent
transactions and 200 secp transactions produced only 47-132 ms ordinary
windows; isolated scheduler delays then stretched samples to 212-369 ms and
all four paired MAD values correctly failed at 4.1-13.8%. An 8,000
cheap/dependent and 2,000 secp follow-up still produced only one- to two-second
windows; three of four paired records exceeded the 1.5% MAD gate. The matrix
above therefore uses 32,000 cheap/dependent transactions and 8,000 secp
transactions. Direct fixed-binary calibration sustained a 4.68-second
four-peer cheap window with 32,768 target transactions, while a 49,152-target
probe did not complete within 60 seconds and crossed into a capacity or
nonlinear regime. The runner retains a hard 32,768-transaction total bound
instead of hiding noise by crossing into a full-pool or timeout regime. Do not
shrink these counts for a release verdict unless a rejected calibration
artifact demonstrates a tighter stable window on the target host.

Each successful attempt records two deliberately different CPU observations.
The one-shot binary samples `RUSAGE_SELF` immediately around target work; this
target-window value owns `cpu_time_per_transaction` and excludes construction,
preflight, warm population and teardown. The parent also records the complete
child process's user/system CPU time, average CPU parallelism and voluntary/
involuntary context-switch rates from `RUSAGE_CHILDREN`; those whole-process
values remain scheduler diagnostics and never substitute for target CPU.
Allocation calls/bytes use a counting global allocator enabled over the same
target work, while peak RSS remains the conservative whole-process high-water
mark. The scenario summary derives paired target-CPU ratios and medians. None
of these observations relaxes the wall-throughput MAD gate.

With `profiling` enabled, the one-shot binary also writes schema-v2 span
lifetimes for spans that start while target work is active. Every required
authority read/write wait/hold span must be observed with a positive start
count and elapsed lifetime; missing/empty coordinates invalidate the attempt.
This is independent of `profile.py`'s schema-v1 low-overhead start-count
artifact and does not change that established profiling protocol.

The additional `reorg_in_flight` confirmation registers both Pending and
Proposed callbacks, deduplicates completion by full transaction hash and
deliberately keeps the effect callback active while ordered reorg authority is
invoked. Its record includes the positive callback-overlap count; a scenario
label without observed overlap is rejected.

Both source roots, and both build target paths when the runner builds both
sides, must have equal UTF-8 byte length. This bounds path-derived code-layout
noise that source remapping cannot remove. The worktrees must be completely
clean, including untracked files. The record binds each source commit,
`Cargo.lock`, workspace and crate manifests, runner, byte-identical harness,
toolchain, host metadata, build flags and binary SHA-256. Source, harness,
runner and binary identities are checked again after measurement. Existing
fixed binaries may be supplied with `--baseline-binary` and
`--candidate-binary`; their hashes remain the execution authority.

The one-shot harness supports `always_success`, `secp256k1`, `dependent`,
`dependent_reverse`, `dependent_forest_<depth>`, `fanout`, `fanout_reverse`
and `always_success_fanin_<width>`. Reverse-order capability cases must use a
zero warm count because their unresolved prefix is itself the behavior under
test. Run capability cases separately with `--allow-noncomparable`: a timeout
or nonzero exit is useful compatibility evidence, but it is never folded into
the throughput aggregate or treated as a noisy performance sample.

## Matrices

The matrix is selected at compile time via environment variables:

- `FULL_BENCH=1` - full matrix.
- `QUICK_BENCH=1` - quick matrix.
- default (no env var) - medium matrix.

### Full matrix (`FULL_BENCH=1`)

| Constant | Value |
|---|---|
| `SIZES` | `[50, 100]` |
| `PEER_COUNTS` | `[1, 2, 4, 8]` |
| `WORKER_COUNTS` | `[4, 8, 12]` |
| `WARM_POOL_SIZE` | `30` |
| `DEPENDENT_SIZES` | `[10, 20]` |
| `DEPENDENT_WARM_POOL_SIZE` | `10` |

### Medium matrix (default)

| Constant | Value |
|---|---|
| `MEDIUM_SIZES` | `[100]` |
| `MEDIUM_PEER_COUNTS` | `[1, 4]` |
| `MEDIUM_WORKER_COUNTS` | `[4, 8]` |
| `MEDIUM_WARM_POOL_SIZE` | `30` |
| `MEDIUM_DEPENDENT_SIZES` | `[10]` |
| `MEDIUM_DEPENDENT_WARM_POOL_SIZE` | `10` |

### Quick matrix (`--quick` / `QUICK_BENCH=1`)

| Constant | Value |
|---|---|
| `QUICK_SIZES` | `[100]` |
| `QUICK_PEER_COUNTS` | `[1]` |
| `QUICK_WORKER_COUNTS` | `[8]` |
| `QUICK_WARM_POOL_SIZE` | `30` |
| `QUICK_DEPENDENT_SIZES` | `[20]` |
| `QUICK_DEPENDENT_WARM_POOL_SIZE` | `10` |

Regular workloads and dependent-chain workloads use different size/warm configurations so the chain never grows too long. Dependent chains are only benchmarked with **1 peer and the first worker count** because they are bottlenecked by serialized dependency resolution/wake-up; varying peers/workers adds no useful signal.

## Workloads

- `always_success`: independent transactions using the always-success lock and genesis issue outputs.
- `secp256k1`: independent transactions using the secp256k1_blake160_sighash_all lock.
- `dependent_always_success_parent_first`: a normal parent -> child chain using the always-success lock and the in-flight dependency path.
- `dependent_always_success_child_first`: the same chain submitted in reverse to exercise kernel dependency waiting and wake-up.
- `dependent_secp_parent_first`: a normal parent -> child chain using the secp lock.
- `dependent_secp_child_first`: the same secp chain submitted in reverse.

Each workload is tested in two variants:

- **cold pool**: submit the target transactions into an empty pool.
- **warm pool**: pre-fill `WARM_POOL_SIZE` transactions, then submit the target transactions.

## Key implementation details

### Cycles measurement

- All `always_success` transactions have the same cycle cost, so a single sample is measured and reused.
- `secp256k1` and dependent-chain transactions may have different cycle costs, so each transaction is measured individually via `test_accept_tx` / `process_tx` and stored as `HashMap<tx_hash, cycle>` to avoid order mismatches with a plain `Vec`.
- Dependent cycle measurement captures its next pending-count target before enqueueing each transaction, so a fast commit cannot turn the wait into an impossible `current + 1` target.
- `max_ancestors_count` is set to `1000` in the benchmark config so dependent chains do not hit the ancestor limit.

### Dependent-chain submission strategy

Dependent chains are measured in both directions because they exercise different state transitions:

- **parent-first**: children observe an in-flight parent and use the ordered dependency path;
- **child-first**: children enter kernel `Wait(Missing)` state and are woken after their parents are accepted.

- **warm benchmark**: the warm prefix is already in the pool; the target segment is submitted in the selected direction.
- **cold benchmark**: the target segment depends on the warm prefix. The warm prefix is submitted in natural order during setup (not measured), then the target segment is submitted parent-first or child-first during measurement.

### Resource lifecycle

- All workload fixtures share one Tokio executor and dummy network handle. The
  executor uses the host's available parallelism, matching the production
  runtime default; its value is part of the comparison fingerprint. Genesis
  stores/snapshots remain isolated, and every measured service remains fresh.
  This avoids parking four independent runtimes in the same benchmark process.
- `start_controller` builds a full UAK tx-pool through the production
  `TxPoolServiceBuilder::start` path and returns a `ServiceHandle`.
- Before returning a new controller to Criterion, setup completes one dispatcher round-trip and a short Tokio scheduling interval. This keeps freshly spawned worker startup latency outside the measured transaction batch without warming the verification cache or pool.
- `ServiceHandle::drop` cancels the local `CancellationToken`, awaits the main dispatcher (which quiesces all message handlers and production workers), and drops/awaits the relay drain. No cancelled worker, pool save, or blocking drain may overlap the next iteration. A teardown timeout or task panic fails the benchmark instead of silently admitting a contaminated sample.
- Criterion uses `iter_batched_ref`, so that complete service shutdown (worker quiescence, pool save and relay-drain join) happens after the measurement interval rather than being charged to transaction latency.
- Cycle measurement reuses that same production service. Independent samples
  use `test_accept_tx`; dependent samples enter through Proposal ingress and
  query the cycles committed by the resulting Accepted owners. There is no
  benchmark-only worker topology or alternate mutation path.
- The sole bounded relay receiver is retained but not drained during a sample.
  This models a slow consumer without adding a competing task; the production
  mailbox's nonblocking overflow/reconciliation behavior prevents authority
  progress from depending on that receiver.
- `SharedBench` creates genesis issue outputs according to the workload's actual need (`issue_outputs = max_size + warm_pool_size`), avoiding over-allocation for dependent chains.

### Criterion sampling

Quick mode uses the narrow one-peer/one-worker-count matrix with 30 flat
samples, a 3-second warm-up, and a 10-second measurement window. Strict paired
use spends the saved time on at least six independent, balanced process pairs;
that is more effective against scheduler and thermal outliers than extending
one process measurement. Its larger 100-transaction
independent batches and 20-transaction dependent chains improve
signal-to-noise without expanding the scenario matrix. Do not reduce this
sampling budget without clean paired evidence that the MAD gate remains
reliable. Medium/full modes remain the release gates.

Medium mode is the quantitative release tier and uses 60 samples, a 6-second
warm-up and a 20-second measurement window. Full mode uses 100 samples, a
10-second warm-up and a 30-second measurement window across the complete
peer/worker/size matrix. Thus each tier increases evidence in a distinct way:
quick narrows the matrix, medium strengthens the estimator, and full combines
the strongest estimator with the broadest matrix.

Paired A/B sleeps for 10 seconds between every fixed-binary measurement by
default. This is separate from the post-build cooldown and is part of the
comparison fingerprint. CPU-bound secp batches can otherwise heat the host
monotonically so the second side of a pair measures thermal history instead of
code. Set `--paired-cooldown-seconds 0` only for a non-gating diagnostic; a
release record must still satisfy the paired-ratio MAD gate.

Before recording a scenario, paired A/B also runs one unrecorded full pilot on
each side. The short generic preflight establishes code-page residency, while
the scenario pilot establishes the sustained CPU state that a secp batch
requires. Pilot results never enter the median or spread calculation; their
count and the `scenario-pilot-alternating-v3` schedule are fingerprinted.

Measured completion is event-driven: the pending callback increments an atomic counter and wakes a `Notify` waiter only after the pool transition is stable. No 1 ms polling timer or `get_tx_pool_info` request runs in the measured path.

`PEER_COUNTS` denotes real ingress owners: every concurrent submitter uses a
distinct `PeerIndex`. It is not merely a task-concurrency multiplier, so medium
and full runs exercise the per-peer scheduler and budget boundaries they claim
to cover.

Dependent-chain latency includes a fixed notify -> resolve -> verify -> commit ->
journal -> callback path. That fixed cost can dilute the percentage impact of a
change isolated to one stage. Review both the absolute median times in the A/B
summaries and the paired ratios; do not interpret a ratio near one as proof
that an individual stage is unchanged.

## Output format

Benchmark ID format:

```text
tx_pool_pipeline/{mode}_{peers}peer_{workers}worker_{warm_}{tx_type}_{size}
```

Example:

```text
tx_pool_pipeline/pipeline_1peer_8worker_warm_dependent_secp_child_first_20
```

`benchmark.py` parses the `time:` and `thrpt:` lines after each ID and produces
the comparison table.
