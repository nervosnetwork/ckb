# CKB Tx-Pool Pipeline Architecture

> The tx-pool runs exclusively in pipeline mode; the legacy serial processing path has been removed.

This document describes the implementation on the refactor checkpoint. The
security and migration acceptance matrix is tracked in
[`security-regression-ledger.md`](security-regression-ledger.md). Statements
marked as a known gap are requirements for the coordinator migration, not
guarantees of the current implementation.

---

## 1. Motivation

The tx-pool accumulated many features over time — RBF, orphan recovery, chunk-based verification, fee estimation, pipeline pre-resolve — each bolted onto the original serial processing model. The result was code that was hard to reason about, hard to parallelize, and hard to extend.

The pipeline architecture separates the transaction lifecycle into clearly bounded stages, each with its own concurrency model:

```
classify → pre_check → resolve → verify → submit
  (sync)   (parallel)  (serial)  (parallel) (serial, write lock)
```

This separation enables IO/compute isolation, parallel verification, configurable ordering, bounded backpressure everywhere, and a single consistency commit point.

---

## 2. Architecture

```
                    submit entry · remote / local / proposal
                                      │
                                      ▼
        ┌────────────────────────────────────────────────────┐
        │ 1. entry / classify                                │
        │    check_tx_basic_validity                         │
        │    classify_and_enqueue_tx[_spawn]                 │
        └───────────────────────┬────────────────────────────┘
                                │
        ┌───────────────────────┴────────────────────────────┐
           ▼ independent                        ▼ dependent
┌───────────────────────┐                     ┌───────────────────────┐
│ 2a. PreCheckQueue     │                     │ 2b. OrderedResolve    │
│     FIFO + id index   │                     │     Queue             │
│     64 MB             │                     │     FIFO + delay heap │
└──────────┬────────────┘                     │     64 MB             │
           │ pop → ActiveSet                  └──────────┬────────────┘
┌──────────▼────────────┐                                │
│ pre-check worker pool │                                │
│ ×min(workers, cores)  │                                │
└──────────┬────────────┘                                │
           └───────────────────────┬─────────────────────┘
                                   ▼ resolved
                    ┌─────────────────────────────┐
                    │ 3. RbfCandidates            │  only remote txs that
                    │    size-based fee-rate gate │  conflict with the pool
                    │    displace → RaceLost hold │
                    └──────────────┬──────────────┘
                                   ▼
                    ┌─────────────────────────────┐
                    │ 4. VerifyQueue              │
                    │    arrival_time / fee_rate  │
                    │    proposals first          │
                    └──────────────┬──────────────┘
                                   │ pop → ActiveSet
                                   ▼
                    ┌─────────────────────────────┐
                    │ 5. VerifyMgr                │
                    │    workers ×max(1, N)       │
                    │    generation cancel token  │
                    │    VM on blocking pool      │
                    └──────────────┬──────────────┘
                                   ▼
                    ┌─────────────────────────────┐
                    │ 6. submit_entry             │ ★ single commit point
                    │    tx_pool write lock:      │   (serial end of the
                    │    RBF check & removal      │    pipeline)
                    │    tip revalidation         │
                    │    escape-hatch (journaled) │
                    │    pool_map · limit_size    │
                    └──────────────┬──────────────┘
                                   ▼
                    ┌─────────────────────────────┐
                    │ 7. TxPool                   │
                    │    pool_map + links + stats │
                    └──────────────┬──────────────┘
                                   ▼
                    ┌─────────────────────────────┐
                    │ 8. terminal (out of lock)   │
                    │    after_process / terminal │
                    │    deferred worker:         │
                    │    RecoverTxs · CacheUpdate │
                    └─────────────────────────────┘
```

```
 ┌──────────────────────── WaitingRoom ─────────────────────────┐
 │  unified parking · per-reason budgets · FIFO eviction        │
 ├──────────────────────────┬───────────────────────────────────┤
 │ pipeline-side            │ pool-side                         │
 │  ParentsMissing (orphan) │  InputsBlocked (conflict)         │
 │  RaceLost (RBF held)     │                                   │
 ├──────────────────────────┼───────────────────────────────────┤
 │ orphan: 100 / 20 MB      │ conflicts: 10k / 50 MB            │
 │ RaceLost: shared resolved-lifecycle byte/count budget        │
 │ expiry 100 blk intervals │ no expiry (recovery / budget only)│
 │  · orphans  → woken by a parent landing in the pool          │
 │  · RaceLost → woken by the winner's terminal state           │
 │               (finalize → real reject, abort → restore)      │
 └──────────────────────────┴───────────────────────────────────┘
```

### Stage descriptions

**Entry / classify** — `check_tx_basic_validity` runs non-contextual verification and pipeline-wide duplicate checks (queues, waiting rooms, pool). `classify_and_enqueue_tx_spawn` routes dependent transactions (spending or cell-depending on in-flight outputs, tracked by `FlightTracker`) to the OrderedResolveQueue in arrival order, and independent ones to the PreCheckQueue. Errors are terminally routed through `after_process` exactly once — every rejection reaches a defined end state (relay, recent_reject, callbacks, or an explicit terminal sink for internal failures).

**PreCheckQueue** — FIFO queue with a 64 MB serialized-size budget and an id→job index for O(1) lookups (compact-block reconstruction queries it per short id). Drained by `min(max_tx_verify_workers, available_parallelism())` workers running `pre_check` (snapshot-only resolution on the lock-free fast path when all inputs are on-chain; tx_pool read lock only for pool-dependent inputs).

**OrderedResolveQueue / OrderedResolver** — dependent transactions wait here (64 MB). FIFO plus a delayed heap (`add_tx_delayed`) for bounded retries: local orphans retry with attempts capped separately for in-flight vs permanently-missing parents, and a permanently unresolvable local orphan ends in a terminal reject (recorded), never an unbounded retry loop. A single resolver preserves arrival ordering; popped jobs stay visible via the ActiveSet until `finish`.

**RbfCandidates (in-flight RBF gate)** — remote transactions conflicting with in-pool transactions register here before entering the verify queue, ordered by *size-based* fee rate (peer-declared cycles are unverified and must not influence ordering). Displacement is speculative (hold-and-restore): a displaced candidate is parked in the pipeline-side waiting room as the winner's `RaceLost` entry, not rejected. It becomes a real rejection only when the winner commits to the pool (`finalize`), and is restored to the verify queue whenever the winner leaves the pipeline (`abort`). A candidate that reaches submit while a stronger registration is still in flight is also held (`hold_superseded_candidate`, with a fee-rate re-check under the write lock). A winner that commits *without actually replacing anything* (its conflicts vanished meanwhile) aborts instead of finalizing.

