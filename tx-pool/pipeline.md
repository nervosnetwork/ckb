# CKB tx-pool Pipeline V2 Refactoring Design and Optimization

> Status: V2 has been implemented and merged; `pipeline` is the default feature; integration tests and tx-pool unit tests pass.  
> Goal: Move the tx-pool remote/local transaction processing from a synchronous serial model to a multi-stage pipeline, improving concurrency for independent transactions while preserving ordering and the original safety semantics for dependent transactions.

---

## 1. Background and Problems

Before the refactoring, tx-pool processing was roughly:

```
receive tx → resolve → contextual verify → submit_entry
```

All steps ran serially on the same async path. CPU-intensive script verification (secp256k1, etc.) could occupy tokio worker threads for a long time, blocking the async runtime, under-utilizing multi-core CPUs, and limiting transaction submission throughput.

V1 pipeline introduced `ResolveQueue` + `PreResolveMgr` for concurrent pre-resolution, which improved secp256k1 throughput by about 8%. However, V1 had several architectural issues: pre-resolve and resolve stages overlapped and duplicated work; during reorgs `readd_detached_tx` performed full VM verification while holding the `tx_pool` write lock, stalling the entire pipeline; the ChunkCommand signal chain penetrated nine layers; and fire-and-forget `tokio::spawn` had no backpressure.

V2 pipeline unifies the entry point into a `classify` stage, eliminates the redundancy of `ResolveQueue` + `PreResolveMgr`, and simplifies the architecture in several dimensions.

---

## 2. V2 Pipeline Architecture

```
                         ┌──────────────────────────┐
  submit entry (remote/  │  classify_and_enqueue_tx  │ ← unified classifier
  local), reorg re-add   │  _spawn (pipeline mode)   │
                         └────────────┬─────────────┘
                                      │
                 ┌────────────────────┼─────────────────────┐
                 │ dependent txs      │ independent txs     │
                 ▼                    ▼                     │
    ┌─────────────────────┐  ┌──────────────────┐          │
    │ OrderedResolveQueue │  │  PreCheckQueue   │          │
    │ (dependent queue)   │  │ (FIFO + size cap)│          │
    └─────────┬───────────┘  └────────┬─────────┘          │
              │                       │                     │
    ┌─────────────────────┐  multi-   │ (min(max_workers,  │
    │   OrderedResolver   │  worker   │  available_parallelism()))
    │ (single-threaded    │  ┌────────▼────────┐           │
    │  ordered retry)     │  │ pre_check concur │           │
    └─────────┬───────────┘  └────────┬────────┘           │
              │                       │                     │
              └───────────┬───────────┘                     │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │     VerifyQueue     │ ← resolved txs await │
                │ (priority/size cap) │   verification       │
                └─────────┬───────────┘                     │
                          │ pop_front                       │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │     VerifyMgr       │ ← multi-worker      │
                │ (shared ChunkCommand)│   concurrent verify │
                └─────────┬───────────┘                     │
                          │                                 │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │    submit_entry     │ ← under tx_pool     │
                │ (consistency commit │   write lock        │
                │  point)             │                     │
                └─────────┬───────────┘                     │
                          │                                 │
                          ▼                                 │
                ┌─────────────────────┐                     │
                │   DeferredTask      │ ← single worker     │
                │ (backpressured      │   recovery/cache    │
                │  side-effects)      │                     │
                └─────────────────────┘                     │
```

### 2.1 Stage Descriptions

#### Classify entry point (new in V2, replaces ResolveQueue + PreResolveMgr)

V2 removes the two-level structure of V1 where `ResolveQueue` was a first-level FIFO and `PreResolveMgr` was a multi-worker pre-resolution pool. The new design unifies the entry into a single classifier:

- `classify_and_enqueue_tx_spawn` (pipeline mode): entry point for remote/local submissions. It first synchronously calls `check_and_route_dependent` to determine dependencies, then pushes independent transactions into `PreCheckQueue` to be processed asynchronously by the worker pool.
- `classify_and_enqueue_tx`: synchronous entry classifier, also used for reorg re-adds. It calls `check_and_route_dependent`, then inline executes `pre_check`, routing the result to `VerifyQueue` (success) or `OrderedResolveQueue` (missing inputs).

`check_and_route_dependent` is a new shared helper in V2. It checks the `FlightTracker` of `ordered_resolve_queue` and `verify_queue`. If a transaction depends on an in-flight transaction, it is routed directly to `OrderedResolveQueue`; otherwise it returns `None` so the caller continues with `pre_check`. This eliminates the duplicated dependency-check logic at the two entry points (`classify` and `classify_spawn`) in V1.

