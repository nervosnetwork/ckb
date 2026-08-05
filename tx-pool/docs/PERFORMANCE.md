# Tx-Pool Performance Contract and Evidence

This document is the reviewer-facing performance companion to
[`ARCHITECTURE.md`](ARCHITECTURE.md). It records the current UAK performance
model, reproducible profiling method, evidence strength and final acceptance
matrix. It is not an implementation diary.

The current architecture has not yet passed P10. Historical profiles below
explain why mechanisms were selected or rejected, but they are not a release
verdict for the final source. The pre-performance checkpoint `4135df3c7`
passed 415/415 internal-feature tx-pool tests and the complete 150/150 managed
integration universe. Any later semantic change reopens its affected
correctness gates before performance evidence can be accepted.

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

### 3.1 Current P10 candidate evidence

Product checkpoint `6f486cdf5` replaces the heterogeneous global authority
mutation broadcast with role-specific hints derived from one committed
before/after authority projection. The hints contain no work identity or
decision state. Resolve, capability-specific Verify, Ready, dependency
maintenance and effect publication use wake-one only where the selected role
can service the level; heterogeneous effect-capacity waiters and optimistic
template lanes retain broadcast.

The cross-checkpoint workload is byte-identical on both sides (harness SHA-256
`0efe6eef81c4be10473785bd204bb7dbf695f483ea2036668fe683dc5e473702`).
The baseline binary is the frozen `f78636e90` UAK and the candidate binary is
the same harness over `6f486cdf5`. A balanced six-pair diagnostic produced a
`+1.84%` throughput geometric mean. Two initially noisy shapes were repeated
for ten balanced pairs rather than accepted or rejected from the first sample:

| Scenario | Candidate vs `f78636e90` | Paired relative MAD |
|---|---:|---:|
| independent always-success, 1 peer / 8 workers | `+6.52%` | `0.52%` |
| independent always-success, 4 peers / 8 workers | `+1.57%` | `1.86%` |
| independent secp256k1, 4 peers / 8 workers | `-0.08%` | `0.24%` |
| dependency forest, depth 10, 4 peers / 8 workers | `+3.00%` | `0.84%` |
| parent-first fanout, 4 peers / 8 workers | `+2.77%` | `1.25%` |
| chain-backed fan-in 8, 4 peers / 8 workers | `+1.12%` | `0.85%` |

Exact feature-gated counters explain the result without relying on timing:

| Same 2,000-transaction workload | `f78636e90` | `6f486cdf5` | Change |
|---|---:|---:|---:|
| 1-peer authority writes | 154,350 | 26,005 | `-83.2%` |
| 1-peer authority reads | 19,915 | 8,000 | `-59.8%` |
| 1-peer Ready attempts / work | 8,679 / 1,995 | 4,000 / 2,000 | failed probes removed |
| 1-peer sampled CPU | 926,199 us | 627,561 us | `-32.2%` |
| 4-peer authority writes | 30,738 | 17,941 | `-41.6%` |
| 4-peer authority reads | 6,316 | 5,051 | `-20.0%` |
| 4-peer Ready attempts / work | 1,817 / 945 | 1,989 / 1,062 | smaller, earlier batches |
| 4-peer sampled CPU | 1,208,563 us | 1,124,267 us | `-7.0%` |

The 4-peer row exposes the remaining trade-off: removing irrelevant wakeups
reduces lock traffic, but a prompt Ready driver can observe smaller batches and
therefore emit more Apply/effect batches. This is evidence for separately
reviewing derived bounded coalescing or a true batch Plan/Apply path; it is not
permission to add a delay, mutable queue or heuristic to the wake router.

The candidate passed 418/418 isolated library tests, strict all-target Clippy,
the complete generated evidence checks, formatting and diff validation before
measurement. `PreparedApply::apply` remained about 0.4-1.3% inclusive in the
matched profiles, so the three fair-lane head reads and one Ready head read did
not become the next hot path. This candidate is retained, but P10 remains open
until later architecture candidates and the final medium fixed-binary matrix
are adjudicated.

Matched parent-first and child-first dependency captures reused the same fixed
profiling binaries and the exact 500-transaction, one-peer, eight-worker,
cold/warm-zero fixture. They are diagnostic operation-count captures, not a
throughput verdict:

| Dependency order | Wall change | Sampled CPU change | Authority writes | Ready attempts / work |
|---|---:|---:|---:|---:|
| parent-first | `-13.71%` | `-32.89%` | 57,388 -> 15,866 | 3,006 / 500 -> 1,000 / 500 |
| child-first | `-13.61%` | `-33.92%` | 60,302 -> 15,381 | 3,273 / 500 -> 1,000 / 500 |

Both candidate runs retained exactly 999 Resolve executions, 500 Verify
executions, 500 Ready work slices and 999 effect publications. The reduction
therefore removes failed scheduling probes rather than skipping dependency
semantics, and the child-first order does not acquire a hidden retry path.

### 3.2 Post-wake lower bound and candidate adjudication

The common successful Remote lifecycle now exposes a constructive lower bound.
This is a semantic bound, not a target obtained by weakening accounting or
publication:

| Boundary | Current common-path authority work | Constructive minimum | Disposition |
|---|---|---|---|
| Retained admission | one owner/charge/dedup Apply | one Apply | Required. Computing before ownership would recreate uncharged hostile work. |
| Compute checkout | one successful `Queued -> Computing` Apply plus capability-mismatched probes | one successful Apply | The lease and active-work charge are required; failed probes are not. Route one typed baton to one compatible worker. |
| Resolve/Verify handoff | zero extra Apply when a verifier continues Resolve into Verify, otherwise a queued-Verify settlement and checkout | zero extra Apply on the continuous path | Preserve the fallback, but make the compatible continuous path easier to select without creating a worker-owned queue. |
| Verified finalization | `Computing -> Ready`, two read cuts, then `Ready -> Accepted` | one final membership Apply | `Ready` remains the charged fallback for effect pressure, coupling and deferred work. A chain-backed independent completion may reuse the same validator and membership compiler directly; it must not create a second fast-path policy. |
| Ordinary effect publication | checkout Apply, external I/O, settlement Apply | one settlement Apply | The sole publisher can borrow a stable FIFO head because later appends and higher sequences cannot displace it. Generation-reset selection still requires active checkout state. |
| External endpoints | no authority guard during I/O | no authority guard during I/O | Fixed contract. |

For an ordinary independent transaction this gives a four-Apply steady-state
floor: admission, compute checkout, atomic final membership, and ordinary
effect settlement. It is not valid for a generation reset, RBF component,
missing dependency, resource rejection or effect-capacity fallback.

The next candidates were reviewed together rather than accumulated as local
patches:

1. **Capability-routed shared compute batons - selected first.** Resolve is one
   executable level shared by the resolver and verifier helpers. A typed wake
   reason makes the selected worker attempt that exact lane first. Small-Verify
   is shared by all verifiers; Large-Verify is consumable only by an Any
   verifier. This removes duplicate notifications and write-lock probes while
   adding no owner, queue, lock, task, timer or retry state.
2. **Direct verified commit - design-accepted after the baton result.** A
   successful chain-backed independent verification may produce a sealed final
   validation receipt while its exact Computing lease remains live, then use
   the existing membership compiler in one total Apply. Stale view, changed
   dependency cut, conflict/RBF, capacity pressure, allocation pressure and
   effect pressure must use closed outcomes and the existing charged Ready or
   re-resolve path. No validation rule may be copied into a second compiler.
3. **Stable queued-effect lease - design-accepted after finalization.** The
   existing sole-publisher claim can borrow the oldest ordinary queued envelope
   without a checkout mutation; settlement validates sequence, batch identity
   and progress before removing or retaining that same head. Generation reset
   keeps the current active checkout because a newer reset may replace the
   pending reset while I/O is in flight.
4. **Ready delay/coalescing - not selected.** Opportunistic batches already use
   the complete available prefix. Delaying a non-empty Ready level either adds
   a timer/local scheduling authority or can strand trusted work behind a
   paused or hostile active computation. True batch Plan/Apply remains open
   only if later profiles prove that committed settlement, rather than failed
   probes and redundant boundaries, is still dominant.

Before each prototype, the falsification set is frozen: mixed Resolve/small/
large levels, one-verifier topology, preexisting coalesced work, cancellation
and pause, effect saturation, queued/reset ordering, partial endpoint progress,
RBF/conflict, parent-first/child-first dependency order, peer revocation and
chain revision races. A candidate is removed if it adds a decision authority,
changes an externally visible outcome, loses progress under any of those
interleavings, or fails fixed-binary A/B after the complete correctness gates.

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

CPU sampling and feature-gated span counting are separate executions of the
same hash-verified binary. The sampled run installs no span subscriber. The
counter run uses a fixed registry of relaxed atomics only while target work is
active and writes one JSON artifact after completion; it performs no per-span
formatting, file locking or I/O. Counts are schedule-dependent control-flow
observations, not timing. Raw artifacts may remain external, but the manifest
records their path, size and SHA-256 and the committed analyzer must reproduce
the summary.

The UAK owns these low-cardinality profiling coordinates:

| Coordinate | Semantic boundary |
|---|---|
| `tx_pool.authority.read_wait`, `read_hold`, `write_wait`, `write_hold` | Every acquisition of the single authority store lock, generated by `AuthorityStoreLock` rather than copied at callers. |
| `tx_pool.authority.upgradable_read_wait`, `upgradable_read_hold`, `upgrade_wait` | The ordered chain-transition read cut and its atomic promotion to Apply. |
| `tx_pool.stage.resolve`, `stage.verify` | One checked-out resolution or verification operation. |
| `tx_pool.stage.ready_attempt`, `stage.ready_work` | Every level-triggered Ready probe versus the subset that captured a non-empty bounded slice. Their difference measures idle/coalesced wake cost without treating it as settlement. |
| `tx_pool.effects.publish` | One checked-out post-commit effect batch, never the permanent publisher task. |

With `profiling` disabled, `AuthorityStoreGuard` is the native parking-lot
guard type and no tracing subscriber or span is constructed. The production
contract checker owns the span-to-function mapping and rejects a missing or
bypassed producer before profiling. An empty target-window span artifact is an
infrastructure failure, not an admissible performance record.

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