**VerifyQueue / VerifyMgr** — resolved transactions awaiting script verification, ordered by `verify_ordering` (`arrival_time` FIFO by default, or `fee_rate`; proposals always first). Bounded by `max(max_verify_queue_tx_size, max_tx_pool_size)`. VerifyMgr spawns `max(1, max_tx_verify_workers)` workers from a shared `ChunkCommand` watch; worker 0 handles small-cycle transactions only. The script VM runs on the blocking pool via `block_offload`. Each manager generation runs on a child cancellation token: a dropped/panicked manager cancels its whole generation, so monitor respawns never double the worker count.

**submit_entry** — the single consistency commit point under the tx_pool write lock: RBF rule checks, conflict removal (`process_rbf`), tip-change revalidation (`check_rtx` + `time_relative_verify`), ancestor checks with the cell-ref escape hatch, pool_map writes, and `limit_size` eviction. Every physical removal carries its original `Pending`/`Gap`/`Proposed` status in one undo journal, including unrelated low-fee entries removed by `limit_size` before the new entry self-evicts. A failed submit restores that journal parent-first under the same write guard, before any candidate or callback can observe free inputs; it does not re-verify prior accepted entries through the pipeline. The superseded gate (`is_superseded`) is evaluated while holding the RBF read guard across the conflict computation and the write-locked submit.

**after_process / terminal outcomes** — every transaction that leaves the pipeline reaches a defined terminal state. `after_process` routes by source and reject kind: remote errors go through the ban/relay/record triple; `RBFRejected`/`Dead` park the transaction for conflict recovery (unless already held); a held `RaceLost` candidate is skipped entirely (its fate follows the winner). `terminal_reject` handles exhausted bounded retries (bypassing orphan parking), `terminal_internal` closes the loop for internal failures (worker panics) without recording. Recording rule (`should_recorded`): `Duplicated` and `Full` are exempt; terminal `RBFRejected` is recorded, while the speculative in-flight gates bypass recording at their call sites. Local (RPC) submissions with missing inputs are recorded as rejected; remote and proposal transactions with missing inputs are parked as orphans.

**DeferredTask worker** — bounded mpsc channel (`DEFERRED_CHANNEL_SIZE = 1024`) for opportunistic recovery re-enqueue (RecoverTxs, `.send().await`) and verify-cache updates (CacheUpdate, `try_send`). A failed RBF replacement is different: entries physically removed by the failed attempt are restored exactly inside the original tx-pool write transaction; they never enter the deferred worker. After the lock is released, the winner registration is finalized or aborted and every `RaceLost` owner is transferred before awaiting the bounded channel, so publication backpressure cannot prolong speculative ownership. The worker merges back-to-back non-critical recovery batches, retries transient `Full` backpressure with a bounded window that ends in a terminal outcome, and drains the channel on shutdown.

Every deferred recovery item carries the pipeline epoch of the submit that
created it. An administrative clear advances the epoch before draining any
structure, so a delayed recovery cannot resurrect pre-clear state behind the
drain.

**WaitingRoom** — unified parking structure with two instances split by lock domain: pipeline-side (`ParentsMissing` orphans and `RaceLost` RBF-held candidates) and pool-side (`InputsBlocked` conflict recovery, the retired conflicts LRU). Per-reason budgets cover orphans (100 entries / 20 MB) and conflicts (10k entries / 50 MB). `RaceLost` is not charged a second time by the room: its `ResolvedTx` carries a shared lifecycle permit that remains charged while queued, active, or held, closing the previous budget-refund bypass. FIFO eviction is per reason and expiry scans are watermark-gated. Orphans and RaceLost expire after 100 block intervals; expiry revokes a still-live speculative winner before restoring the loser, and a re-park retains the original expiry so a stalled winner cannot create an endless restore/reverify loop.

**WorkerRunner / ActiveSet** — all queue workers (verify, ordered resolver) share a runner skeleton: wait on command changes, queue notifications, or deadlines; pop one job at a time; process to completion. Popped jobs move into the queue's ActiveSet and stay visible (`contains_or_active`, `get_active_tx`) until `finish`, so "in flight" checks, RPC queries, and administrative removal never lose sight of mid-processing transactions. Each pop receives a monotonic active-lease token. Finish and same-stage requeue are token-conditional ownership transitions, so a late worker from before an RBF hold/restore cycle cannot erase the restored lease even within the same administrative epoch. Every job is wrapped in a panic guard; monitors respawn crashed workers with cancel-aware exponential backoff. A dropped command channel means a clean stop, not a busy loop.

**Pipeline epoch / administrative clear** — every pre-check, resolve, verify,
RBF aftermath and deferred-recovery job carries the epoch in which it was
admitted. `clear_pool` and `clear_pipeline` advance the epoch before acquiring
structure locks. Each ownership boundary rechecks it, and the authoritative
commit check occurs while holding `tx_pool.write()`. The clear path then waits
on the same lock as a commit barrier and drains all structures. Jobs that
linearized before the advance are either removed by `clear_pool` or retained by
the documented `clear_pipeline` semantics; jobs that did not linearize cannot
commit or reinsert afterwards. Epoch and active-token exhaustion are
fail-closed and never wrap.

**Dispatcher shutdown** — explicit cancellation, closure of the controller message
channel, and defensive dispatcher exits converge on one ordered shutdown tail:
cancel the pipeline, drain every in-flight message handler, quiesce and join the
background workers, then persist the accepted pool. The builder's startup-only
controller clone is dropped before `start` returns, so dropping the last
user-facing controller really closes the channel. Persistence never races a
still-running verifier or recovery worker, and completion of the dispatcher
handle is the durable shutdown boundary.

**Reorg** — `update_tx_pool_for_reorg` runs under `recovery_lock` for its authoritative mutation and retained-transaction recovery, so `save_pool` cannot intentionally observe a half-recovered pool. Detached transactions are filtered against attached transactions by raw transaction hash (not witness hash) and re-added per-transaction through `process_tx_direct_outcome` in topological order: `Committed` and `Duplicated` count as success, `Superseded` skips cascading (the transaction is merely held), and only a definitive failure cascade-removes dependents. Transactions committed in attached blocks leave every pipeline structure through the full terminal sequence. Reorg callbacks are deferred by an RAII batch whose outer guard outlives `recovery_lock`; reverse-order drop releases the recovery lock before invoking user code. Callback panic and controller re-entry therefore cannot deadlock or expose a half-mutated reorg state.

