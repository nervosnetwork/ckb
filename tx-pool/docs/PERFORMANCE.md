# Tx-Pool Performance Design and Evidence

This document records the performance design that complements
[`ARCHITECTURE.md`](ARCHITECTURE.md). It is intentionally not an implementation
diary: it keeps only decisions a reviewer needs to reproduce, challenge or
extend the optimization work.

Correctness remains the first gate. An optimization is retained only when its
safety argument follows the single-authority Plan/Apply model, deterministic
tests pass, and a controlled fixed-binary A/B shows value. A change with no
measured value is removed even when it is locally plausible.

## Objective and fixed constraints

The target is not merely parity with `develop`. The pipeline should exceed it
where CKB transactions permit parallel work while remaining easier to reason
about:

- `PrePoolKernel` is the sole pre-pool owner; `TxPool` is the sole accepted
  owner, with one explicit atomic handoff;
- validation and immutable-snapshot computation run outside authority locks;
- Plan is read-only, Apply is total and single-consumption;
- external I/O consumes only effects committed with authoritative mutation;
- legal transaction, peer, capacity and stale-lease outcomes are typed and do
  not reach invariant failure;
- no optimization adds an inferred owner, mutable cache, unbounded task, queue
  scan or lock held across an await;
- block-template full/reset serialization remains independent from optimistic
  uncle/partial publication, preserving their intended concurrency.

The dependency-frontier/DAG model is therefore an analytical tool, not a new
resident graph authority. Existing exact dependency indexes may expose ready
frontiers, but a second scheduler or duplicated lifecycle state is rejected.

## Profiling evidence

### Method and evidence strength

The first pass used pre-built release binaries and an isolated one-shot harness
on an 8-core Apple Silicon host. The measured interval starts immediately
before remote submission and ends when the stable pending callback observes the
target population; fixture construction and cycle discovery are outside that
interval. Current/develop runs use equal-length worktree paths and alternating
order. Whole-process CPU/RSS figures include fixture setup and are therefore
supporting evidence, not interval-only attribution.

The comparison below is the pre-optimization current checkpoint
`284a67b7f871` against develop `91b97ab5f67f`, with 12 paired repetitions,
Rust 1.95.0, empty `RUSTFLAGS`, and separately hashed binaries. Positive elapsed
delta means the pipeline checkpoint was slower.

| Workload | Shape | Median elapsed: current / develop | Paired elapsed delta | Median CPU ratio | Median RSS ratio |
|---|---|---:|---:|---:|---:|
| always-success | 1 peer, 8 workers, 500 target + 100 warm | 62.61 / 40.12 ms | `+56.44%` | 1.000 | 0.871 |
| always-success | 4 peers, 8 workers, 500 target + 100 warm | 59.05 / 41.73 ms | `+40.85%` | 1.022 | 0.899 |
| dependent chain | 1 peer, 8 workers, 20 target + 10 warm | 5.47 / 4.21 ms | `+29.84%` | 1.000 | 1.022 |
| secp256k1 | 4 peers, 8 workers, 200 target + 50 warm | 79.45 / 76.88 ms | `+3.36%` | 1.000 | 1.066 |

These records identify the shape of the regression; their roughly 3.1–7.9%
paired max deviation is too wide for the final 2% release verdict. Final
acceptance therefore uses the stricter protocol in
[`BENCHMARK.md`](BENCHMARK.md), not these exploratory numbers.

### Sampled attribution

Samply and macOS sampled stacks were captured only inside the emitted
submission window, with symbols generated from each exact binary. They showed:

- cheap current spent about 46% of sampled time in `__psynch_cvwait`, versus
  about 4% on develop;
- on the corrected dependency-forest workload, current wall time was about 36%
  higher while sampled CPU was only about 4.5% higher;
- roughly 14–17% of current dependency-forest samples remained below the
  PrePoolKernel mutex path, with substantially more runtime-thread parking;
- the resumable verifier wrapped an already-Tokio-scheduled VM future in
  `block_in_place(Handle::block_on(...))`, adding a blocking parent and runtime
  compensation without moving VM execution to a dedicated executor.

The combination is stronger evidence for scheduling/acquisition latency than
for excess verification compute. It selected A0 and A4: remove the false
blocking boundary, then reuse a successful stage's existing kernel acquisition
for one fair same-lane checkout. A notification-only experiment was
neutral/slower and was discarded; adding more wakes does not address the
observed cost.

### A5 candidate-selection matrix

