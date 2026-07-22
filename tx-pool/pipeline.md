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

**DeferredTask worker** — bounded mpsc channel (`DEFERRED_CHANNEL_SIZE = 1024`) for opportunistic recovery re-enqueue (RecoverTxs, `.send().await`) and verify-cache updates (CacheUpdate, `try_send`). A failed RBF replacement is different: entries physically removed by the failed attempt are restored exactly inside the original tx-pool write transaction; they never enter the deferred worker. The worker merges back-to-back non-critical recovery batches, retries transient `Full` backpressure with a bounded window that ends in a terminal outcome, and drains the channel on shutdown.

**WaitingRoom** — unified parking structure with two instances split by lock domain: pipeline-side (`ParentsMissing` orphans and `RaceLost` RBF-held candidates) and pool-side (`InputsBlocked` conflict recovery, the retired conflicts LRU). Per-reason budgets cover orphans (100 entries / 20 MB) and conflicts (10k entries / 50 MB). `RaceLost` is not charged a second time by the room: its `ResolvedTx` carries a shared lifecycle permit that remains charged while queued, active, or held, closing the previous budget-refund bypass. FIFO eviction is per reason and expiry scans are watermark-gated. Orphans and RaceLost expire after 100 block intervals; expiry revokes a still-live speculative winner before restoring the loser, and a re-park retains the original expiry so a stalled winner cannot create an endless restore/reverify loop.

**WorkerRunner / ActiveSet** — all queue workers (verify, ordered resolver) share a runner skeleton: wait on command changes, queue notifications, or deadlines; pop one job at a time; process to completion. Popped jobs move into the queue's ActiveSet and stay visible (`contains_or_active`, `get_active_tx`) until `finish`, so "in flight" checks, RPC queries, and administrative removal never lose sight of mid-processing transactions. Every job is wrapped in a panic guard; monitors respawn crashed workers with cancel-aware exponential backoff. A dropped command channel means a clean stop, not a busy loop.

**Dispatcher shutdown** — explicit cancellation, closure of the controller message
channel, and defensive dispatcher exits converge on one ordered shutdown tail:
cancel the pipeline, drain every in-flight message handler, quiesce and join the
background workers, then persist the accepted pool. The builder's startup-only
controller clone is dropped before `start` returns, so dropping the last
user-facing controller really closes the channel. Persistence never races a
still-running verifier or recovery worker, and completion of the dispatcher
handle is the durable shutdown boundary.

**Reorg** — `update_tx_pool_for_reorg` runs under `recovery_lock` for its whole duration (write-lock section, callbacks, and retained-transaction recovery), so `save_pool` cannot intentionally run in the middle of recovery. Detached transactions are re-added per-transaction through `process_tx_direct_outcome` in topological order: `Committed` and `Duplicated` count as success, `Superseded` skips cascading (the transaction is merely held), and only a definitive failure cascade-removes dependents. Transactions committed in attached blocks leave every pipeline structure through the full terminal sequence. Callbacks are outside the tx-pool write lock, but the current implementation still holds `recovery_lock` while dispatching and directly recovering transactions.

The controller applies backpressure instead of dropping a reorg delta when the
bounded channel is full. The handler retains the received head delta across
panics, retries it with cancel-aware exponential backoff, and never receives a
later delta first. Authoritative reorg operations converge when repeated;
registered callbacks are panic-contained at the side-effect boundary so they
cannot trap the retry loop. The target sequencer still replaces the coarse
`recovery_lock` with explicit prepare/commit/publish stages and must not run
external callbacks while holding recovery state.

**Block assembler** — the template lives behind a version counter; partial updates (`update_proposals`, `update_transactions`, `update_uncles`) swap via CAS while `update_full` and `reset_template` serialize under `template_lock` (with `update_uncles` joining that lock so a full update cannot revert a concurrent uncle update). Template byte accounting uses the consensus `serialized_size_without_uncle_proposals` basis throughout; uncle candidates that do not fit are truncated to the longest fitting prefix instead of dropped wholesale; embedded or stale candidates are removed eagerly. Pending proposals take priority over optional uncles: an uncle carrying a selected proposal id (and any descendant that loses its only valid parent) is filtered atomically from that template, so miners may omit optional uncles without stranding the transaction. Management-triggered resets (e.g. `clear_pool`) notify miners immediately, like the reorg path.

