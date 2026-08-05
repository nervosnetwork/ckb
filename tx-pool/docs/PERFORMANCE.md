# Tx-Pool Performance Contract and Evidence

This document is the reviewer-facing performance companion to
[`ARCHITECTURE.md`](ARCHITECTURE.md). It records the current UAK performance
model, reproducible profiling method, evidence strength and final acceptance
matrix. It is not an implementation diary.

The current architecture has not yet passed P10. Historical profiles below
explain why mechanisms were selected or rejected, but they are not a release
verdict for the final source. P9.8 correctness acceptance passed at
`8d5c27559`: 410/410 internal-feature tx-pool tests and the complete 150/150
managed integration universe passed. Any later semantic change reopens its
affected correctness gates before performance evidence can be accepted.

## 1. Performance contract

The objective is not merely parity with `develop`. The design should exploit
CKB's cell model so independent transactions validate concurrently, while
coupled dependencies, RBF and chain transitions pay only their necessary
atomicity cost.

The fixed constraints are:

- `TxPoolAuthority.entries` remains the sole lifecycle owner;
- resolve, script verification and immutable computation run outside the
  authority guard;
- Plan is semantic-read-only and Apply is total and single-use;
- external I/O consumes only effects committed by Apply;
- no optimization creates an inferred owner, mutable decision cache,
  unbounded task, rollback protocol or lock held across await;
- full/reset template replacement stays serialized while proposal,
  transaction and uncle construction remains optimistic and concurrent;
- hostile count, byte, edge, closure, retry and memory work stays bounded;
- performance evidence never weakens final validation, deterministic order,
  resource accounting or compatibility.

The dependency graph is a derived analytical projection, not another owner.
Its purpose is to expose independent frontiers and bounded commuting batches.
A resident second DAG or lifecycle shard requires a proof of cross-shard
RBF/chain atomicity and a measured benefit before it can be considered.

## 2. Current cost model

The architecture cost ledger and lock-held complexity inventory are normative
in sections 3.1 and 14 of `ARCHITECTURE.md`. The performance review focuses on
four measurable costs:

| Cost | Expected shape | Required observation |
|---|---|---|
| Authority acquisition and short Apply | Material for cheap independent transactions; diluted when VM verification dominates | target-window lock wait/hold plus transitions per accepted transaction |
| Checkout, settlement and wake scheduling | Sensitive to worker/peer count and dependency arrival order | Tokio task wake/poll data plus stage spans |
| Projection and graph maintenance | Small and local for independent work; bounded closure work for dependency/RBF/reorg | operation counts and adversarial complexity tests |
| Effect and template publication | Outside the authority guard; should not gate committed ownership | endpoint latency/circuit metrics and source-version rebuild counts |

Ready admission uses bounded batches of eight. Resolve and Verify use per-owner
round robin; Ready deliberately uses strict source/economic order without
aging. Profiling must not interpret that policy distinction as a scheduler
bug, but adversarial tests must retain Remote expiry and bounded work.

## 3. Existing evidence and its limits

Early fixed-binary experiments found the largest regression on cheap work and
the smallest on secp verification:

| Historical workload | Observed candidate vs `develop` | Admissible conclusion |
|---|---:|---|
| independent always-success, 1 peer / 8 workers | about `+56%` elapsed | scheduling/authority ceremony dominated cheap work |
| independent always-success, 4 peers / 8 workers | about `+41%` elapsed | peer concurrency amplified authority acquisition |
| dependent always-success chain | about `+30%` elapsed | causal wake/settlement added fixed latency |
| independent secp, 4 peers / 8 workers | about `+3%` elapsed | VM work hid most control-plane cost |

A later pre-UAK six-scenario run was approximately 17.6% below `develop` in
geometric throughput. Windowed samples attributed cheap multi-peer cost to
authority acquisition, stage publication/checkout and runtime parking rather
than verification compute. Several plausible local changes - extra wakes,
parallel mutation authorities, dirty scheduler state, copy-on-write dependency
sets and alternative projection Apply paths - were neutral or slower and were
removed.

Those numbers were captured on earlier type/state models and cannot be quoted
as current UAK throughput. Their surviving value is methodological:

1. measure cheap and cryptographic work separately;
2. keep wall time, CPU, parking, lock wait and lock hold distinct;
3. prefer deletion of work over caches or duplicate ownership;
4. profile a candidate before retaining or rejecting it;
5. use controlled paired A/B, not flame-graph percentages, for acceptance.

## 4. Reproducible profiling

The canonical runner is `tx-pool/scripts/profile.py`. It reuses benchmark
fixtures and measures the interval from target submission to the stable
completion callback. Fixture construction, cycle discovery and teardown are
outside that interval.

An admissible manifest records Git revision and tracked diff, feature set,
binary/harness/lockfile/workspace hashes, toolchain and profiler versions,
logical CPU count, platform/filesystem, flags, power/thermal state where
available, exact scenario, commands and nanosecond target window. Missing
identity is an error; `unknown == unknown` is never accepted.

