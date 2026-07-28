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

Host-specific Samply profiles and symbol tables are not versioned because they
are large and not portable. A feature-gated low-cardinality tx-pool profiling
surface is still required so reviewers can regenerate stage/kernel attribution
without a private harness. It must add no default-build span/subscriber work,
must reuse bounded stage names, and must initialize fallibly rather than panic.

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

A5 is incomplete until `tx-pool/scripts` contains the capture/analysis entry
point and this document contains exact commands for Samply, cargo-instruments
and tokio-console. `/private/tmp` artifacts used in the exploratory pass are
not sufficient final evidence.

## Retained changes

| ID | Design | Safety argument | Performance intent | Status |
|---|---|---|---|---|
| A0 | Await verifier work directly after its existing async dispatch; remove redundant `block_in_place`. | Ownership, cancellation and verification semantics are unchanged; no lock crosses the await. | Avoid Tokio compensation and scheduler overhead. | Committed in `bc437ccbe`. |
| A1 | Seal raw revisions behind typed Resolve/Verify leases. | Callers cannot assemble a hash/revision/location authority tuple; stale completion is rejected by construction. | Keeps later hot-path changes proof-carrying without lookup/defensive state. | Committed in `3376261f1`. |
| A2 | Borrow Ready commit authority through Plan/Apply instead of cloning it. | The exclusive kernel borrow proves the Ready owner remains current until handoff or rejection. | Remove payload cloning and redundant authority reads at commit. | Committed in `30b77357c`. |
| A4 | On successful Resolve or Verify completion, check out one next same-lane lease inside the same kernel mutation. | The fair queue still selects work; `VerifyLease` seals the original worker capability and continuation cannot cross stages. `AppliedContinuation` distinguishes completed Apply from post-Apply checkout failure. Pause, cancellation or command loss completes at most the already-owned lease in final mode. | Remove one kernel mutex acquisition and one wake/scheduler round trip per independent same-lane transaction. | Implemented; 271 unit tests, strict static gates and direct RelayV3 integration evidence pass; final performance acceptance remains. |
| A5 | Feature-gated low-cardinality stage/kernel profiling points and reproducible capture instructions. | Feature-off builds have no instrumentation path; profiling observes existing transitions and never selects state, retry or failure behavior. | Preserve future attribution and wake/lock analysis without temporary source patches. | Design frozen; implementation pending. |

## Rejected change

| ID | Candidate | Result | Decision |
|---|---|---|---|
| A3 | A second state-local projection Apply path intended to reduce projection maintenance. | Controlled dependency A/B was effectively neutral (four-scenario geometric mean about `+0.38%`; parent-first about `-0.23%`) while adding roughly 60 production lines and a second mutation implementation. | Fully reverted. The extra proof surface was not justified. |

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

1. Add A5's feature-gated, observation-only profiling surface, committed
   capture/analyzer entry point, artifact manifest and exact reproduction
   instructions without changing the default binary.
2. Run all `ckb-tx-pool` nextest tests, production Clippy/static panic gates,
   documentation and security-manifest validators.
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