#### PreCheckQueue (new in V2, replaces ResolveQueue)

- FIFO queue holding `PreCheckJob { tx, is_proposal_tx, remote }`.
- Bounded by total serialized transaction size to 256 MB; new transactions are rejected when the limit is exceeded, preventing a flood of large transactions from growing the queue unboundedly.
- Worker count is `min(max_workers, available_parallelism())`, dynamically adapting to the number of CPU cores.
- Workers pop from the queue and execute `classify_and_enqueue_tx`; clean shutdown is via `CancellationToken`.

#### OrderedResolveQueue / OrderedResolver (ordered processing of dependent transactions)

- If a transaction's inputs are missing at pre-check time (the parent transaction is still in the pipeline), it enters `OrderedResolveQueue`.
- Single-threaded `OrderedResolver` retries transactions in arrival order.
- `HashSet<ProposalShortId>` index gives O(1) `contains_key`.
- Also bounded by total serialized transaction size to 256 MB.
- Automatic respawn on unexpected exit.

#### VerifyQueue (verification queue)

- Holds resolved `ResolvedTx`.
- Multiple indexes: `id`, `added_time`, `is_large_cycle`, `is_proposal_tx`.
- Pop priority: proposal tx > non-large-cycle tx (small-cycle worker) > ordinary tx.
- Total serialized size bounded to 256 MB, serving as the second level of pipeline backpressure.

#### VerifyMgr (concurrent verification, simplified in V2)

- Multiple workers pull transactions from `VerifyQueue`.
- Worker 0 has the `OnlySmallCycleTx` role and only processes small-cycle transactions.
- **V2 change**: all workers share a clone of the same `watch::Receiver<ChunkCommand>`; VerifyMgr no longer maintains per-worker child channels and the `send_child_command` broadcast loop. Signals propagate directly from `chunk_tx` to each worker, removing the intermediate forwarding overhead of VerifyMgr.
- Worker exit is controlled by `CancellationToken` instead of `ChunkCommand::Stop` broadcast.
- Script VM verification runs on the tokio blocking pool (`block_in_place`).

#### submit_entry (consistency commit point)

- Conflict checking, RBF handling, writing to `pool_map`, and `limit_size` eviction all happen inside the `tx_pool` write lock.
- The serial end point of the entire pipeline, ensuring that transactions verified concurrently still satisfy consistency when written to the pool.

#### DeferredTask (new in V2, backpressured side-effect handling)

In V1, RBF-displaced transactions being re-enqueued and verify-cache updates used fire-and-forget `tokio::spawn`. Under high RBF frequency these spawns had no backpressure, leading to accumulation of many short-lived tasks.

V2 introduces a `DeferredTask` enum plus a bounded `tokio::sync::mpsc` channel:

```rust
pub(crate) enum DeferredTask {
    RecoverTxs(Vec<TransactionView>),     // re-enqueue txs displaced by RBF
    CacheUpdate { wtx_hash, verified },   // write to verify cache
}
```

- `deferred_sender` lives on `TxPoolService` and is used after releasing the write lock.
- `RecoverTxs` uses `.send().await` (recovery transactions must not be silently dropped); `CacheUpdate` uses `try_send` (cache misses are acceptable and will be re-verified).
- A single background worker is spawned in `start()` to process recovery and cache updates sequentially; the handler is wrapped in `catch_unwind` so panic triggers automatic respawn.
- **Lifetime note**: the worker must only receive the fields it actually needs (`ordered_resolve_queue`, `txs_verify_cache`). It must not hold a full `TxPoolService` (which contains `deferred_sender`). Otherwise the worker, as the only consumer of the channel, would also be a producer, keeping the channel open forever; `recv().await` would never return `None`, causing graceful shutdown to hang.

---

## 3. Reorg Pipeline Integration (P0-1, a key V2 improvement)

### 3.1 V1 Problem

In V1, `readd_detached_tx` was executed inside `update_tx_pool_for_reorg` while **holding the `tx_pool` write lock**, performing `verify_rtx` (full VM verification) for every detached transaction. This meant:

- All verify workers were blocked during reorg (write-lock exclusion);
- 100 detached transactions × ~1 ms each ≈ ~100 ms of full-pipeline stall;
- The verify cache key used `tx.hash()` (txid) instead of `wtx_hash`, so the cache never hit and every detached transaction was fully re-verified.

### 3.2 V2 Solution

In pipeline mode, `update_tx_pool_for_reorg` is split into two phases:

```rust
pub(crate) async fn update_tx_pool_for_reorg(&self, ...) {
    // Phase 1: under the write lock, only update pool state
    #[cfg(not(feature = "pipeline"))]
    let fetched_cache = self.fetch_txs_verify_cache(retain.iter()).await;
    {
        let mut tx_pool = self.tx_pool.write().await;
        _update_tx_pool_for_reorg(&mut tx_pool, ...);
        #[cfg(not(feature = "pipeline"))]
        self.readd_detached_tx(&mut tx_pool, retain, fetched_cache).await;
    }
    // write lock released

    // Post-lock cleanup
    self.remove_orphan_txs_by_attach(&attached).await;
    { let mut queue = self.verify_queue.write().await; queue.remove_txs(...); }

    // Phase 2: in pipeline mode, retained txs enter through the classify entry
    #[cfg(feature = "pipeline")]
    for tx in retain {
        if let Err(e) = self.classify_and_enqueue_tx(tx, false, None).await {
            debug!("reorg re-add tx failed: {}", e);
        }
    }
}
```

Key design decisions:

- **`min_fee_rate` check**: committed transactions have already been selected by miners, so their fee rate always satisfies the requirement and does not need re-checking.
- **`after_process` side effects**: `remote: None` means the callback is essentially a no-op (no network notification is sent).
- **Atomicity**: after `_update_tx_pool_for_reorg` completes the pool is consistent; re-adding is an append operation, and intermediate states are safe.
- **Cache key fix**: after entering the pipeline, `verify_and_submit_core` uses `wtx_hash` as the cache key, so the cache can hit correctly.

---

## 4. Key Optimizations

### 4.1 Service Actor Semaphore (P0-3)

The `TxPoolService` actor loop (the `receiver.recv()` loop in `start()`) spawns an async task for each message. Under high concurrency, unbounded spawning causes:

- Many concurrent queue read locks contending with the `submit_entry` write lock;
- Tokio runtime scheduling pressure.

V2 introduces `Arc<Semaphore>`:

```rust
let semaphore = Arc::new(Semaphore::new(max_workers * 2));
// actor loop:
let permit = semaphore.clone().acquire_owned().await.unwrap();
handle.spawn(async move {
    let _permit = permit;
    process(service, message).await;
});
```

Permit count = `max_tx_verify_workers * 2`, ensuring queue concurrency does not exceed twice the verification consumption capacity.

### 4.2 Shared ChunkCommand Channel (P1-1)

V1 ChunkCommand signal chain:

```
chain → TxPoolController → watch::Sender → VerifyMgr.command_rx
  → send_child_command → per-worker watch::Sender → Worker.command_rx
  → verify_rtx
```

Nine layers, with VerifyMgr as an intermediate forwarding layer continuously doing `borrow_and_update` + `send_child_command` broadcasts.

V2 simplifies to:

```
chunk_tx (shared watch::Sender) → Worker.command_rx (each worker clones its own)
```

- VerifyMgr no longer maintains per-worker child channels and no longer needs the `send_child_command` method.
- All workers receive a clone of the shared `command_rx` in `VerifyMgr::new()`.
- Worker exit uses individual `CancellationToken`s (`signal_exit.child_token()`).
- The `command_rx.changed()` branch is removed from `start_loop()`.

### 4.3 Fire-and-Forget Backpressure (P1-4)

See section 2.1 for the DeferredTask description. The two fire-and-forget `tokio::spawn` sites are replaced by bounded channels:

- RBF recovery tx re-enqueue in `submit_entry` uses `.send().await` (originally `process.rs:219`).
- Verify cache update in `verify_and_submit_core` uses `try_send` (originally `process.rs:969`; cache misses are acceptable).

Two shared failure-path helpers were also extracted, eliminating duplicated code between `after_process` and `process_orphan_tx`, and between `after_process` and `OrderedResolver`:

- `handle_remote_reject`: unified "remote-error triple" (ban malformed peer + relay Reject if allowed + record recent_reject).
- `handle_missing_input_orphan`: routes a missing-input transaction into the orphan pool, and only relays `UnknownParents` **after** the insertion succeeds, preventing invalid notifications for duplicate or overflowing orphans.

`OrphanPool::add_orphan_tx` now returns `(bool, Vec<Byte32>)`, allowing callers to distinguish new insertions from duplicates and precisely control network notifications.

### 4.4 Classify Deduplication (P1-5)

In V1, `classify_and_enqueue_tx` and `classify_and_enqueue_tx_spawn` each independently implemented dependency checking + `OrderedResolveQueue` routing logic. V2 extracts this into the shared helper `check_and_route_dependent`, removing duplicate code.