**remove_tx (administrative)** — removes a transaction from every structure it may occupy (pre-check, ordered, verify queue plus its registration, both waiting rooms, the conflict cache, and the pool) and reports a tri-state outcome: `Removed`, `InProgress` (a worker is mid-flight on it — reported honestly instead of "not found"), or `NotFound`.

---

## 3. Optimizations

### 3.1 Service Actor Semaphore

The actor loop spawns a task per message, capped at `max_tx_verify_workers * MESSAGE_CONCURRENCY_MULTIPLIER` concurrent tasks via `Arc<Semaphore>`.

### 3.2 Shared ChunkCommand Channel

One shared `watch::Sender` for the chunk pause/resume signal; every worker clones the receiver. No layered forwarding.

### 3.3 DeferredTask Backpressure

Opportunistic conflict recovery and cache updates go through a bounded mpsc channel with a single sequential worker: `RecoverTxs` uses `.send().await`, while `CacheUpdate` uses `try_send` because a cache miss is acceptable. Back-to-back recovery batches merge and shutdown drains the channel. Failed-submit rollback is deliberately not deferred; it completes before RBF ownership is released.

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

The whole reorg (write-lock section + retained-transaction re-add) holds `recovery_lock`; `save_pool` and `clear_pool` take the same lock, so normal completion is observed atomically by persistence and administration. Retained transactions re-enter through `process_tx_direct_outcome` with per-outcome handling (`Committed` / `Superseded` / `Duplicated` / failure), avoiding both write-lock stalls and false failure cascades. The ordered handler retains the head delta until success or shutdown and retries with backoff; callback panics are contained outside the authoritative mutation. This lock is still a legacy safety barrier rather than the target design because callbacks and direct recovery run while it is held.

### 3.12 Unified Verify-and-Submit Core

`verify_and_submit_core` is the single verify→submit path for pipeline workers, RPC, tests, and reorg recovery. Verified cycles are written to the verify cache on *every* terminal outcome (including superseded candidates), so restores never pay for a full re-verification.

### 3.13 Active-set Visibility

Popped-but-unfinished jobs stay visible in every queue's ActiveSet, closing the pop→finish window for duplicate checks, orphan flight heuristics, RPC queries, and administrative removal (which reports them as `InProgress`).

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
pipeline latency. Its diagnostic defaults are a 7% cross-run spread ceiling and
a 2% directional regression threshold; medium/full keep the 5%/0% release gate.
The runner can filter benchmark IDs for a fast focused retest without weakening
the recorded harness/environment checks.

Current measurement status: isolated one-run quick diagnostics are not release
evidence. An adjacent same-binary quick rerun changed the always-success median
by roughly 11%, exceeding the 5% record limit, so that attempted record was
discarded. No performance conclusion is drawn from its provisional deltas. A
clean, isolated, repeated medium/full checkpoint A/B is still required before
the coordinator enters the production hot path.

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

The next implementation keeps fixed parallel pre-check/verify worker pools and introduces a batched coordinator for lifecycle state plus a batched authoritative commit sequencer. Payload ownership becomes single-source; dependency and conflict schedulers store IDs only. The current `RbfCandidates`/`RaceLost` behavior is replaced only after the new conflict scheduler preserves concurrent fee preference, bounded residency, rollback ordering, and all ledger regressions without reducing throughput. A lock-free queue is not a goal by itself; it is considered only when measurement identifies the queue lock as a real bottleneck.

### 5.1 LifecycleStore migration slice

The first target component now exists as an isolated executable model in
`component/lifecycle_store.rs`; it is not connected to the production hot path
yet. It establishes these integration contracts before queue replacement:

- the full transaction hash is the authoritative identity, while proposal
  short IDs are a collision-checked secondary index;
- one immutable payload record moves through queued, active, waiting, and
  committing locations; worker leases carry incarnation/revision tokens, so a
  stale completion cannot mutate a removed and re-admitted transaction;
- global and per-peer count/byte charges remain live across every location and
  payload recharge is transactional;
