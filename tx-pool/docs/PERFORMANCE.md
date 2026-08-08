# Tx-Pool Performance Contract and Evidence

This document is the reviewer-facing performance companion to
[`ARCHITECTURE.md`](ARCHITECTURE.md). It records the current UAK performance
model, reproducible profiling method, evidence strength and final acceptance
matrix. Machine-contract maintenance is defined by
[`VALIDATION.md`](VALIDATION.md). It is not an implementation diary.

The current architecture has not yet passed either the M0-M4 mathematical/
refinement/minimality gate or the later controlled performance gate. New
timing experiments are paused until the semantic model derives a complete
candidate; benchmark results cannot reveal that a required concurrency
mechanism is absent. Historical profiles below explain why mechanisms were
considered or rejected, but they are not a release verdict for the final
source. The pre-performance checkpoint `4135df3c7` passed 415/415
internal-feature tx-pool tests and the complete 150/150 managed integration
universe. Any later semantic change reopens its affected correctness gates
before performance evidence can be accepted.

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

## 2. Current implementation cost model

The mathematical cost algebra in `ARCHITECTURE.md` is normative. The table
below describes costs in the current implementation and remains evidence for
M3; it does not prove that the current stage/task topology is minimal. The
performance review focuses on four measurable costs:

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

### 3.1 Historical P10 candidate evidence

Product checkpoint `6f486cdf5` replaced the heterogeneous global authority
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

### 3.2 I2 checkpoint Apply cost and semantic obligations

The I2 checkpoint's common successful Remote lifecycle had an exact topology
cost, but not a five-Apply semantic lower bound. The distinction is the input
to the implemented I3 exchange, which batches commutative transitions without
weakening any obligation:

| Boundary | I2 checkpoint authority work | Required semantic fact | I3 realization |
|---|---|---|---|
| Retained admission | one owner/charge/dedup Apply per immediately available homogeneous prefix | hostile work is owned and charged before compute | I2 implements one ordered count/byte-bounded Apply without delaying a prompt item |
| Compute checkout | one `Queued -> Computing` Apply per item | the owner and unique compute capability move atomically | one exchange issues the complete immediately available compatible wave in one Apply |
| Resolve/Verify handoff | no extra Apply on continuous Resolve-to-Verify; otherwise a settlement and later checkout | evidence and capability remain exact | continuous execution and bounded multi-completion settlement may remove topology-only cuts |
| Verified completion | one `Computing -> Ready` Apply per item | compute capacity is released before unrelated final membership work | the canonical available completion cut settles together and may refill in the same Apply |
| Ready membership | one Apply per non-empty Ready slice of at most eight | final validation, membership, resources and effects commit atomically | one commuting strict-priority prefix already shares this Apply |
| Ordinary effect publication | external I/O followed by one settlement Apply per immutable effect batch | endpoint progress cannot reinterpret the committed transition | several logical records in one batch require only one settlement Apply |
| External endpoints | no authority guard during I/O | fixed boundary | none |

For `N` homogeneous, chain-backed independent owners with continuous
Resolve-to-Verify execution, no stale/pressure path, `A` immediately available
homogeneous ingress cuts and `R` non-empty Ready slices, the I2 implementation
performs

```text
authority_applies(A, N, R) = A + 2N + 2R
ceil(N / 8) <= R <= N
```

The ingress term is one Apply per cut; the two per-item terms are checkout and
completion. The two per-slice terms are membership and settlement of the
resulting immutable effect batch. A prompt single item therefore uses five
Applies, while one full eight-owner Ready slice in one ingress cut uses 19
total Applies. Logical effect-record count is not effect-settlement Apply
count.

This equation is an executable description of the I2 checkpoint, not a
semantic minimum and not a timing model. Generation reset, RBF, missing
dependency, resource rejection, stale work and effect-capacity fallback are
outside its scope. The M3.5 model and production contract checker bind the
equation to the actual batch, stage and settlement owners so later topology
changes cannot silently preserve obsolete prose.