Build once, capture one scenario, then reuse the exact hashed binary:

```bash
python3 tx-pool/scripts/profile.py capture \
  --output-prefix /private/tmp/txpool-independent-cold \
  --tx-type always_success \
  --pool-state cold \
  --dependency-order parent_first \
  --peers 1 --workers 8 --size 500 --warm-pool-size 100

python3 tx-pool/scripts/profile.py capture \
  --output-prefix /private/tmp/txpool-secp-cold \
  --binary /absolute/path/from/the/first/manifest \
  --tx-type secp256k1 \
  --pool-state cold \
  --dependency-order parent_first \
  --peers 4 --workers 8 --size 200 --warm-pool-size 50
```

Re-analysis verifies every artifact hash and does not execute CKB:

```bash
python3 tx-pool/scripts/profile.py analyze \
  --manifest /private/tmp/txpool-secp-cold.manifest.json
```

CPU sampling and feature-gated stage spans are separate executions of the same
hash-verified binary. This avoids injecting span formatting and file I/O into
the sampled run. Raw profiles may remain external, but the manifest records
their path, size and SHA-256 and the committed analyzer must reproduce the
summary.

### Tokio task observation

Tokio console complements CPU sampling with task lifetime, poll, wake and
resource behavior:

```bash
RUSTFLAGS='--cfg tokio_unstable' \
  cargo build --bin ckb --features profiling,tokio-trace --locked

TOKIO_CONSOLE_BIND=127.0.0.1:6669 \
TOKIO_CONSOLE_RETENTION=30s \
TX_POOL_PROFILE_TRACE_PATH=/private/tmp/txpool-span-close.log \
  target/debug/ckb -C /path/to/isolated-node run

tokio-console http://127.0.0.1:6669
```

Profiling is optional node telemetry, never a startup precondition. Invalid
addresses, paths, capacities, subscriber conflicts or telemetry task failure
must degrade locally. The default build contains no profiling subscriber work.

On macOS, `cargo-instruments` may corroborate a result when a full Xcode
installation exposes `xctrace`; absence is recorded as unavailable, not
evidence.

## 5. Required scenario matrix

P10 must cover both ordinary and adversarial CKB shapes:

| Family | Required dimensions | Purpose |
|---|---|---|
| independent always-success | cold/warm, 1/4 peers, 1/8 workers | authority and scheduler ceiling |
| independent secp256k1 | cold/warm, 1/4 peers, 1/8 workers | useful validation parallelism and CPU scaling |
| dependent chains/forest | parent-first and child-first, shallow/deep | wake latency, causal serialization and frontier work |
| RBF/conflict | accepted victim closure, reject, winner failure/history recovery | atomic replacement cost and bounded history |
| full pool/eviction | near configured limits and hostile causal closure | cold-path asymptotic bound |
| reorg | blank fork, recovered tree, large bounded fork | ordered reconciliation and proposal/template convergence |
| template | independent suffix, deep dependency tree, conditional cycle, uncle pressure | packing quality and derived graph bounds |
| peer pressure | many owners, large-cycle backlog, ban/refetch | fairness partitions, budget and cleanup cost |

Static operation/complexity tests must close any issue inferable from code
before this matrix runs. Benchmarking is not correctness discovery.

## 6. Fixed-binary A/B acceptance

Follow [`BENCHMARK.md`](BENCHMARK.md). Build candidate and baseline once in
separate equal-length worktrees, hash both binaries, cool down after build and
run adjacent balanced AB/BA pairs. Setup, cache state and teardown must match;
the runner rejects differing toolchain, lockfile, workspace profile, flags,
CPU count, harness or environment identity.

The final comparisons are:

1. final UAK versus `develop`, establishing necessity did not cause an
   unexplained regression and identifying any workload where the architecture
   now exceeds it;
2. final UAK versus the frozen pre-performance UAK checkpoint, attributing the
   retained optimization value;
3. absolute operation counts and profiles for adversarial shapes that a common
   throughput ratio cannot expose.

Quick mode is diagnostic and uses its documented 2% threshold/noise limits.
Medium or full repeated records close P10; full may be run on a more suitable
host, but all artifacts must satisfy the same fingerprint contract. A noisy or
unexplained result blocks the claim; it does not authorize a compensating cache,
retry path or second authority.

## 7. Reviewer questions

- Does the change remove work, or transfer it to another queue, cache, task,
  failure domain or shutdown path?
- Is its safety premise represented by ownership/types and exact evidence?
- Does it preserve independent validation and template-lane concurrency?
- Is hostile work bounded independently of unrelated pool size?
- Do profiles explain the result, and does controlled paired A/B confirm it?
- Was a neutral or shape-regressive prototype removed completely?
- Are the measured sources exactly the final reviewed sources?