- multi-transaction transitions validate completely before mutation, including
  an RBF handoff batch that terminalizes the committed winner and its
  speculative pre-pool victims;
- `Committed` is an ownership handoff: `LifecycleStore` then contains no live
  record for the transaction, and the existing `TxPool` is the sole authority
  for accepted `Pending`/`Gap`/`Proposed` state. There is no shadow pool state,
  duplicated payload, or hot-path status double-write;
- callbacks consume terminalized records only after the store mutation, which
  creates an explicit stable-state side-effect boundary.

The model is intentionally compiled out of the runtime call graph until the
ID-only dependency/conflict schedulers and differential tests are ready. Its
quick benchmark therefore must be runtime-neutral; each later integration
slice is gated against the checkpoint medium/full records.

### 5.2 DependencyScheduler migration slice

The isolated `component/dependency_scheduler.rs` model owns dependency IDs and
edges only. Readiness is represented by generation-tagged tickets: parent
availability wakes a child once, parent invalidation makes even a dispatched
ticket stale, and definitive failure cascades through ready and
capacity-blocked descendants. A downstream `Full` outcome becomes an explicit
`CapacityBlocked(stage)` state that is requeued by a capacity event; it cannot
silently fall back to orphan expiry. Entry, total-edge, and per-entry-edge
limits bound scheduler metadata independently from payload residency.

Production integration remains pending: queue capacity notifications must be
routed through the coordinator and differential tests must prove that current
parent-first/child-first behavior is preserved before the legacy orphan retry
logic can be removed.

### 5.3 ConflictScheduler migration slice

The isolated `component/conflict_scheduler.rs` model separates fee eligibility
from scheduling. `ReplacementFeeGate` must produce an eligible candidate before
the scheduler can hold or order it; both the candidate-specific pool
replacement fee and the size-based fee-rate floor are checked first. The
scheduler then stores IDs, conflict outpoints, score metadata and generation
tickets only. A multi-input candidate either wins every active conflict domain
or waits without partially displacing any owner. Committing candidates are
frozen against later arrivals; abort rebalances the highest-fee valid waiter,
while only authoritative commit success terminalizes direct conflicts.

Candidate count, total conflict edges and per-candidate edges are separately
bounded. Production integration must retain the current pool write-lock fee
calculation as the proof source. A successful pool mutation transfers the
winner out of `LifecycleStore`; it does not mirror the winner as a pool
location. Direct speculative losers are terminalized in the same coordinator
finalization before replacing `RbfCandidates` and `RaceLost`.

### 5.4 Authoritative commit transaction

Production integration uses one explicit prepare/apply/finalize/publish
protocol. It keeps the existing parallel verification workers and the existing
serialized `TxPool` write section; it does not add an actor hop or a second pool
representation to the hot path.

1. **Prepare (coordinator metadata only).** Validate the lifecycle lease and
   conflict-generation ticket and build an immutable commit intent. This phase
   does not mutate `TxPool`.
2. **Apply (coordinator → `TxPool` lock order).** Perform every fallible pool
   validation before destructive mutation wherever possible. The remaining
   mutation phase must either be infallible or carry an exact journal containing
   each removed `TxEntry` and its original `Status`. On failure, that journal is
   restored while the same `TxPool` write guard is still held; competing
   candidates can never observe temporarily free inputs.
3. **Finalize pre-pool ownership.** After successful pool apply, atomically
   terminalize the lifecycle winner as `Committed` and its direct speculative
   losers as `Rejected`. On failure, abort/requeue/terminalize from an explicit
   disposition. No callback, relay, miner notification, or asynchronous
   recovery runs in this phase.
4. **Publish effects.** After all internal locks are released, dispatch a typed
   effect journal: callbacks, relay, cache update, miner notification, and
   opportunistic dependency wakes. Effects are consequences of an already
   stable state and cannot decide or repair ownership.

The coordinator lock protects only lifecycle IDs, generations, bounded
accounting, and scheduler metadata. Queue leases and transitions are batched;
payload verification remains on the fixed worker pools. Consequently the
design removes legacy cross-queue lock choreography without serializing script
verification or copying accepted pool state. Medium/full A/B gates must confirm
that coordinator and journal costs do not reduce throughput before the new path
becomes the default.