The controller applies backpressure instead of dropping a reorg delta when the
bounded channel is full. The handler retains the received head delta across
panics, retries it with cancel-aware exponential backoff, and never receives a
later delta first. Authoritative reorg operations converge when repeated;
registered callbacks are panic-contained at the side-effect boundary so they
cannot trap the retry loop. The target sequencer still replaces the coarse
`recovery_lock` with explicit prepare/commit/publish stages and must not run
external callbacks while holding recovery state.

**Block assembler** — the template lives behind a version counter; partial updates (`update_proposals`, `update_transactions`, `update_uncles`) swap via CAS while `update_full` and `reset_template` serialize under `template_lock` (with `update_uncles` joining that lock so a full update cannot revert a concurrent uncle update). A level-triggered dirty-bit journal is written before the bounded notification channel is used as a wake edge; a full channel can coalesce updates but cannot lose the only Pending/Proposed transition and strand a valid transaction. Template byte accounting uses the consensus `serialized_size_without_uncle_proposals` basis throughout; uncle candidates that do not fit are truncated to the longest fitting prefix instead of dropped wholesale; embedded or stale candidates are removed eagerly. Pending proposals take priority over optional uncles: an uncle carrying a selected proposal id (and any descendant that loses its only valid parent) is filtered atomically from that template, so miners may omit optional uncles without stranding the transaction. Management-triggered resets (e.g. `clear_pool`) notify miners immediately, like the reorg path.

**remove_tx (administrative)** — removes a transaction from every structure it may occupy (pre-check, ordered, verify queue plus its registration, both waiting rooms, the conflict cache, and the pool) and reports a tri-state outcome: `Removed`, `InProgress` (a worker is mid-flight on it — reported honestly instead of "not found"), or `NotFound`.

---

## 3. Optimizations

### 3.1 Service Actor Semaphore

The actor loop spawns a task per message, capped at `max_tx_verify_workers * MESSAGE_CONCURRENCY_MULTIPLIER` concurrent tasks via `Arc<Semaphore>`.

### 3.2 Shared ChunkCommand Channel

One shared `watch::Sender` for the chunk pause/resume signal; every worker clones the receiver. No layered forwarding.

### 3.3 DeferredTask Backpressure

Opportunistic conflict recovery and cache updates go through a bounded mpsc channel with a single sequential worker: `RecoverTxs` uses `.send().await`, while `CacheUpdate` uses `try_send` because a cache miss is acceptable. Back-to-back recovery batches merge and shutdown drains the channel. Failed-submit rollback is deliberately not deferred; it completes before RBF ownership is released, and that ownership settlement completes before any deferred send can block.

### 3.4 Lock-free Fast Path for On-chain Inputs

`pre_check` resolves from the chain snapshot without the tx_pool read lock whenever all inputs are on-chain.

### 3.5 FlightTracker Double Index

Forward `HashMap<OutPoint, ProposalShortId>` for `depends_on` lookups; reverse index for O(outputs-per-tx) removal.

### 3.6 RBF Replacement Failure Recovery

A rejected replacement rolls back completely: conflict removals, escape-hatch evictions, and every size-limit eviction merge into an exact dependency-sorted undo journal with original proposal-window statuses. Restoration finishes under the same tx-pool write guard, and spurious reject events for restored entries are suppressed. Only third-party transactions newly unblocked by the attempt use deferred recovery.

### 3.7 O(1) Queue Lookups

`OrderedResolveQueue` uses a `HashMap` lookup with generation-tagged tombstones; `PreCheckQueue` keeps an id→job index. All `get_tx`/`remove_tx`/`contains` paths are O(1), including compact-block reconstruction over thousands of short ids.

### 3.8 Push-based Dependent Wake-up

A newly submitted transaction wakes the ordered resolver and orphan waiters directly (`wake_ordered_resolver_if_needed`, `process_orphan_tx` with a batched parent-availability check), instead of relying on polling scans.

### 3.9 Configurable Verify Queue Ordering

`verify_ordering` selects `arrival_time` (default) or `fee_rate`; proposals always take priority in both modes.

### 3.10 Lock-free RecentReject

`RecentReject` lives outside the tx_pool lock as `Option<Arc<RecentReject>>` with sharded RocksDB TTL storage; reads take the shard read lock, writes upgrade only during shard shrink/recreate. A missing shard column family reads as "no entry" rather than an error.

### 3.11 Reorg Recovery Under recovery_lock

The authoritative reorg (write-lock section + retained-transaction re-add) holds `recovery_lock`; `save_pool` and `clear_pool` take the same lock, so normal completion is observed atomically by persistence and administration. Retained transactions re-enter through `process_tx_direct_outcome` with per-outcome handling (`Committed` / `Superseded` / `Duplicated` / failure), avoiding both write-lock stalls and false failure cascades. The ordered handler retains the head delta until success or shutdown and retries with backoff. Callback batches are released only after `recovery_lock` drops. This lock remains a legacy safety barrier rather than the target sequencer because direct recovery still runs while it is held.

### 3.12 Unified Verify-and-Submit Core

`verify_and_submit_core` is the single verify→submit path for pipeline workers, RPC, tests, and reorg recovery. A superseded candidate carries its authoritative `Completed` result in the owned `ResolvedTx` across `RaceLost`; the best-effort global cache update is only an optimization, so a saturated deferred channel cannot force a full re-verification after restore.

### 3.13 Active-set Visibility

Popped-but-unfinished jobs stay visible in every queue's ActiveSet, closing the pop→finish window for duplicate checks, orphan flight heuristics, RPC queries, and administrative removal (which reports them as `InProgress`). Monotonic pop tokens make completion ABA-safe across same-epoch hold/restore, while the pipeline epoch invalidates all pre-clear work at the final commit barrier.

### 3.14 FIFO Waiting-room Eviction

Waiting-room budget eviction uses per-reason insertion-order queues: O(1) per eviction (no full-table scan) and oldest-first semantics.

### 3.15 Saturating Counters with Recompute

All queue/pool size counters saturate and, on an underflow (an accounting bug elsewhere), restore the true total from the live collection instead of silently clamping to zero — budget enforcement can never be silently disabled.

### 3.16 Lock-sliced Restore

Bulk RBF restores process the worklist in 32-entry slices, releasing the `rbf_candidates`/`verify_queue`/`waiting_room` write guards between slices so a large restore cannot stall the pipeline.

---

## 4. Performance

Performance is a hard migration gate. The authoritative baseline must be
recorded from checkpoint `3ece94af1` (with the benchmark-only harness changes
applied equally to both sides). The comparison tool supports repeated runs, JSON
records, and a strict non-regression exit status. Parent-first and child-first
dependent paths are benchmarked separately. The measured closure ends at an
event-driven accepted callback; per-iteration service quiescence/destruction is
outside the timer, but teardown timeouts and task failures invalidate the run
instead of allowing work to overlap the next sample. A failing comparison
requires symmetric same-host/toolchain
records with at least three complete repetitions per side; a record whose
cross-run spread exceeds the configured noise ceiling is invalid rather than a
pass or regression. Quick mode is diagnostic, while repeated medium/full A/B
records are the migration acceptance gate. Baseline and candidate checkouts use
isolated Cargo target directories and must record an identical benchmark-harness
fingerprint, preventing stale cross-worktree executable reuse from producing a
false comparison.