The next candidates were reviewed together rather than accumulated as local
patches:

1. **Capability-routed shared compute batons - superseded by I3.** The
   historical prototype established the complete runnable predicate and
   removed duplicate notifications, but retained per-worker authority
   checkout and settlement. I3 preserves its level-triggered lesson with one
   coalesced compute prompt and a complete coordinator role probe, while the
   bounded exchange removes the remaining per-worker mutation ownership.
2. **Direct verified commit - measured and rejected.** The experiment reused
   the exact final validator and membership compiler and removed every Ready
   probe on eligible chain-backed work. It nevertheless held the compute
   permit and active-work charge through final membership, eliminating overlap
   between later verification and the Ready driver. Stable paired A/B regressed
   beyond the quick boundary, so the experiment was removed rather than
   compensated with another queue or actor.
3. **Claim-bound effect receipt - retained.** The existing sole publisher claim
   borrows the minimum committed FIFO/reset record without moving it to an
   active authority location. Settlement validates source, sequence, batch
   identity and progress before retaining or removing the record. A newer
   generation reset is a typed mutation-free supersession of an older receipt;
   it can never be overwritten or resurrected. The claim is lifetime-bound to
   the publication lease, so two live receipts are unrepresentable in safe
   production code.
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

#### M3.6 selected topology

The complete-candidate model selected the **bounded semantic exchange**. It is
the only candidate that preserves the single authority, Ready/effect overlap,
Direct permit priority and template concurrency while removing per-owner
write-side round trips from one available wave. This is now a normative design
decision, not another optional P10 prototype.

For homogeneous independent work, the frozen pre-I2 baseline, the I2
checkpoint and the implemented I3 static contract are:

```text
pre_i2_applies(N, R) = 3N + 2R
i2_applies(A, N, R) = A + 2N + 2R
i3_applies(A, W, R) = A + (W + 1) + 2R
```

`A` is the number of immediately available bounded ingress cuts, `W` is the
number of retained compute waves and `R` is the number of strict Ready/effect
slices. With one ingress cut, eight worker slots and full Ready slices, the
executable model gives:

| Owners | Frozen pre-I2 UAK | I2 checkpoint | Self-fused workers | I3 exchange |
|---:|---:|---:|---:|---:|
| 1 | 5 Applies | 5 Applies | 5 Applies | 5 Applies |
| 8 | 26 Applies | 19 Applies | 26 Applies | 5 Applies |
| 64 | 208 Applies | 145 Applies | 152 Applies | 26 Applies |

The one available wave counterexample rejects both inherited alternatives.
The selected design pays for one compute coordinator task, `P` one-slot worker
assignments, one `P`-slot completion transport and one additional join edge.
It adds no authority, authority lock, persistent state, timer or retry queue.
Those costs are bounded by the existing active-work capacity and are checked
against the exact capability conservation law. Throughput improvement remains
unclaimed until the production slices pass profiling and fixed-binary A/B.

### 3.3 Shared compute baton result

The first shared-baton prototype exposed a real liveness defect before any
performance result was accepted. A verifier could consume the only Small wake
while the submitting peer's active-work slot was still held by Resolve. The
Verify head then remained unchanged, so releasing the final slot emitted no
head-change edge and left every transaction charged in `Queued(Verify)`. The
fix derives the complete predicate:

```text
Runnable = QueueHead AND ResourceEligible
```

The existing ledger remains the sole resource authority. A decrease in total
preaccepted active work republishes retained compatible heads; every Remote or
per-peer release necessarily decreases that same total. An unrelated release
may therefore cause a conservative probe, but the work is bounded by completed
compute and creates no repair scan, timeout, watchdog or mutable wake state.