After A4, the committed runner was used to capture a broader matrix at
`a5dd757432d6b4150f8d224a6895278db95553b5`. Every row reused the same release
binary (`sha256:59f1ba2396c86ce0aa3cc37773a6c0e6820c3a51dffe83f586031b569e3b7c6c`)
and the same harness source. All twelve manifests were subsequently rechecked
by the committed analyzer, including their source and artifact hashes.

The wall/CPU columns are single profiling executions and are not benchmark
results. Mutex percentages are inclusive target-window Samply samples. Lock,
mutation and read counts come from the separate same-scenario span execution;
they are deterministic control-flow evidence per target transaction, not CPU
time.

| Workload | Shape | Wall / CPU ms | Kernel mutex samples | Lock / mutate / read closes per target |
|---|---|---:|---:|---:|
| always-success cold | 1 peer / 1 worker / 1,000 tx | 245.1 / 293.7 | 0.00% | 9.01 / 5.01 / 4.00 |
| always-success cold | 1 peer / 8 workers / 1,000 tx | 185.6 / 700.0 | 3.14% | 9.53 / 5.87 / 3.67 |
| always-success cold | 4 peers / 8 workers / 1,000 tx | 160.1 / 740.2 | 11.72% | 13.07 / 9.22 / 3.85 |
| always-success warm | 4 peers / 8 workers / 1,000 tx, 200 warm | 147.5 / 707.9 | 9.88% | 13.22 / 9.28 / 3.94 |
| dependent always-success, parent first | 1 peer / 8 workers / 200 tx, 20 warm | 92.3 / 124.2 | 4.29% | 30.74 / 19.87 / 10.88 |
| dependent always-success, child first | 1 peer / 8 workers / 200 tx, 20 warm | 103.4 / 130.1 | 3.62% | 30.45 / 19.52 / 10.93 |
| secp256k1 cold | 1 peer / 1 worker / 400 tx | 813.6 / 827.2 | 0.00% | 9.05 / 5.04 / 4.01 |
| secp256k1 cold | 1 peer / 8 workers / 400 tx | 232.5 / 930.4 | 0.38% | 9.16 / 5.13 / 4.03 |
| secp256k1 cold | 4 peers / 8 workers / 400 tx | 204.9 / 1,238.0 | 2.36% | 9.93 / 6.21 / 3.72 |
| secp256k1 warm | 4 peers / 8 workers / 400 tx, 100 warm | 240.3 / 1,239.3 | 2.34% | 10.33 / 6.45 / 3.88 |
| dependent secp256k1, parent first | 1 peer / 8 workers / 100 tx, 10 warm | 189.4 / 196.9 | 0.97% | 30.70 / 19.86 / 10.85 |
| dependent secp256k1, child first | 1 peer / 8 workers / 100 tx, 10 warm | 203.3 / 205.7 | 0.47% | 28.95 / 18.43 / 10.52 |

This matrix narrows A6 rather than proving an optimization. Independent cheap
transactions scale from one to eight workers, but multi-peer concurrency raises
the mutex path from no sampled contention to 11.72% and raises authoritative
mutations from 5.01 to 9.22 per target. The equivalent secp workload spends only
2.36% in that path because verification dominates. Dependent workloads require
about three times as many authority acquisitions by design because dependency
availability changes and wake settlement are causal transitions. Consequently,
the first A6 candidates are repeated work within the existing atomic mutation:
idempotent scheduler-head publication and unchanged projection maintenance.
The evidence does not admit a second DAG, cache, batch owner or wider lock
epoch. Those designs add proof and starvation costs before the measured
mechanical work has been removed.

The host artifacts were retained under
`/private/tmp/txpool-profile-matrix-a5` during the review. They are deliberately
not repository inputs. To reproduce the matrix, run the first command in the
next section without `--binary`, then pass its manifest's exact binary path to
the remaining scenario combinations shown in the table. Published release
claims still require the paired quick/medium protocol rather than these rows.

Host-specific Samply profiles and symbol tables are not versioned because they
are large and not portable. The committed feature-gated profiling surface lets
reviewers regenerate stage/kernel attribution without a private harness. It
adds no default-build span/subscriber work, uses bounded static stage names and
initializes the optional Tokio console fallibly rather than making telemetry a
node-startup precondition.

### Reproducibility contract

A profiling observation is admissible review evidence only when all of these
conditions hold:

1. The committed profiling runner reuses the benchmark transaction fixtures
   and the same submission-to-stable-callback interval. A temporary benchmark,
   manual RPC timing or whole-process sample cannot replace that interval.