Quick keeps only one peer/worker combination but uses 100-transaction
independent batches, 20-transaction dependency chains, and 20 Criterion samples.
Each fresh service completes a dispatcher readiness round-trip plus one short
scheduler interval before timing, so worker startup jitter is not mistaken for
pipeline latency. Its diagnostic defaults are an 8% paired-ratio spread ceiling and
a 2% directional regression threshold; medium/full keep the 5%/0% release gate.
The runner can filter benchmark IDs for a fast focused retest without weakening
the recorded harness/environment checks. Preferred quick A/B execution accepts
a baseline worktree and alternates adjacent baseline/candidate pairs (reversing
the order every second pair), reducing thermal and host-load drift without
mixing their Cargo targets. Verdicts use the median of adjacent
candidate/baseline ratios, and stability is assessed on those ratios rather than
on unrelated absolute host-speed movement. The paired stability bound is the
maximum relative deviation from the median ratio, matching the estimator used
for the verdict instead of double-counting both sides of a symmetric range.

Current measurement status: the old isolated quick harness was invalid as
release evidence: an adjacent same-binary rerun changed the always-success
median by roughly 11%. After the event-driven completion, readiness, larger
batch, isolated-target, and paired-execution fixes, a three-pair focused quick
A/B against checkpoint `3ece94af1` measured cold/warm always-success throughput
deltas of `+1.26%`/`+1.56%`. The maximum deviations from each median paired
ratio were `2.46%`/`2.15%`, below the quick diagnostic ceiling. This validates
quick as a fast directional regression check; it does not promote quick to a
release gate. A clean, isolated, repeated medium/full checkpoint A/B is still
required before the coordinator enters the production hot path. A later medium
run was stopped on operator request after two complete adjacent pairs and one
incomplete pair because its wall-clock cost was too high; it is deliberately
recorded as **no verdict**, not combined with the focused quick result.

The numbers below are historical reference values from the original pipeline
benchmark run; they are not a substitute for the checkpoint A/B record.
Benchmark environment: Apple M-series (arm64), macOS, Rust 1.95.
Matrix: MEDIUM (100 tx/batch, peers [1, 4], workers [4, 8], 30 samples, Criterion 95% CI).

### 4.1 secp256k1 Transactions (CPU-intensive verification)

**Cold pool (empty pool, submit 100 txs):**

| Scenario | Pipeline (ms) |
|----------|---------------|
| 1 peer, 4 workers | 62.15 |
| 1 peer, 8 workers | 51.15 |
| 4 peers, 4 workers | 62.90 |
| 4 peers, 8 workers | 50.84 |

**Throughput (secp256k1, tx/s):**

| Scenario | Pipeline |
|----------|----------|
| 1 peer, 8 workers (cold) | 1,955 |
| 1 peer, 8 workers (warm) | 1,907 |
| 4 peers, 8 workers (cold) | 1,967 |
| 4 peers, 8 workers (warm) | 1,910 |

**Analysis:**

- **8 workers outperform 4 workers**: the blocking pool parallelizes CPU-heavy secp256k1 verification across cores.
- **Warm vs cold**: negligible difference — pool state lookups are O(1) and do not affect pipeline staging.

### 4.2 Dependent Chain Transactions

10-deep dependency chain, testing CPFP latency:

| Scenario | Pipeline (ms) |
|----------|---------------|
| 1 peer, 4 workers, secp256k1 | 20.69 |

Push-based dependent wake-up re-enqueues each child immediately when its parent lands; latency scales linearly with chain depth × single-tx resolve time.

---

## 5. Migration Direction

The audited target keeps the fixed parallel pre-check/resolve/verify workers,
but replaces every pre-pool queue, active set, waiting room and speculative RBF
owner with one `PipelineCoordinator`. It does not put an actor hop in front of
workers and it never mirrors accepted `TxPool` entries.

The first isolated model split lifecycle, dependency and conflict state across
three components. Review found that this repeats the original architectural
mistake: all three components independently store locations such as ready,
dispatched, waiting and committing, and all three issue generations. They
cannot be composed atomically, so they are prototypes only and are blocked
from production integration.

### 5.1 Target-model review findings