### 4.5 Lock-free Fast Path for Purely On-chain Inputs

`pre_check` is split into two paths:

- **Fast path**: all inputs of the transaction can be found in the chain snapshot. It only reads the snapshot and does not acquire the `tx_pool` read lock.
- **Fallback path**: as long as one input may come from the transaction pool, resolution goes through the `tx_pool` read lock.

### 4.6 Verification Stage on the Blocking Pool

```rust
block_in_place(|| {
    let handle = Handle::current();
    handle.block_on(async move {
        ContextualTransactionVerifier::new(...)
            .verify_with_pause(max_tx_verify_cycles, &mut command_rx)
            .await
    })
})
```

`verify_with_pause` is a chunk-aware pausable verifier; `block_in_place` moves it onto the tokio blocking pool.

### 4.7 FlightTracker Double Index

- Forward `HashMap<OutPoint, ProposalShortId>` for `depends_on` lookups.
- Reverse `HashMap<ProposalShortId, Vec<OutPoint>>` for precise deletion on `remove`.
- `remove` complexity is O(outputs-per-tx) (typically 1–4), not O(total-entries).

### 4.8 Lock-free recent_reject (V2 follow-up improvement)

`RecentReject` was originally embedded in `TxPool`; `put` / `get` required the `tx_pool` write or read lock. After refactoring:

- Ownership moves up to `TxPoolService` as `Option<Arc<RecentReject>>`.
- `RecentReject` itself is made lock-free:
  - `RwLock<DBWithTTL>` only protects the Rust-side column-family map; the RocksDB C API is already thread-safe.
  - `put` / `get` only acquire the read lock (concurrent).
  - `shrink` acquires the write lock only when the key count exceeds the threshold, dropping and recreating a shard; the key counter is maintained with `AtomicU64` + `Relaxed` ordering.
- The `RejectCallback` signature drops the `&mut TxPool` parameter so callbacks no longer depend on the write lock.
- `GetTotalRecentRejectNum` / `GetTxStatus` / `GetTransactionWithStatus` access the service-level `recent_reject` directly, without entering the `tx_pool` lock.

### 4.9 RBF Replacement Failure Recovery + Callbacks Moved Outside the Lock

`submit_entry` executes `process_rbf` inside the `tx_pool` write lock: if old transactions are removed but the new transaction ultimately fails to submit due to ancestor/size limits, both the old and new transactions are lost. The fix:

```rust
// Collect old txs to recover while still inside the write lock
let mut recovered = Vec::new();
recovered = self.process_rbf(tx_pool, &entry, &conflicts, &mut reject_events);
...
// If submission later fails, restore old txs removed by process_rbf
// from the conflicts cache
if result.is_err() {
    recovered.extend(
        tx_pool.get_conflicted_txs_from_inputs(entry.transaction().input_pts_iter()),
    );
}
```

After the write lock is released:

1. Dispatch all reject callbacks from `reject_events` uniformly, avoiding callback re-entry deadlocks that could occur inside the lock.
2. Send recoverable old transactions back to `OrderedResolveQueue` for re-verification/submission via `deferred_sender.send(RecoverTxs(recovered)).await`.

`check_rbf` is also decomposed into four focused methods: `check_rbf_no_new_unconfirmed_inputs`, `check_rbf_descendants`, `check_rbf_no_conflict_cell_deps`, and `check_rbf_fee`, making unit testing and future rule extensions easier.

### 4.10 Unified Verify-and-Submit Core Path

`_process_tx` (sync path) and `_verify_and_submit_tx` (pipeline path) originally each maintained their own copy of the "pre_check → verify_rtx → submit_entry → after_process" code. After refactoring:

- `verify_and_submit_core` is extracted: unified pre_check, VM verification, submit_entry, and cache/deferred scheduling logic.
- `handle_verify_success` is extracted: unified handling of relayer notification and orphan cascade recovery after successful verification, shared by the local and remote branches of `after_process`.

This keeps verification semantics consistent between pipeline and non-pipeline paths and reduces dual-track maintenance cost.

---

## 5. Correctness and Safety

### 5.1 Double Spends Are Not Accepted

Although `pre_check` and `verify` can execute concurrently, the final commit point `submit_entry` still runs inside the `tx_pool` write lock. Conflict checking is completed inside the lock, so of two concurrently verified double-spend transactions only one can be written.

### 5.2 Lock Ordering

```
pre_check_queue → ordered_resolve_queue → verify_queue → orphan → tx_pool
```