2. Its manifest records the Git revision and tracked diff, enabled features,
   binary and harness SHA-256, `Cargo.lock` and workspace-manifest SHA-256,
   Rust/Cargo/profiler versions, CPU count, platform, source-root length,
   workload parameters, capture command and emitted start/end timestamps.
3. Baseline, candidate and develop use the same profiler, sampling frequency,
   symbolization and workload parameters. Missing or `unknown` identity fields
   make the comparison invalid rather than equal.
4. The raw host-specific trace may remain an external artifact, but its path,
   byte size and SHA-256 plus the deterministic analysis summary are retained.
   A reviewer can rerun the committed analyzer against that artifact.
5. CPU samples, task wait/poll data, kernel wait/hold spans and wall-clock A/B
   are reported separately. A flame-graph percentage alone cannot establish a
   throughput improvement.
6. Every published conclusion is labelled exploratory, candidate-selecting or
   release-accepting. Only the controlled medium/full benchmark protocol can
   close the performance release blocker.

The canonical runner is `tx-pool/scripts/profile.py`. Its one-shot mode reuses
the exact benchmark fixture and emits one `TX_POOL_PROFILE_WINDOW` record. The
runner asks Samply to sample the process and pre-symbolicate a sidecar, then the
deterministic analyzer counts only samples inside that record. CPU deltas are
counted only when both ends of their sampling interval lie inside the window;
this prevents fixture CPU immediately before submission from leaking into the
result. Parking samples and per-thread CPU deltas remain separate because a
thread's leaf frame at the end of an interval is not proof of where that
interval consumed CPU.

### Reproducing a Samply profile

Install Samply 0.13.1 or later, then run one scenario. Omitting `--binary`
builds the `internal,profiling` benchmark once with locked dependencies and
records its exact path and SHA-256:

```bash
python3 tx-pool/scripts/profile.py capture \
  --output-prefix /private/tmp/txpool-independent-cold \
  --tx-type always_success \
  --pool-state cold \
  --dependency-order parent_first \
  --peers 1 --workers 8 --size 500 --warm-pool-size 100
```

For the remaining scenarios, read `artifacts.binary.path` from the first
manifest and pass that unchanged path. This avoids recompilation; every new
manifest re-hashes the binary and identifies its provenance as reused by hash:

```bash
python3 tx-pool/scripts/profile.py capture \
  --output-prefix /private/tmp/txpool-secp-cold \
  --binary /absolute/path/from/the/first/manifest \
  --tx-type secp256k1 \
  --pool-state cold \
  --dependency-order parent_first \
  --peers 4 --workers 8 --size 200 --warm-pool-size 50
```

Re-analysis never samples or runs the target binary. It requires the exact
recorded benchmark/analyzer source, verifies every recorded artifact size/SHA,
and rewrites the deterministic summary from the raw profile, symbol sidecar,
span log and recorded target windows:

```bash
python3 tx-pool/scripts/profile.py analyze \
  --manifest /private/tmp/txpool-secp-cold.manifest.json
```

Each capture produces `.json.gz`, `.json.syms.json`, `.stdout.log`,
`.stderr.log`, `.spans.log`, `.span.stdout.log`, `.span.stderr.log`,
`.manifest.json` and `.summary.json`. CPU sampling and span formatting are two
separate executions of the same SHA-verified binary and exact scenario. This
is a load-bearing measurement boundary: emitting thousands of formatted close
records during Samply capture would inject subscriber locking and file I/O into
the mutex profile. The manifest records both commands and both target windows;
the span log contains only close/busy/idle records for the seven static tx-pool
spans, while the Samply summary is the deterministic window-cropped
CPU/residency view from the execution with no span subscriber. Artifacts are
refused inside the source tree
and existing files are not replaced unless `--force` is explicit. The
manifest binds the Git diff and untracked content, enabled
features, binary/harness/manifests/lockfile hashes, exact command, scenario,
sampling rate, toolchain/profiler, CPU/OS/filesystem, flags, power/thermal state
where available and nanosecond target window. Missing identity data is an
error, never an `unknown == unknown` match.

### Tokio wake and lock observation

Samply answers where process threads were sampled. Tokio console is the
separate tool for task lifetime, poll, wake and resource behavior. Build the
node with both observation features and Tokio's required unstable cfg, run an
isolated node workload, then connect from another terminal:

```bash
RUSTFLAGS='--cfg tokio_unstable' \
  cargo build --bin ckb --features profiling,tokio-trace --locked

TOKIO_CONSOLE_BIND=127.0.0.1:6669 \
TOKIO_CONSOLE_RETENTION=30s \
TX_POOL_PROFILE_TRACE_PATH=/private/tmp/txpool-span-close.log \
  target/debug/ckb -C /path/to/isolated-node run

tokio-console http://127.0.0.1:6669
```

Tokio console intentionally consumes only Tokio runtime events; it does not
pretend application spans are runtime tasks. The separate
`TX_POOL_PROFILE_TRACE_PATH` layer writes close records with busy/idle timing
for tx-pool spans only. It creates a new file and refuses an existing path so
two runs cannot be silently mixed. The spans have only the following static
low-cardinality names:
`tx_pool.stage.resolve`, `tx_pool.stage.verify`, `tx_pool.commit.drive`,
`tx_pool.effects.publish`, `tx_pool.kernel.lock_wait`,
`tx_pool.kernel.read_hold` and `tx_pool.kernel.mutate_hold`. No transaction or
peer identifier is attached. Kernel wait and hold are distinct, and none of
the observed async functions holds the kernel mutex across an await.

`TOKIO_CONSOLE_PUBLISH_INTERVAL` and
`TOKIO_CONSOLE_BUFFER_CAPACITY` are optional. `TOKIO_CONSOLE_RECORD_PATH` is
deliberately rejected: the upstream builder opens it through `expect`, so
accepting an operator-controlled path would reintroduce panic-based telemetry.
The publish interval and buffer capacity must both be positive because the
upstream Tokio interval/channel constructors reject zero. An invalid
duration/address/capacity/span path, subscriber conflict, thread
creation failure or server error is reported without terminating the node.
Building `tokio-trace` without `RUSTFLAGS='--cfg tokio_unstable'` is rejected at
compile time, before the upstream runtime assertion can exist in an executable.
The ordinary `profiling` benchmark feature does not enable the console
subscriber.

### Optional macOS cross-check

`cargo-instruments` is a host-specific corroboration tool, not the canonical
artifact format. It requires a full Xcode developer directory for `xctrace`:

```bash
xcode-select -p
xcrun --find xctrace

TX_POOL_PROFILE_TX_TYPE=always_success \
TX_POOL_PROFILE_POOL_STATE=cold \
TX_POOL_PROFILE_DEPENDENCY_ORDER=parent_first \
TX_POOL_PROFILE_PEERS=1 \
TX_POOL_PROFILE_WORKERS=8 \
TX_POOL_PROFILE_SIZE=500 \
TX_POOL_PROFILE_WARM_POOL_SIZE=100 \
  cargo instruments -p ckb-tx-pool --bench pipeline \
    --features internal,profiling --template 'Time Profiler' --no-open \
    -- --bench --noplot --discard-baseline --color never
```

If `xcrun --find xctrace` fails, the result is unavailable rather than
evidence. On the current development host the command-line developer tools do
not expose `xctrace`, so no Instruments result is claimed.

## Retained changes

| ID | Design | Safety argument | Performance intent | Status |
|---|---|---|---|---|
| A0 | Await verifier work directly after its existing async dispatch; remove redundant `block_in_place`. | Ownership, cancellation and verification semantics are unchanged; no lock crosses the await. | Avoid Tokio compensation and scheduler overhead. | Committed in `bc437ccbe`. |
| A1 | Seal raw revisions behind typed Resolve/Verify leases. | Callers cannot assemble a hash/revision/location authority tuple; stale completion is rejected by construction. | Keeps later hot-path changes proof-carrying without lookup/defensive state. | Committed in `3376261f1`. |
| A2 | Borrow Ready commit authority through Plan/Apply instead of cloning it. | The exclusive kernel borrow proves the Ready owner remains current until handoff or rejection. | Remove payload cloning and redundant authority reads at commit. | Committed in `30b77357c`. |
| A4 | On successful Resolve or Verify completion, check out one next same-lane lease inside the same kernel mutation. | The fair queue still selects work; `VerifyLease` seals the original worker capability and continuation cannot cross stages. `AppliedContinuation` distinguishes completed Apply from post-Apply checkout failure. Pause, cancellation or command loss completes at most the already-owned lease in final mode. | Remove one kernel mutex acquisition and one wake/scheduler round trip per independent same-lane transaction. | Implemented; 271 unit tests, strict static gates and direct RelayV3 integration evidence pass; final performance acceptance remains. |
| A5 | Feature-gated low-cardinality stage/kernel profiling points, fallible Tokio-console setup and a reproducible windowed Samply capture/analyzer. | Feature-off builds have no instrumentation path; profiling observes existing transitions and never selects state, retry or failure behavior. Strict manifests and boundary-aware CPU accounting prevent attribution drift. | Preserve future attribution and wake/lock analysis without temporary source patches. | Completed at `a5dd75743`; 271 unit tests, strict static gates and the twelve-scenario candidate-selection matrix pass. |