Checkpoint `17088e240` passed 421/421 isolated library tests. Its service-level
regression starts 32 fresh idle worker generations and submits the formerly
stranded one-peer burst to each. Four additional executions of the exact warm
Criterion shape (30 samples and a 10-second target window) all completed before
the timing comparison.

The balanced six-pair quick comparison reused baseline `6f486cdf5` and fixed
binary hashes `94db5131c9c7...` / `2777ac7f69bc...`:

| Scenario | Candidate delta | Paired relative MAD |
|---|---:|---:|
| cold always-success, 1 peer / 8 workers | `+0.37%` | `1.05%` |
| warm always-success, 1 peer / 8 workers | `-0.54%` | `0.29%` |

The throughput geometric mean was `-0.08%`; both shapes pass the quick 2%
diagnostic boundary. The baseline and candidate JSON SHA-256 values are
`942ed3616cae...` and `bf0ffad44b6c...` respectively.

Matched feature-gated profiles used the same 2,000-transaction independent and
500-transaction parent/child fixtures as section 3.1. Semantic work counts did
not change. Authority write probes fell from 26,005 to 20,097 for independent
work (`-22.7%`), 15,866 to 13,145 for parent-first (`-17.1%`), and 15,381 to
12,740 for child-first (`-17.2%`). Resolve, Verify, Ready work and effect counts
were identical on each pair. Single-capture wall and sampled CPU varied by
about `+0.9%` to `+2.9%`; those captures explain work and are not timing
acceptance evidence. The paired Criterion result owns the timing conclusion.

The candidate is retained because it closes a legal-input liveness failure and
removes measured failed probes without transferring authority or adding a
failure domain. It does not yet establish a general throughput improvement;
the final medium matrix remains the production gate.

### 3.4 Direct verified-commit result

Checkpoint `51b93a830` tested whether a fully chain-backed independent verifier
could reuse the canonical final validator and membership compiler while its
exact Computing capability remained live. The experiment introduced no second
policy or owner and passed 423/423 isolated library tests, strict all-target
Clippy, generated evidence validation and the complete 150/150 managed
integration universe.

The balanced six-pair fixed-binary quick comparison against retained
`17088e240` rejected the design:

| Scenario | Candidate delta | Paired relative MAD |
|---|---:|---:|
| cold always-success, 1 peer / 8 workers | `-3.4340%` | `0.5786%` |
| warm always-success, 1 peer / 8 workers | `-1.3487%` | `0.7949%` |

The throughput geometric mean was `-2.40%`, outside the 2% diagnostic
boundary. Baseline/candidate binary SHA-256 values were
`2777ac7f69bc...` / `fdf4c8f662ea...`; result JSON hashes were
`2deb921df45c...` / `debec9882d74...`.

Matched 2,000-transaction profiles showed that the candidate did remove
mechanical work: authority reads fell from 8,000 to 2,000, writes from 20,097
to 18,008, and Ready attempts/work from 4,000/2,000 to zero. Resolve, Verify
and effect work remained exactly 2,000 each. The result is therefore not a
hidden retry or measurement-noise explanation. Direct finalization lengthened
the bottleneck compute lifetime: it retained the compute permit and active-work
charge during final validation and membership compilation. The Ready handoff
instead releases compute first and overlaps final membership with subsequent
verification. Replacing it with a worker-owned handoff queue would recreate
the same boundary with a second owner and failure domain.

The direct path is removed and Ready is retained as a necessary pipeline
concurrency boundary as well as an authority state. This is an example where
fewer transitions and lower CPU work did not imply higher throughput; both
operation evidence and paired timing were required for disposition.

### 3.5 Claim-bound effect-receipt result

Checkpoint `e2f310f16` removes the effect log's resident `active` location and
checkout Apply. A committed queued record or coalesced reset remains the sole
owner while the claimed publisher holds an immutable `EffectReceipt`; one
exact settlement Apply advances or removes that resident. The implementation
adds no lock, task, queue, cache, timer, retry or fallback protocol, and the
four affected production files are net +1 line.