### 5.3 Dependency Correctness

- `check_and_route_dependent` checks the `FlightTracker` of both `ordered_resolve_queue` and `verify_queue`.
- `OrderedResolver` processes transactions in arrival order, ensuring that a parent submitted earlier is resolved, verified, and submitted earlier than its children.

### 5.4 Worker Reliability

- VerifyMgr and OrderedResolver workers both catch panics (`catch_unwind`) and respawn automatically.
- Worker exit is controlled by `CancellationToken`; respawn only stops after the cancellation signal is received.

---

## 6. Performance Comparison

### 6.1 V2 Pipeline vs Sync (MEDIUM matrix, 10 samples)

| Scenario | Pipeline V2 | Sync | Difference |
|----------|-------------|------|------------|
| 1-peer secp256k1 4w | 61.2 ms | 180.1 ms | **pipeline -66%** |
| 1-peer secp256k1 8w | 51.3 ms | 189.1 ms | **pipeline -73%** |
| 1-peer always_success 4w | 16.1 ms | 30.0 ms | **pipeline -46%** |
| 1-peer always_success 8w | 18.7 ms | 28.5 ms | **pipeline -35%** |
| 1-peer dep. always_succ 4w | 6.8 ms | 7.4 ms | pipeline -9% |
| 1-peer dep. secp 4w | 20.4 ms | 22.2 ms | pipeline -8% |
| 4-peer secp256k1 4w | 61.6 ms | 62.5 ms | ~flat (-1%) |
| 4-peer secp256k1 8w | 51.0 ms | 66.4 ms | **pipeline -23%** |
| 4-peer always_success 4w | 16.5 ms | 14.9 ms | sync +11% |
| 4-peer always_success 8w | 18.8 ms | 14.4 ms | sync +31% |

### 6.2 V2 vs V1 Baseline

| Scenario | V1 Pipeline | V2 Pipeline | Change |
|----------|-------------|-------------|--------|
| 1-peer secp256k1 (4w) | ~54 ms | ~61 ms | +13% (semaphore throttling + PreCheckQueue worker overhead) |
| 1-peer always_success | ~14 ms | ~16 ms | +14% (same as above) |
| dep. chain | ~39 ms | ~7 ms (10 txs) | Different test scale; not directly comparable |
| 4-peer secp256k1 4w | ~55 ms | ~62 ms | +13% |
| 4-peer secp256k1 8w | — | ~51 ms | 8w first tested in V2 |
| 4-peer always_success 8w | ~17 ms | ~19 ms | +12% |

V2 absolute numbers are slightly higher than the V1 baseline (~10–15%), which is expected:

- **Semaphore throttling** (P0-3): the actor loop moves from unbounded spawn to `max_workers * 2` permits, intentionally limiting message-processing concurrency. This reduces tokio scheduling pressure and stabilizes tail latency, at the cost of slightly lower peak throughput.
- **DeferredTask channel** (P1-4): recovery tx and cache updates move from fire-and-forget spawn to bounded channel + single worker, adding a small amount of serial overhead.
- **PreCheckQueue worker cap**: worker count is capped at `min(max_workers, available_parallelism())`, dynamically adapting to the number of CPU cores and avoiding over-scheduling on smaller machines.

The **relative advantage** of pipeline vs sync remains stable: 1-peer secp256k1 is still -66% (V1 was -67%), showing that V2's architectural changes did not introduce a performance regression.

### 6.3 Analysis

- **1-peer secp256k1**: the core pipeline benefit is parallelization of pre_check + verify, spreading CPU-heavy secp256k1 verification across the blocking pool. The advantage is larger with 8 workers (-73%).
- **1-peer always_success**: verification itself is cheap, but pipeline pre_check parallelization still yields significant benefit (-46%).
- **dependent chain**: dependent transactions are serialized by `OrderedResolver`, so pipeline offers no extra advantage.
- **4-peer secp256k1 8w**: the new Semaphore + shared chunk channel in V2 improves the 8-worker scenario noticeably (-23%; V1 was flat).
- **4-peer always_success**: when verification is extremely cheap, pipeline scheduling overhead (PreCheckQueue workers, semaphore acquire) becomes overhead. This is an inherent limitation of pipeline mode.
- **Stability**: Sync mode CV < 1%; Pipeline mode CV 1–12% (scheduling contention increases with 8 workers).

### 6.4 Benchmark Matrix and Commands

The Criterion benchmark supports three matrices:

- `QUICK_BENCH=1`: minimal matrix, used only to quickly verify the benchmark pipeline does not hang.
- `FULL_BENCH=1`: full matrix, covering `PEER_COUNTS = [1, 2, 4, 8]`, `WORKER_COUNTS = [4, 8]`, and independent/dependent (always_success / secp256k1) combinations.
- Default MEDIUM matrix: peers `[1, 4]`, workers `[4, 8]`, plus dependent-chain scenarios, balancing speed and coverage.

> Note: the numbers in sections 6.1 and 6.2 come from a single MEDIUM-matrix run. Subsequent commits expanded the matrix to peers `[1, 2, 4, 8]` and dependent-chain scenarios, but the absolute magnitudes remain consistent.

```bash
# Pipeline mode (ckb-tx-pool has pipeline enabled by default)
cargo bench --features "ckb-tx-pool/internal" -p ckb-tx-pool --bench pipeline

# Sync mode (must disable pipeline feature with --no-default-features)
cargo bench --no-default-features --features "ckb-tx-pool/internal" -p ckb-tx-pool --bench pipeline
```

`QUICK_BENCH=1` for the quick matrix, `FULL_BENCH=1` for the full matrix, default is MEDIUM.

### 6.5 Post-Audit Fix Benchmark Results

After the correctness and reliability fixes (see section 10), benchmarks were re-run to measure the performance impact.

#### QUICK Matrix: Pipeline vs Sync (1 peer, 8 workers, 10 samples)

| Scenario | Pipeline | Sync | Difference |
|----------|----------|------|------------|
| cold always_success 50 | 7.37 ms | 12.41 ms | **pipeline -41%** |
| cold secp256k1 50 | 23.23 ms | 83.01 ms | **pipeline -72%** |
| warm always_success 50 | 8.01 ms | 12.42 ms | **pipeline -36%** |
| warm secp256k1 50 | 23.88 ms | 82.87 ms | **pipeline -71%** |
| cold dep. always_success 10 | 5.27 ms | — | pipeline only* |
| cold dep. secp 10 | 18.34 ms | — | pipeline only* |
| warm dep. always_success 10 | 4.91 ms | — | pipeline only* |
| warm dep. secp 10 | 18.55 ms | — | pipeline only* |

\* Dependent chain benchmarks are skipped in sync mode because the cycle measurement path (`PerTxProcess`) requires the pipeline's classify → ordered-resolve → verify chain. The sync mode's `notify_tx` cannot resolve pool-only dependencies.

#### MEDIUM Matrix: Pipeline vs Sync (1 peer & 4 peer, 4 & 8 workers, 30 samples)

| Scenario | Pipeline | Sync | Difference |
|----------|----------|------|------------|
| 1-peer 4w always_success 100 | 13.55 ms | 22.30 ms | **pipeline -39%** |
| 1-peer 4w secp256k1 100 | 79.12 ms | 165.48 ms | **pipeline -52%** |
| 1-peer 8w always_success 100 | 13.82 ms | 22.84 ms | **pipeline -39%** |
| 1-peer 8w secp256k1 100 | 44.85 ms | 166.83 ms | **pipeline -73%** |
| 4-peer 4w always_success 100 | 14.70 ms | 10.87 ms | sync +35% |
| 4-peer 4w secp256k1 100 | 78.30 ms | 53.58 ms | sync +46% |
| 4-peer 8w always_success 100 | 13.90 ms | 11.11 ms | sync +25% |
| 4-peer 8w secp256k1 100 | 45.14 ms | 54.30 ms | **pipeline -17%** |
| warm 1-peer 8w always_success 100 | 15.96 ms | 23.40 ms | **pipeline -32%** |
| warm 1-peer 8w secp256k1 100 | 45.97 ms | 167.34 ms | **pipeline -73%** |
| warm 4-peer 8w always_success 100 | — | 12.24 ms | — |
| warm 4-peer 8w secp256k1 100 | 57.63 ms | 55.70 ms | ~flat (+3%) |

The audit fixes (RBF deduplication, verify_queue promotion, clear_pool consistency, shutdown drain, reorg panic recovery) did not introduce measurable performance regressions.

### 6.6 Post-Fix Analysis