| ID | Finding | Required correction |
|---|---|---|
| A1 | `LifecycleStore`, `DependencyScheduler` and `ConflictScheduler` each own overlapping transaction state and generations. A transition can succeed in one and fail or become stale in another. | One coordinator entry is the only state and generation authority. Dependency/conflict structures become indexes derived from that entry. |
| A2 | The lifecycle prototype permits `Committed` terminalization from any location, and administrative removal can also request `Committed`. | Make commit handoff a dedicated typed operation available only for `Committing`; administrative dispositions cannot express `Committed`. |
| A3 | `DependencyScheduler::pop_ready` removes the live ready ticket before allocating its next generation. Generation exhaustion leaves a `Ready` record with no schedulable ticket. | A queue pop and lease transition are one coordinator mutation; all fallible preflight occurs before the live queue slot is consumed. |
| A4 | Capacity wake-up repeatedly scans stale queue prefixes, while compaction counts the global blocked set for every stage on every mutation. Small wake batches can become quadratic under attacker-controlled churn. | Do not reproduce payload queue capacity in the coordinator: entries are already residency-charged and stage queues hold bounded ID tickets only. Any retained lazy queue consumes stale prefixes monotonically with O(1) live counts. |
| A5 | Scheduler audits compare logical sets but do not prove that every live ready/blocked ticket has a physical queue slot. They can report success for a liveness failure. | The coordinator audit checks entry↔index↔physical-ticket bijection for every schedulable state. |
| A6 | Registration-time replacement eligibility can become stale while a candidate waits. Treating it as a durable proof would reintroduce a fee-floor TOCTOU. | Registration is only an admission/ranking filter. The commit sequencer recalculates the complete RBF closure and fee requirements under the authoritative pool write guard. |
| A7 | The migration draft nested the coordinator before `TxPool`, which would block all queue transitions for the duration of pool validation and creates an unnecessary hot-path lock convoy. Adding a second mutation lock would also tax every commit. | Reuse the existing `TxPool` write guard as the membership sequencer. The only nesting is `TxPool → coordinator`; no coordinator path waits for `TxPool`, and no extra hot-path gate/actor is added. |
| A8 | Payload bytes were charged, but dependency/conflict edges, tickets and deadlines had independent limits rather than one peer-attributable residency budget. Many tiny transactions could maximize every metadata limit simultaneously. | Charge payload plus conservative metadata cost continuously to global and per-peer budgets; retain hard entry/edge caps as secondary defenses. |
| A9 | Pre-verification conflict ownership lets a stream of invalid high-fee candidates repeatedly displace honest verified work. Expiry bounds one hold but not repeated censorship. | Unverified candidates may be ordered for verification but cannot own/preempt a conflict domain. Only successfully verified entries enter reversible conflict scheduling. |
| A10 | A parked `ResolvedTx` retains an `Arc<Snapshot>`. Repeated waits across tips can pin many historical snapshots and their backing state. | Snapshots are transient worker inputs. Parked entries retain tip identity and resolved/verified data only; checkout reacquires/revalidates the current snapshot. |
| A11 | Parent fan-out, descendant failure and conflict waiter rebalance can touch the global entry count while holding the coordinator mutex. Entry/edge caps bound memory but not lock latency. | Add per-bucket fan-out caps and reliable bounded maintenance batches. Mark invalid roots/children before yielding so stale workers cannot commit ahead of deferred cascade work. |
| A12 | A query that checks `TxPool` and then the coordinator can observe neither side during a successful ownership transfer. | A cross-authority query holds the existing pool read guard while taking a short coordinator snapshot (`TxPool → coordinator`), so the writer cannot expose the handoff gap. |
| A13 | Publishing effects directly from the committing task can lose or reorder callbacks/relay notifications if it is cancelled after state finalization. An unbounded outbox would turn a slow consumer into memory exhaustion. | Finalization appends a sequence-ordered batch to a bounded, pre-reserved effect outbox before releasing the pool write guard. Publication is outside state locks, panic-contained and drained on shutdown. |
| A14 | A count-bounded outbox can still retain unbounded payload bytes, and releasing lifecycle accounting at terminalization creates a second residency gap. | Effect batches contain minimal IDs/outcomes, have count and byte limits, and inherit any unavoidable payload charge until publication releases it. |
| A15 | Coordinator conflict membership can change while pool validation runs with the coordinator unlocked, making an exact waiter list captured at prepare stale. | `Committing` freezes every input domain; later verified contenders enter its capped waiter buckets. Finalization consumes/reclassifies the current bucket without allocation, while abort schedules bounded rebalance. |
| A16 | Even with complete preflight, a panic in a multi-entry/index apply could leave half a coordinator batch applied. | A transition undo guard retains old authoritative entries until commit. Unwind restores entries and rebuilds derived indexes; production code contains no panicking invariant accessors. |

These defects do not affect the current production path because the prototypes
are unreachable (`#![allow(dead_code)]`). They invalidate the prototypes as an
integration base. Their useful test cases are retained as requirements and
must move to the single-coordinator model before the prototype modules are
deleted.

### 5.2 Single authoritative coordinator

`PipelineCoordinator` contains one short-held synchronous mutex around
`CoordinatorInner`. Every admitted full transaction hash maps to exactly one
entry. Proposal short IDs are collision-checked secondary indexes only.

The entry uses payload-phase-aware typed states so illegal combinations are not
representable:

- raw entries may be queued/active in pre-check or resolve, wait for parents,
  or wait for a retry deadline;
- resolved-unverified entries may only be queued/active in verify; they can be
  prioritized by preliminary fee score but cannot own a conflict domain;
- verified entries may wait for accepted-pool inputs or verified speculative
  conflict blockers, be ready to commit, or carry a unique committing lease;
- accepted `Pending`/`Gap`/`Proposed` states exist only in `TxPool`; successful
  commit consumes the committing entry rather than changing it to a shadow
  pool location.

One monotonic incarnation/revision pair supplies every worker, queue, deadline,
dependency and conflict ticket. Auxiliary structures contain full hashes plus
that same revision; they never have an independent state or generation.
Indexes include short ID, peer, location, dependency parent, blocked pool
input, speculative blocker, expiry/deadline and stage-ready order. Queue tickets
may be lazily stale, but every live queued entry has exactly one physical live
ticket and stale storage is bounded by amortized compaction. Stage queues carry
IDs only and are bounded by the same maximum entry count as the store, so a
successful internal transition cannot fail merely because another payload
queue has a separate byte budget. The old `CapacityBlocked` lifecycle therefore
disappears instead of being reimplemented.

Expiry tickets use `(deadline, full_hash, incarnation)`, not the changing
revision. Normal state transitions cannot accidentally invalidate an entry's
original lifetime; removal and re-admission get a new incarnation and make the
old deadline harmlessly stale.

Payload ownership is also singular. Indexes never clone `TransactionView` or
`ResolvedTx`; worker leases clone only an `Arc` to immutable work data. A raw
entry is transactionally replaced by a resolved-unverified entry after
resolution, and verification replaces that with a verified entry carrying its
owned proof. Snapshots never become resident payload: the worker lease obtains
the current `Arc<Snapshot>` transiently and the entry retains only the tip hash
needed for final revalidation. Its exact resolved charge is computed outside
the lock and recharged atomically; if it cannot fit, the result is a defined
`Full` terminal outcome rather than an unbudgeted capacity wait. Proposal
admission/recharge has validated reserved headroom (or deterministically evicts
remote entries) so remote saturation cannot prevent chain reconciliation.
Local RPC remains globally bounded because it may be exposed to untrusted
clients; it does not receive an unlimited trusted bypass.

The residency charge covers serialized payload/accounted heap data plus
conservative index metadata and remains live across queued, active, waiting and
committing states. Global and per-peer count/byte/active-work caps are checked
in the same admission/recharge transaction. Per-peer active-work limits and a
bounded fair peer rotation prevent one peer from occupying every worker with a
FIFO prefix; proposal priority remains absolute and fee ordering remains the
configured policy within eligible verify work.

The required transition families are deliberately small:

| From | Event | To / outcome |
|---|---|---|
| absent | admit independent/dependent | raw `QueuedPreCheck` / `QueuedResolve` |
| raw queued | checkout | raw `Active(stage)` plus versioned worker lease |
| raw active | resolved | resolved-unverified `QueuedVerify`, with atomic recharge; preliminary fee score affects order only |
| raw active | parents missing | raw `WaitingParents`; the parent reverse index is updated in the same transition |
| raw waiting | final parent available | raw `QueuedResolve` exactly once |
| unverified queued | checkout | unverified `ActiveVerify` plus versioned worker lease and transient current snapshot |
| unverified active | verification success | verified `ReadyToCommit` or `WaitingConflict`; conflict ownership is decided only now |
| verified ready/waiting | stronger verified arrival | deterministic all-input rebalance; an already `Committing` owner is frozen |
| waiting conflict | blocker abort/remove | atomically rebalanced `ReadyToCommit` or continued `WaitingConflict` without re-verification |
| verified entry | accepted input freed | revalidated `ReadyToCommit`/`WaitingConflict`, reusing proof only when its resolved inputs remain identical |
| ready | begin authoritative mutation | unique `Committing` lease |
| committing | pool apply succeeds | consumed as typed `Committed` handoff; direct speculative conflicts are consumed as `Rejected` in the same batch |
| committing | pool apply fails | journal restored first, then explicit requeue/wait/reject disposition |
| any pre-pool state | remove/clear/peer ban/expiry | consumed terminal record; stale workers become harmless no-ops |
| raw or resolved dependent | parent becomes unavailable/definitively fails | atomically demote to raw re-resolve/wait or terminal cascade; resolved snapshot/proof is never reused across invalidated dependencies |

Duplicate admission never creates another entry. A proposal notification may
promote an existing entry's priority, and a local submission may attach its
completion observer, but neither changes full-hash identity nor duplicates
payload/accounting. Remote peer attribution is retained until a trusted source
promotion explicitly releases it, preventing duplicate submissions from
moving charges between peers.

#### Current isolated implementation status

The first single-authority checkpoint is implemented in
`component/pipeline_coordinator.rs` together with the bounded
`component/effect_outbox.rs`. It is deliberately unreachable from the
production hot path. The coordinator currently covers one full-hash owner,
typed raw/unverified/verified payload replacement, versioned stage and commit
leases, dependency wake/invalidation, verified-only conflict scheduling,
multi-input committing freeze, typed commit handoff, global/per-peer payload
residency, physical queue-ticket audit and clear/removal. The outbox covers
continuous count/byte reservation, mutation-order sequence binding, FIFO retry
and active-publication residency. The coordinator is split by responsibility
into state types, derived indexes and invariant audit modules while retaining
one entry store and one transition authority. The complete tx-pool unit suite
currently contains 30 coordinator and 5 outbox focused tests.

Source promotion, incarnation-scoped expiry, accepted-pool-input waiting,
conservative metadata charging and global/per-peer active-work fairness now
live in the same model. Stage queues use proposal/normal FIFO lanes and
round-robin peer buckets; a trusted-source promotion transactionally retickets
the queued entry, so the entry, live set and physical lane never disagree. A
deterministic 4,000-step state-machine test audits every generated transition
and found the missing reticket edge during development.

This checkpoint is not a production migration claim. Before raw cutover, the
model still needs configured fee-priority ordering within eligible peer heads,
bounded dependency-cascade maintenance, wider property/fault coverage and the
multi-entry undo guard. Before mutation cutover it additionally needs
coordinator/outbox charge transfer, final in-lock pool RBF recalculation,
cross-authority query tests and production publisher/shutdown integration.
The existing split prototypes remain test oracles only until their remaining
properties are ported; they must then be deleted rather than hardened or
integrated.

### 5.3 Atomic transition engine

Every coordinator API is a complete domain transition rather than a public
collection primitive. Examples are `admit_raw`, `checkout_stage`,
`complete_resolution`, `wait_for_parents`, `parent_available`,
`register_conflict`, `begin_commit`, `abort_commit`, `remove_peer`, `clear` and
`apply_reorg_delta`.

Each operation follows the same rule:

1. validate the full hash, incarnation/revision, typed source state, budgets,
   generation capacity and the whole affected batch without mutation;
2. update entries and all auxiliary indexes under the one coordinator lock;
3. return immutable worker leases, terminal records and a typed effect journal;
4. publish notifications/callbacks only after releasing all internal locks.

No helper is allowed to mutate a relationship index independently. Batch
operations reject duplicate hashes before applying the first member. Revision
or counter exhaustion fails closed without consuming a live queue ticket.
Administrative removal, clear, peer ban, expiry and reorg use the same APIs, so
there is no cross-structure scan that can miss an active or parked owner.