Before timing, the checkpoint passed 423/423 isolated library tests, strict
all-target Clippy, every generated/static contract and the complete 150/150
managed integration universe. The fixed-binary baseline is `d462c5154`
(`d90de5f52` is product-equivalent) and the candidate is `e2f310f16`; binary
SHA-256 values are `fd3b0e6eec18...` and `08c827cfced5...`.

Matched 500-transaction profiles show that the change converts exclusive
authority acquisitions into shared reads while retaining the semantic stages:

| Scenario | Baseline reads / writes | Candidate reads / writes | Semantic counts |
|---|---:|---:|---|
| cold independent | 2,000 / 5,056 | 3,000 / 4,003 | 500 Resolve, Verify, Ready work and effect publish |
| warm independent | 2,000 / 5,047 | 2,993 / 3,996 | 500 Resolve and Verify; one run coalesced 500 owners into 499 Ready/effect batches |
| cold parent-first dependency | 3,994 / 13,219 | 5,603 / 11,471 | 999 Resolve, 500 Verify/Ready work and 999 effect publishes |

The acquisition total changes little; the benefit is a smaller exclusive
critical-section surface, not skipped validation or publication. Single
profile wall/CPU values varied within about 2% and remain attribution evidence,
not timing acceptance.

The controlled quick A/B used independent records for every required shape:

| Scenario | Pairs | Candidate throughput | Paired relative MAD |
|---|---:|---:|---:|
| cold always-success, 1 peer / 8 workers | 8 | `+0.0341%` | `1.1774%` |
| warm always-success, 1 peer / 8 workers | 6 | `-0.0570%` | `1.4892%` |
| cold parent-first dependency, 1 peer / 8 workers | 6 | `+0.2570%` | `0.7615%` |

An earlier combined independent record was rejected because the cold ratio MAD
was 2.40%, above the 2% quick limit; it was not used as a regression claim.
The exact eight-pair cold rerun above closed that noise question. Baseline and
candidate JSON hashes are respectively `b5397e246199...` /
`1b928af0429c...` (cold), `e006a8f0e23f...` / `7967ca0a3dc7...`
(warm), and `e16af64a72a6...` / `ca68b0bd8b86...` (dependency).

The effect receipt is retained because it deletes an authority location and a
mutation protocol, makes publisher exclusivity type-visible, substantially
reduces exclusive acquisitions, and shows no measurable regression in any
accepted paired shape. The data does not establish a general throughput gain;
the final medium comparisons remain the production gate.

### 3.6 Current final A/B status

The final/pre-performance 4-peer cheap shape has not produced admissible wall-
throughput evidence on the current host. The strongest completed record uses
ten balanced samples, each aggregating four immutable-binary target windows per
side (128,000 target transactions per side and sample). All 82 processes passed
identity, completion and temporal-evidence checks. The paired wall-ratio MAD is
1.6952%, above the unchanged 1.5% limit, so its apparent `+0.9920%` median is
not a performance claim. The artifact is
`/private/tmp/txp-medium-prepf-final-p4-v6.json`, SHA-256
`96217b6e180f959fc5743fd80ebb95bbde324a34490fb6057bb3fa1f62d6a3d6`.

CPU work is stable enough for attribution, not acceptance: paired CPU-ratio
MAD is 0.5387% and the final/pre-performance median CPU ratio is 0.98285. Raw
attempts expose multi-minute host scheduling regimes, including one baseline
window at 3.23 average CPU parallelism immediately followed by a candidate
window at 3.76. Removing the outlier, increasing aggregation, weakening the
gate or repeating until a pass would bias the verdict. P10 therefore remains
open for a controlled-host run of this exact protocol before final/develop is
measured. This is an evidence-environment blocker, not a demonstrated product
regression or improvement.

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
| `tx_pool.effects.publish` | One immutable committed effect batch, never the permanent publisher task. |

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