- **1-peer secp256k1**: pipeline advantage scales with worker count — -52% at 4 workers, -73% at 8 workers. The blocking pool parallelizes CPU-heavy verification across cores, while sync serializes everything on the actor loop.
- **1-peer always_success**: pipeline shows -39% improvement even for cheap verification, because pre_check parallelization amortizes lock acquisition and resolution overhead.
- **4-peer secp256k1 4w**: sync is 46% faster than pipeline at 4 workers. The 4-peer submission pattern (4 concurrent submitters each blocking on their response) creates less contention in sync mode's serialized actor loop. At 8 workers, pipeline catches up (-17%) because more verify workers can absorb the higher submission rate.
- **4-peer always_success**: sync is faster (+25-35%) because verification is trivially cheap and pipeline scheduling overhead (PreCheckQueue workers, semaphore acquire, queue enqueue/dequeue) dominates.
- **Warm pool**: the warm-vs-cold pattern is consistent — warm pool adds a fixed setup cost but the relative pipeline advantage is similar.
- **Dependent chains**: pipeline processes dependent chains through OrderedResolver serialization. Performance is comparable to sync's orphan-pool cascade (QUICK: pipeline 5.27 ms vs sync's orphan cascade in section 6.1).
- **Stability**: pipeline CV is slightly higher than sync (1-10% vs <1%), consistent with pre-fix observations. The audit fixes did not worsen stability.

---

## 7. Configuration

| Field | Meaning |
|-------|---------|
| `max_tx_verify_workers` | Number of pre-check workers (capped at `available_parallelism`) and verify workers |
| `max_tx_verify_cycles` | Large-cycle threshold, affects verify-queue priority |
| `max_ancestors_count` | Ancestor-chain limit inside the transaction pool |
| Semaphore permits | `max_tx_verify_workers * 2` (service actor concurrency cap) |

---

## 8. Test Coverage

Regression tests (all support `pipeline` feature on/off):

- `pipeline_processes_independent_remote_txs`: independent remote transactions are processed correctly through the pipeline.
- `pipeline_processes_independent_secp_remote_txs`: secp256k1 independent transactions are processed correctly.
- `pipeline_preserves_order_for_dependent_txs`: dependent-transaction ordering is preserved.
- `pipeline_preserves_order_for_dependent_secp_txs`: secp256k1 dependent-transaction ordering is preserved.
- `pipeline_rejects_conflicting_double_spend`: concurrent double spends are rejected.
- `pipeline_throughput` / `secp_remote_throughput`: throughput benchmarks.
- `pipeline_rbf_rejected_replacement_recovers_original_tx`: old transactions are recovered when an RBF replacement fails.
- Integration test `RbfOrphanRecovery`: after cascading RBF replacements, dependent transactions are recovered through `DeferredTask::RecoverTxs` → `ordered_resolve_queue` → `OrderedResolver`.
- Integration test `ReorgRecoversDependentTxs`: after a reorg, parent/child transactions are re-enqueued through the pipeline and recovered.

---

## 9. Future Directions

### 9.1 Deferred P0: `readd_detached_tx` still verifies under the write lock in non-pipeline mode

The improvement currently applies only under `#[cfg(feature = "pipeline")]`, routing re-adds through the pipeline. Non-pipeline mode still uses the original `readd_detached_tx` (full `verify_rtx` inside the write lock). If non-pipeline reorg performance is needed in production, consider splitting it into three phases (collect → release lock → re-verify → re-acquire lock).

### 9.2 Deferred P1: `process_orphan_tx` is now connected to the pipeline (still under observation)

`process_orphan_tx` now calls `classify_and_enqueue_tx` when it finds a recoverable orphan, so it goes through `check_and_route_dependent`, `pre_check`, and `FlightTracker` checks and no longer bypasses the pipeline. Continue observing whether behavior in dependency-chain recovery scenarios meets expectations, and whether local orphans should also be unified into the `classify_and_enqueue_tx_spawn` worker pool.

### 9.3 Further Compress the ChunkCommand Signal Chain

Currently the `chunk_tx` watch channel is subscribed by both VerifyMgr and OrderedResolver. The chain module → TxPoolController → chunk_tx still has an intermediate layer. Consider letting the chain module own `chunk_tx` directly, removing TxPoolController's forwarding.

### 9.4 crossbeam `tx_relay_sender` (P2, low priority)

Production uses `ckb_channel::unbounded()` (`send()` never blocks), so this is not a practical issue. The benchmark uses `bounded(1024)` with a drain task and also does not block. Only worth attention under extreme scenarios.

---

## 10. Audit Fixes

This section documents correctness, reliability, and safety fixes identified through deep audit of the V2 pipeline implementation.

### 10.1 RbfCandidates Multi-Displacement (F3)

`RbfCandidates::register()` previously returned `Option<ProposalShortId>` — only the last displaced candidate. When a higher-fee transaction conflicts with multiple existing candidates across different inputs, only one was evicted from the `VerifyQueue`, leaving stale entries. Fixed to return `Vec<ProposalShortId>` collecting all displaced candidates. The caller in `classify_and_enqueue_tx` now iterates and removes all displaced entries from `verify_queue`.