Hot-path transitions are O(1), O(log n) for configured priority indexes, or
O(the transaction's bounded dependency/conflict degree). Per-parent and
per-blocker fan-out have explicit caps. Larger cascade/rebalance work is placed
on an ID-only maintenance deque and drained in bounded batches. Before the lock
is released, every discovered failed child is marked invalid as a possible
parent and every affected active lease is revision-invalidated; deferred BFS
work can delay cleanup but cannot allow a descendant to commit. The maintenance
deque is level-triggered, bounded by coordinator entry/edge limits, retained
across worker panics and drained during shutdown.

The invariant auditor reconstructs every index, budget and physical live queue
membership from entries. In tests it runs after every model transition; in
production it is sampled/rate-limited and reports metrics without repairing
state silently.

Each apply batch owns a bounded undo guard containing the prior authoritative
entries (payloads are shared `Arc`s). It is disarmed only after entry and index
changes pass the invariant boundary. If an injected panic unwinds an apply,
the guard restores those entries and marks derived indexes for deterministic
rebuild before the next operation. This recovery path is tested; ordinary
production accessors return typed invariant errors instead of `unwrap`,
`expect` or `assert` inside the coordinator lock.

### 5.4 Authoritative pool transaction

The existing `TxPool` write guard remains the sole hot-path membership
sequencer for normal commit, RBF, attached/detached block application, clear
and administrative pool removal. No second lock or actor hop is added to every
transaction. Persistence/reorg may retain a separate chain-operation guard only
for work that genuinely spans multiple pool-lock sections; it is not acquired
by normal commits. CPU-heavy resolution and script verification remain
parallel outside all pool locks.

RPC, compact-block and persistence reads that need a combined accepted/pre-pool
view hold the existing pool read guard while taking a short coordinator
snapshot. The universal nested order is therefore `TxPool → coordinator`.
Coordinator-only queue/wait transitions never acquire `TxPool`.

A normal commit is one prepare/apply/finalize/publish transaction:

1. **Acquire.** Reserve one bounded effect-outbox slot, then acquire the
   existing `TxPool` write guard. No lifecycle state has changed at either
   asynchronous wait point, so cancellation is harmless.
2. **Prepare.** Briefly lock the coordinator while already holding `TxPool`,
   validate the verified lease/current conflict winner, freeze it as a unique
   committing lease, freeze its input domains and reserve bounded
   success/failure work. A later verified contender can join only the capped
   waiter buckets of that committing owner; it cannot alter the pool plan.
   Release the coordinator lock; prepare/finalize are the only two bounded
   `TxPool → coordinator` sections.
3. **Apply.** Under the existing `TxPool` write guard, revalidate tip,
   transaction context, the current complete RBF conflict closure, replacement
   fee and size-based fee-rate. Perform all fallible work before destructive
   mutation where possible. The remaining mutation uses an exact undo journal
   containing every removed entry and original pool status. Failure restores
   the journal before releasing the pool guard.
4. **Finalize.** While the pool write guard still excludes every competing
   membership mutation, finalize the preflighted coordinator batch. Success consumes the winner as
   `Committed` and direct speculative losers as `Rejected`; failure returns or
   terminalizes the winner from an explicit disposition. Finalization performs
   no allocation-dependent or externally fallible work. If both locks are
   briefly held, the sole legal nesting is `TxPool → coordinator`; coordinator
   code never waits for `TxPool`.
5. **Journal and publish.** Before releasing the pool write guard, append the
   typed effect batch to the already-reserved count/byte-bounded FIFO outbox.
   Batches contain minimal hashes, sources and outcomes; any unavoidable
   payload residency charge moves from the coordinator entry to the outbox
   record until publication. Then release every state lock. The supervised
   publisher emits callbacks, relay result,
   cache update, miner dirty bits, metrics and dependency wake edges in sequence
   order. Publication cannot choose or repair an ownership outcome.

The committing lease plus pool write guard is the clear/reorg/remove
linearization boundary. There is no `.await` from lifecycle prepare through
pool apply, coordinator finalize and outbox append. The undo/finalize guards
retain enough preflighted ownership to roll both structures back if an injected
panic occurs before the transaction is armed as stable. On startup/debug audit,
any impossible pool/coordinator residue is reconciled with `TxPool` as authority
and surfaced as an invariant failure rather than silently re-executed.

### 5.5 Performance contract

Correctness integration is rejected if it lowers performance. The target is
designed to remove work as well as locks:

- no coordinator actor hop and no channel round trip per stage;
- one hash lookup and one short critical section per checkout/completion,
  replacing separate queue, active-set, waiting-room and RBF lock choreography;
- immutable payloads referenced by `Arc`; indexes and tickets contain IDs only;
- bounded checkout/completion batches amortize wakeups without delaying a
  partially filled batch;
- stage queues consume their physical head monotonically and use O(1) live
  counters; there is no payload-capacity wait queue or per-operation global
  blocked-set scan;
- dependency and conflict changes touch only affected reverse-index buckets;
- callbacks, relay, cache writes and miner notifications remain outside locks;
- the effect outbox is bounded and pre-reserved before mutation; its common
  publication path may be flat-combined by the committing task to avoid a
  mandatory scheduling hop, while a supervised drainer retains cancellation
  and shutdown safety;
- accepted pool data is never copied into the coordinator.

Every production slice records lock hold/wait time, queue depth, stale-slot
ratio, allocations, CPU, throughput and dependent-chain latency. Focused quick
A/B is the per-slice directional gate. Medium/full remain the final release
gate and are not run again until explicitly requested; the interrupted medium
record remains no verdict.

### 5.6 Migration and deletion sequence

1. **Replace the prototypes.** Build an isolated single-coordinator model,
   port all lifecycle/dependency/conflict tests, add state-machine/property
   tests and exhaustion/fault-injection tests, then delete the three split-state
   prototype modules.
2. **Read-only differential surface.** In test builds, compare coordinator
   queries against legacy queue/active/waiting/RBF queries for full hash, short
   ID, peer, location, dependency and residency results. No production shadow
   copy is allowed because it would distort performance and ownership.
3. **Raw pipeline cutover.** Move admission, pre-check, ordered resolve and
   parent/deadline waiting to the coordinator. Delete
   `PreCheckQueue`, `OrderedResolveQueue`, their `ActiveSet`/`FlightTracker`
   ownership and `ParentsMissing` storage after differential and quick gates.
4. **Resolved/conflict cutover.** Move verify ordering, active verification,
   accepted-input waiting and speculative conflict scheduling. Delete
   `VerifyQueue` ownership, `RbfCandidates`, `RaceLost`, both executable
   `WaitingRoom` instances and the resolved RAII budget permit. Historical
   conflict audit records remain a separate bounded, non-executable cache.
5. **Mutation cutover.** Route commit/RBF/reorg/clear/remove/persistence through
   the authoritative pool transaction and typed commit journal. Run all historical security and
   reorg/template regressions plus injected panic/exhaustion cases.
6. **Cleanup and acceptance.** Remove epoch/token mechanisms made redundant by
   coordinator revisions, delete obsolete compatibility paths, run quick during
   cleanup, and run medium/full only on explicit final-validation instruction.

A legacy component is deleted only in the same slice that makes every caller
unreachable and ports its security property. Keeping two production owners as
a long-lived fallback is forbidden: rollback is performed by reverting the
whole slice at its checkpoint, not by dual-writing transactions.

### 5.7 Verification matrix and phase exits

The isolated coordinator is not eligible for production integration until the
following model suites pass with `audit()` after every transition:

| Area | Mandatory cases |
|---|---|
| Identity and payload | full-hash duplicate, proposal-short-id collision, witness variant, remote→proposal promotion, raw→unverified→verified replacement, and no resident snapshot |
| Typed state and leases | every legal edge, every illegal edge, stale checkout, remove/re-admit ABA, clear, revision/incarnation exhaustion, duplicate batch member and unwind at every apply boundary |
| Dependency graph | parent-first/child-first, multiple missing parents, cell deps, parent unavailable after dispatch/verification, exact-once final-parent wake, definitive cascade, parent-hash re-admission, fan-out cap and bounded maintenance slices |
| Conflict graph | preliminary under-fee rejection, unverified high-fee non-preemption, verified total ordering, multi-input all-or-none ownership, committing freeze, late waiter, success cohort, abort rebalance, stale fee proof and final in-lock RBF recalculation |
| Residency and fairness | exact-fit global/per-peer limits, aggregate metadata charge, resolved recharge, active-work cap, peer rotation, sybil/global cap, proposal reserve/remote eviction, expiry lifetime and terminal outbox charge transfer |
| Authoritative pool handoff | exact RBF/size-eviction journal, original status restoration, tip change, clear/remove/reorg races, raw-hash attached/detached identity and combined query during every handoff instruction boundary |
| Effects and shutdown | full outbox backpressure, publisher panic/cancellation/restart, callback re-entry, FIFO chain/reorg order, miner dirty journal, terminal exactly once and shutdown drain |
| Complexity | operation counters (not wall-clock alone) prove bounded lock work, monotonic stale-prefix consumption and no full-store scan on normal admission/checkout/completion |

Production slices additionally require differential tests at their public API
boundary and a focused quick A/B against the checkpoint. A slice with an
unexplained negative quick result, an unbounded complexity counter, a new
legacy/new dual owner, or any relevant **Open** ledger item is not allowed to
advance. Medium/full remain final acceptance only and require explicit
instruction.

---

## 6. Correctness Guarantees

- **Double-spend safety**: `submit_entry` runs inside the tx_pool write lock; concurrently verified double-spends cannot both commit.
- **Checkpoint lock ordering**: until coordinator cutover, the implemented order is `ordered_resolve_queue → rbf_candidates → verify_queue → waiting_room → tx_pool`, with `recovery_lock` outermost (before `tx_pool`) and the synchronous `pre_check_queue` mutex never held across `.await`.
- **Target lock ordering**: the existing pool read/write guard is the accepted-membership linearization lock. The only nested pair is `TxPool → coordinator`; coordinator-only transitions never acquire `TxPool`, and no external effect runs under either lock.
- **Single state authority**: the reviewed target has one coordinator entry and one revision for every pre-pool transaction. Dependency/conflict/deadline/queue structures are derived ID indexes, not parallel state machines.
- **Transaction no-silent-loss contract**: every admitted transaction leaving a normal pipeline worker reaches a defined terminal state — relayed, recorded, restored, parked, or explicitly routed to an internal terminal sink. Ordered chain deltas remain at the channel head until success or shutdown; a panic cannot acknowledge/drop one or allow a later delta to overtake it.
- **Speculative RBF is reversible**: displacement is hold-and-restore; only a committed replacement makes a rejection real, and a commit that replaced nothing aborts instead of finalizing. Speculative paths never write recent_reject, so an unverified candidate cannot censor an honest transaction.
- **No ghost state**: removal paths converge on full coverage (queues, registrations, both waiting rooms, conflict cache, pool); attached blocks finalize what they held; the reconcile's removals feed the same registration cleanup. Links/entries consistency is guarded: traversal filters ghost ids, ancestor links are committed only after all fallible checks, and eviction cascades purge removed ids from parent sets.
- **No panics in the write lock**: fallible paths inside the tx_pool write lock return defensive errors instead of asserting, so unwind can never strand the eviction journal or side-effect records.
- **Dependency correctness**: `FlightTracker` tracks in-flight outputs across all pipeline stages; the orphan flight heuristic counts queued, active, and parked parents as in flight.
- **Callbacks outside internal mutation locks**: submit callbacks are collected before dispatch; reorg callbacks are RAII-deferred until both the tx-pool write lock and `recovery_lock` are released. Callback panics are contained at the stable-state side-effect boundary.
- **Worker reliability**: per-job panic guards, monitor respawn with cancel-aware backoff, retained/FIFO chain transitions, per-generation cancellation for the verify manager, and clean stop on command-channel drop. Every dispatcher exit shares the same cancel → handler drain → worker quiesce/join → persist sequence.

---

## 7. Configuration

| Field | Meaning |
|-------|---------|
| `max_tx_verify_workers` | Verify workers (clamped to at least 1); pre-check workers are `min(this, available_parallelism())` |
| `max_tx_verify_cycles` | Large-cycle threshold for verify queue priority |
| `max_ancestors_count` | Ancestor chain limit |
| `verify_ordering` | Verify queue ordering: `arrival_time` (default) or `fee_rate` |
| `max_verify_queue_tx_size` | Verify queue budget; effective budget is `max(this, max_tx_pool_size)` |
| Resolved lifecycle budget | Same byte budget plus a 10,000-entry cap across verify queue, active workers, and RaceLost holds |
| Semaphore permits | `max_tx_verify_workers * 2` (actor loop concurrency cap) |

---

## 8. Running the Benchmark

```bash
# smoke
python3 devtools/tx_pool_bench.py --quick

# fast focused, interleaved A/B diagnostic (three adjacent pairs)
python3 devtools/tx_pool_bench.py --quick --runs 3 \
  --filter always_success_100 \
  --baseline-worktree /path/to/checkpoint-worktree \
  --save-baseline-json /tmp/tx-pool-quick-baseline.json \
  --save-json /tmp/tx-pool-quick-candidate.json \
  --fail-on-regression

# strict medium candidate gate; omit --filter for the complete matrix
python3 devtools/tx_pool_bench.py --runs 3 \
  --baseline-worktree /path/to/checkpoint-worktree \
  --save-baseline-json /tmp/tx-pool-baseline.json \
  --save-json /tmp/tx-pool-candidate.json \
  --fail-on-regression
```

Matrix selection: `QUICK_BENCH=1` for fast validation, `FULL_BENCH=1` for
comprehensive coverage, default is MEDIUM. Prefer `--baseline-worktree` for
performance decisions because it interleaves both checkouts; `--compare-json`
remains useful for non-gating historical inspection.

---

## 9. Test Coverage

Unit tests (`cargo test -p ckb-tx-pool --features internal`, currently 239/239) cover, among others:

- Queue invariants: pop/active/finish visibility, delayed retries with bounded attempts, FIFO waiting-room eviction, O(1) lookups.
- Pool invariants: child-before-parent weight folding, escape-hatch eviction without ghost parents, conflict closure ghost filtering, zombie reconciliation (inputs and cell deps), expiry cascade.
- RBF: hold-and-restore (displace, supersede-at-submit, abort/finalize, no-replacement abort), capacity pre-validation, recovery of the full removed cascade, speculative gates bypassing recent_reject.
- Reorg: retained-transaction outcome matrix (no cascade on `Duplicated`/`Superseded`), attached orphans skipped from routing, attached winners finalizing held candidates, `save_pool` blocking on `recovery_lock`, and fault-injected status-transition replay without false callbacks.
- Lifecycle: worker stop on command-channel drop, panic guards with terminal outcomes, callback-panic containment, retained/FIFO reorg deltas, VerifyMgr generation cancellation, deferred-worker drain on shutdown, controller-channel-close persistence only after all handlers/workers quiesce, clear/reset-to-miner-notify delivery, clear-vs-active commit cancellation, stale deferred recovery rejection, epoch exhaustion, and same-epoch RBF active-lease ABA.
- Terminal semantics: pre-check Full notified to the relayer, bounded recovery ending in a terminal reject, banned peers' in-flight jobs dropped, administrative removal tri-state and double-park cleanup.

Integration tests (`make integration`, full ckb-test suite) cover the end-to-end behavior, including the RBF family (`RbfBasic`, `RbfRejectReplaceProposed`, `RbfReplaceProposedSuccess`, `RbfConcurrency`, `RbfCyclingAttack`, `RbfOrphanRecovery`), conflict/removal flows (`RemoveConflictFromPending`, `RemoveTx`), and orphan handling.