---

## 6. Correctness Guarantees

- **Double-spend safety**: `submit_entry` runs inside the tx_pool write lock; concurrently verified double-spends cannot both commit.
- **Lock ordering**: `ordered_resolve_queue → rbf_candidates → verify_queue → waiting_room → tx_pool`, with `recovery_lock` outermost (before `tx_pool`) and the synchronous `pre_check_queue` mutex never held across `.await`.
- **Transaction no-silent-loss contract**: every admitted transaction leaving a normal pipeline worker reaches a defined terminal state — relayed, recorded, restored, parked, or explicitly routed to an internal terminal sink. Ordered chain deltas remain at the channel head until success or shutdown; a panic cannot acknowledge/drop one or allow a later delta to overtake it.
- **Speculative RBF is reversible**: displacement is hold-and-restore; only a committed replacement makes a rejection real, and a commit that replaced nothing aborts instead of finalizing. Speculative paths never write recent_reject, so an unverified candidate cannot censor an honest transaction.
- **No ghost state**: removal paths converge on full coverage (queues, registrations, both waiting rooms, conflict cache, pool); attached blocks finalize what they held; the reconcile's removals feed the same registration cleanup. Links/entries consistency is guarded: traversal filters ghost ids, ancestor links are committed only after all fallible checks, and eviction cascades purge removed ids from parent sets.
- **No panics in the write lock**: fallible paths inside the tx_pool write lock return defensive errors instead of asserting, so unwind can never strand the eviction journal or side-effect records.
- **Dependency correctness**: `FlightTracker` tracks in-flight outputs across all pipeline stages; the orphan flight heuristic counts queued, active, and parked parents as in flight.
- **Callbacks outside the tx-pool write lock**: submit and reorg callbacks are collected before dispatch, and callback panics are contained at that side-effect boundary. The target architecture strengthens this to "outside every internal transition/recovery lock" and adds callback-reentry tests.
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

# repeatable baseline
python3 devtools/tx_pool_bench.py --runs 3 --save-json /tmp/tx-pool-baseline.json

# strict candidate gate
python3 devtools/tx_pool_bench.py --runs 3 \
  --compare-json /tmp/tx-pool-baseline.json \
  --save-json /tmp/tx-pool-candidate.json \
  --fail-on-regression
```

Matrix selection: `QUICK_BENCH=1` for fast validation, `FULL_BENCH=1` for comprehensive coverage, default is MEDIUM.

---

## 9. Test Coverage

Unit tests (`cargo test -p ckb-tx-pool --features internal`) cover, among others:

- Queue invariants: pop/active/finish visibility, delayed retries with bounded attempts, FIFO waiting-room eviction, O(1) lookups.
- Pool invariants: child-before-parent weight folding, escape-hatch eviction without ghost parents, conflict closure ghost filtering, zombie reconciliation (inputs and cell deps), expiry cascade.
- RBF: hold-and-restore (displace, supersede-at-submit, abort/finalize, no-replacement abort), capacity pre-validation, recovery of the full removed cascade, speculative gates bypassing recent_reject.
- Reorg: retained-transaction outcome matrix (no cascade on `Duplicated`/`Superseded`), attached orphans skipped from routing, attached winners finalizing held candidates, `save_pool` blocking on `recovery_lock`.
- Lifecycle: worker stop on command-channel drop, panic guards with terminal outcomes, callback-panic containment, retained/FIFO reorg deltas, VerifyMgr generation cancellation, deferred-worker drain on shutdown, and controller-channel-close persistence only after all handlers/workers quiesce.
- Terminal semantics: pre-check Full notified to the relayer, bounded recovery ending in a terminal reject, banned peers' in-flight jobs dropped, administrative removal tri-state and double-park cleanup.

Integration tests (`make integration`, full ckb-test suite) cover the end-to-end behavior, including the RBF family (`RbfBasic`, `RbfRejectReplaceProposed`, `RbfReplaceProposedSuccess`, `RbfConcurrency`, `RbfCyclingAttack`, `RbfOrphanRecovery`), conflict/removal flows (`RemoveConflictFromPending`, `RemoveTx`), and orphan handling.