## Rejected change

| ID | Candidate | Result | Decision |
|---|---|---|---|
| A3 | A second state-local projection Apply path intended to reduce projection maintenance. | Controlled dependency A/B was effectively neutral (four-scenario geometric mean about `+0.38%`; parent-first about `-0.23%`) while adding roughly 60 production lines and a second mutation implementation. | Fully reverted. The extra proof surface was not justified. |
| A6-M2 | Skip `FairQueue` head remove/insert when an owner's runnable flag is unchanged. | The mechanism was redundant and 272 unit tests passed, but a focused 8-pair 4-peer/8-worker cold always-success A/B was neutral (`-0.07%` paired throughput, 3.34% ratio spread). An earlier four-scenario run was too noisy to admit a conclusion. | Fully reverted in `d98f9955c`. Six production lines without measurable value do not justify another scheduler branch. |
| A6-M1 | Apply common entry projections as old/new deltas instead of complete detach/attach. | Both variants passed 271 unit tests and strict Clippy. Per-parent set difference regressed the focused 8-pair contention scenario by `0.51%`; a smaller whole-dependency equality gate was neutral at `+0.02%` with 3.21% ratio spread. | Fully reverted in `08de209fd` and `b488288ee`. The existing complete transition is smaller and easier to audit; measured data does not justify 62 additional production lines. |

This rejection is an architectural constraint: projection performance should
be improved by changing the canonical representation or proven update set, not
by maintaining parallel Apply implementations.

## A4 preliminary signal

The A4 candidate and baseline were pre-built once from equal-length source
roots and compared with the same benchmark harness and environment
fingerprint. These quick records guide implementation; they do not replace the
final acceptance gate.

| Workload | Repetitions | Result |
|---|---:|---:|
| independent, cold, always-success, 1 peer / 8 workers / 100 tx | 3 paired | throughput `+8.39%`, latency `-7.74%` |
| dependent parent/child, cold and warm, both arrival orders | 3 paired per scenario | geometric mean `-0.14%` (neutral) |

The split is expected: same-lane continuation targets independent concurrency;
dependency chains still wait for their causal frontier and should not gain a
cross-stage shortcut.

## Remaining acceptance plan

The order is fixed. A failed stage returns to design review rather than gaining
a compensating patch.

1. Run all `ckb-tx-pool` nextest tests, production Clippy/static panic gates,
   documentation and security-manifest validators.
2. Use the retained profiling surface to cover the CKB-semantic independent,
   secp, pool-state, peer-count and both dependency-order dimensions; retain
   only optimization candidates supported by more than one observation mode.
   The first candidate family is A6: eliminate demonstrably repeated mechanical
   work inside the existing authority transaction (unchanged-index churn,
   repeated owner-head publication, unconditional lane readiness and avoidable
   Plan cloning) before considering another batch, cache or resident graph.
3. Run the complete managed tx-pool-related process-test universe through
   `make integration`; classify any failure as product defect, obsolete test or
   environment failure before changing code.
4. Freeze final candidate binaries and run the controlled quick/medium protocol
   in [`BENCHMARK.md`](BENCHMARK.md).
5. Compare the final candidate with both the pre-optimization checkpoint and
   `develop`, including throughput, latency, spread and environment evidence.
6. Perform a final high-level invariant/security/Rust/performance review and
   record unresolved release conditions rather than hiding them.

## Review questions

- Does an optimization remove work, or merely transfer it into another queue,
  cache, retry path or failure domain?
- Does it preserve one owner and one mutation implementation?
- Can the type system express its safety premise instead of an assertion,
  `expect`, panic handler or boolean mode?
- Is attacker-controlled work bounded independently of unrelated pool size?
- Does it preserve cancellation, fairness, per-peer budget and worker
  capability semantics?
- Is the claimed gain supported by comparable binaries and low-spread paired
  measurements rather than unit-test duration?
- Does every hot-path change have target-window sampling, span timing or a
  deterministic operation-count reason, with parking residency kept distinct
  from CPU consumption?