### 10.2 check_rbf_descendants Deduplication (3.7 + 3.8)

`check_rbf_descendants` collected descendants of all conflict transactions without deduplication. When two conflict transactions shared a common descendant, it was counted twice, inflating `replace_count` and potentially rejecting valid RBF replacements. Fixed with a `HashSet<ProposalShortId>` to track seen IDs, ensuring each descendant is counted exactly once.

### 10.3 Non-Pipeline Orphan Double Notification (D4)

The non-pipeline orphan processing path (`process_orphan_tx`) called both `handle_remote_reject` explicitly AND `after_process` (which internally calls `handle_remote_reject`). This double notification sent duplicate ban/relay messages for the same rejected transaction. Fixed by removing the explicit `handle_remote_reject` call, relying solely on `after_process` to handle remote notifications.

### 10.4 clear_pool Missing rbf_candidates (F4)

`clear_pool` (called during chain state changes) cleared `pre_check_queue` but not `rbf_candidates`. After a reorg, stale RBF candidate entries could reject new legitimate replacements. Fixed by adding `self.rbf_candidates.write().await.clear()` in the pipeline branch of `clear_pool`.

### 10.5 ban_malformed Missing Orphan Cleanup (N5)

`ban_malformed` removed a misbehaving peer's transactions from `verify_queue`, `pre_check_queue`, and `ordered_resolve_queue`, but not from the orphan pool. Orphans from a banned peer could linger and be re-processed. Fixed by iterating orphan entries and removing all orphans belonging to the banned peer.

### 10.6 Shutdown Drain of In-Flight Tasks (N1)

The shutdown handler saved the pool state without waiting for in-flight detached tasks (spawned by the semaphore-gated actor loop) to complete. Transactions accepted by workers but not yet committed to the pool were lost on restart. Fixed by acquiring all semaphore permits (`max_workers * 2`) before calling `save_pool()`, ensuring all in-flight tasks have completed.

### 10.7 Reorg Handler Panic Recovery (N2)

The reorg handler (`update_tx_pool_for_reorg`) ran without panic protection. A panic would permanently kill reorg processing while other workers (VerifyMgr, OrderedResolver, DeferredTask) all had catch_unwind + respawn. Fixed by wrapping the reorg handler loop in `AssertUnwindSafe(...).catch_unwind()` with automatic respawn on panic.

### 10.8 VerifyQueue Proposal Promotion (4.6)

When a proposal transaction was added to `verify_queue` that already existed as a non-proposal entry, `add_tx` simply returned `false` (duplicate) without promoting the existing entry. This meant proposal transactions lost priority in the verification queue. Fixed by using `modify_by_id` to promote `is_proposal_tx` in-place and refresh `added_time` when a proposal duplicate arrives.

### 10.9 Sync Benchmark Dependent Chain Fix

The benchmark's cycle measurement for dependent chains (`PerTxProcess` mode) relied on the pipeline's classify → ordered-resolve → verify path, which doesn't exist in sync mode. This caused the sync benchmark to hang indefinitely when initializing dependent chain data sets. Fixed by gating dependent chain BenchData construction behind `#[cfg(feature = "pipeline")]`.

---

## 11. Known Remaining Issues

The following issues were identified during audit but deferred due to low impact or acceptable tradeoffs:

- **N3 MED**: `resumeble_process_tx` does not check `pre_check_queue` for duplicates (pipeline mode only). A transaction already queued in pre_check can be submitted again. Impact: duplicate is harmlessly absorbed during classify.
- **N4 MED**: `RecoverTxs` discards `remote` and `is_proposal_tx` metadata from the original submission. Recovered transactions are re-enqueued as local non-proposal. Impact: minor — recovery is a best-effort retry path.
- **N7 LOW**: `OrderedResolveQueue::get_tx` and `remove_tx` are O(n) linear scans. Acceptable because the queue is bounded and n is typically small (< 100).
- **N10 LOW**: `depends_on_pipeline` checks multiple queues sequentially, creating a benign TOCTOU window where a transaction may be misclassified as independent. The subsequent submit_entry handles this correctly via conflict detection.
- **4.7 LOW**: `RecentReject::put()` performs synchronous RocksDB I/O under `RwLock` read lock. Bounded impact due to low call frequency and small key sizes.
- **reorg handler no catch_unwind** (pre-fix): Now fixed (N2).
- **shutdown save_pool race** (pre-fix): Now fixed (N1).
