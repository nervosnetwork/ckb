# CKB Tx-Pool Pipeline Architecture

> The tx-pool now runs exclusively in pipeline mode; the legacy serial processing path has been removed.

---

## 1. Motivation

The tx-pool accumulated many features over time — RBF, orphan recovery, chunk-based verification, fee estimation, pipeline pre-resolve — each bolted onto the original serial processing model. The result was code that was:

- **Hard to reason about**: resolve, verify, and submit stages were interleaved on a single async path with ad-hoc `tokio::spawn` for concurrency. Bug fixes in one feature often had subtle interactions with others.
- **Hard to parallelize**: CPU-intensive script verification (secp256k1) ran on the same tokio worker as I/O-bound snapshot reads and lock acquisitions, under-utilizing multi-core CPUs.
- **Hard to extend**: adding new ordering strategies (e.g., fee-rate prioritization) or new entry points (e.g., package submission) required touching deeply coupled code paths.

The pipeline architecture addresses all three by separating the transaction lifecycle into clearly bounded stages, each with its own concurrency model:

```
classify → pre_check → resolve → verify → submit
  (sync)   (parallel)  (serial)  (parallel) (serial, write lock)
```

This separation enables:

1. **IO/compute isolation**: snapshot reads and script verification run on the tokio blocking pool, freeing async workers for message dispatch.
2. **Parallel verification**: multiple verify workers consume a shared priority queue concurrently.
3. **Configurable ordering**: the verify queue supports fee-rate or arrival-time ordering via config.
4. **Cleaner concurrency**: bounded channels with backpressure replace fire-and-forget spawns.
5. **Unified entry**: a single classifier routes all transactions (remote, local, reorg re-adds) through the same pipeline.

---

## 2. Architecture

```
                         ┌──────────────────────────┐
  submit entry (remote/  │  classify_and_enqueue_tx  │ ← unified classifier
  local), reorg re-add   │          _spawn           │
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
                │     VerifyQueue     │ ← resolved txs      │
                │ (fee-rate or FIFO)  │   await verification│
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

### Stage Descriptions

**Classify (entry point)** — `classify_and_enqueue_tx_spawn` is the pipeline entry for remote/local submissions. It calls `check_and_route_dependent` to check the FlightTracker of ordered_resolve_queue, verify_queue, and pre_check_queue. Dependent transactions go directly to OrderedResolveQueue; independent ones enter PreCheckQueue for parallel processing. `classify_and_enqueue_tx` is used for reorg re-adds.

**PreCheckQueue** — FIFO queue bounded to 256 MB total serialized size. Worker count is `min(max_workers, available_parallelism())`. Workers pop jobs and execute pre_check (snapshot-only resolution for on-chain inputs, tx_pool read lock for pool-dependent inputs).

**OrderedResolveQueue / OrderedResolver** — transactions with missing inputs (parent still in-flight) wait here. Single-threaded resolver retries in arrival order. Maintains an `output_dependents` reverse index for push-based wake-up: when a parent enters the pool, its dependent children are extracted directly and re-enqueued, bypassing the FIFO scan. Bounded to 256 MB. Automatic respawn on panic.

**VerifyQueue** — holds resolved transactions awaiting script verification. Supports two ordering modes via `verify_ordering` config: `arrival_time` (FIFO, default) or `fee_rate` (highest fee rate first). Proposal transactions always take priority. Bounded to 256 MB.

**VerifyMgr** — multi-worker pool pulling from VerifyQueue. Worker 0 has the `OnlySmallCycleTx` role. All workers share a `watch::Receiver<ChunkCommand>` for chunk pause/resume. Script VM runs on the tokio blocking pool via `block_in_place`.

**submit_entry** — the consistency commit point. Conflict checking, RBF, pool_map writes, and limit_size eviction all run inside the tx_pool write lock. The serial end of the pipeline ensuring concurrent verification still produces a consistent pool state.

**DeferredTask** — bounded mpsc channel replacing fire-and-forget `tokio::spawn`. Handles RBF recovery tx re-enqueue (`.send().await`, must not drop) and verify cache updates (`try_send`, cache miss is acceptable). Single background worker with catch_unwind + respawn.

---

## 3. Optimizations

### 3.1 Service Actor Semaphore

The actor loop spawns a task per message. Unbounded spawning under high concurrency caused tokio scheduling pressure and write-lock contention. Fixed with `Arc<Semaphore>` capping concurrent tasks at `max_tx_verify_workers * 2`.

### 3.2 Shared ChunkCommand Channel

V1 had a 9-layer signal chain: `chain → TxPoolController → watch::Sender → VerifyMgr → send_child_command → per-worker watch::Sender → Worker → verify_rtx`. Replaced with a shared `watch::Sender`: `chunk_tx → Worker.command_rx` (each worker clones the receiver). VerifyMgr no longer forwards signals.

### 3.3 DeferredTask Backpressure

RBF recovery and cache updates moved from fire-and-forget `tokio::spawn` to a bounded mpsc channel with a single sequential worker. `RecoverTxs` uses `.send().await` (must not drop); `CacheUpdate` uses `try_send` (misses acceptable).

### 3.4 Lock-free Fast Path for On-chain Inputs

`pre_check` splits into two paths: when all inputs resolve from the chain snapshot, no tx_pool read lock is acquired. Only pool-dependent inputs take the read lock.

### 3.5 FlightTracker Double Index

Forward `HashMap<OutPoint, ProposalShortId>` for `depends_on` lookups. Reverse `HashMap<ProposalShortId, Vec<OutPoint>>` for O(outputs-per-tx) removal instead of O(total-entries).

### 3.6 RBF Replacement Failure Recovery

`submit_entry` runs `process_rbf` inside the write lock. If the replacement is later rejected (ancestor/size limits), the old transactions — already removed — are recovered from the conflicts cache and re-enqueued via DeferredTask. Reject callbacks dispatch after the lock is released to avoid re-entry deadlocks.

### 3.7 O(1) OrderedResolveQueue Lookups

Replaced `VecDeque<ResolveJob>` linear scans with `VecDeque<ProposalShortId>` + `HashMap<ProposalShortId, ResolveJob>` + lazy tombstone deletion. `get_tx` and `remove_tx` are now O(1).

### 3.8 Push-based Dependent Wake-up

Added `output_dependents: HashMap<OutPoint, HashSet<ProposalShortId>>` reverse index to OrderedResolveQueue. When a parent tx enters the pool, `drain_dependents` extracts its children directly and re-enqueues them at the FIFO back, bypassing the full queue scan. FIFO order is preserved by iterating the VecDeque (not the HashSet) when collecting matching ids.

### 3.9 Configurable Verify Queue Ordering

Added `verify_ordering` config (`arrival_time` | `fee_rate`). Fee-rate mode uses an `inverted_fee_rate` ordered index (`u64::MAX - fee_rate`) so that ascending BTreeSet iteration yields highest-fee-first ordering. Proposal txs always take absolute priority in both modes.

### 3.10 Lock-free RecentReject

`RecentReject` moved from inside TxPool (requiring write/read lock) to TxPoolService as `Option<Arc<RecentReject>>`. The RocksDB-backed structure uses read locks for concurrent put/get; write lock only during shard shrink. Reject callback signature drops the `&mut TxPool` parameter.

### 3.11 Reorg Pipeline Integration

Reorg re-adds go through the classify entry point (not direct verify under write lock). `_update_tx_pool_for_reorg` updates pool state under the write lock; detached transactions are re-enqueued through the pipeline after the lock is released.

### 3.12 Unified Verify-and-Submit Core

`verify_and_submit_core` extracts the shared pre_check → verify → submit → after_process logic. `handle_verify_success` unifies relayer notification and orphan cascade recovery.

---

## 4. Performance

Benchmark environment: Apple M-series (arm64), macOS 24.6.0, Rust 1.95.0.
Matrix: MEDIUM (100 tx/batch, peers [1, 4], workers [4, 8], 30 samples, Criterion 95% CI).

### 4.1 secp256k1 Transactions (CPU-intensive verification)

This is the primary production scenario. secp256k1 signature verification dominates processing time.

**Cold pool (empty pool, submit 100 txs):**

| Scenario | Pipeline (ms) |
|----------|---------------|
| 1 peer, 4 workers | 62.15 |
| 1 peer, 8 workers | 51.15 |
| 4 peers, 4 workers | 62.90 |
| 4 peers, 8 workers | 50.84 |

**Warm pool (30 txs already in pool):**

| Scenario | Pipeline (ms) |
|----------|---------------|
| 1 peer, 4 workers | 64.00 |
| 1 peer, 8 workers | 52.44 |
| 4 peers, 4 workers | 64.13 |
| 4 peers, 8 workers | 52.36 |

**Throughput (secp256k1, tx/s):**

| Scenario | Pipeline |
|----------|----------|
| 1 peer, 8 workers (cold) | 1,955 |
| 1 peer, 8 workers (warm) | 1,907 |
| 4 peers, 8 workers (cold) | 1,967 |
| 4 peers, 8 workers (warm) | 1,910 |

**Analysis:**

- **1-peer, 8 workers**: the blocking pool parallelizes CPU-heavy secp256k1 verification across cores.
- **8 workers outperform 4 workers** at 1-peer: more verify workers absorb more concurrent verification.
- **4-peer secp256k1 at 8 workers**: the multi-peer submission pattern creates more concurrent verify work, which the pipeline can absorb.
- **Warm vs cold**: negligible difference — pool state lookups are O(1) and do not affect pipeline staging.

### 4.2 Dependent Chain Transactions

10-deep dependency chain (child spends parent output), testing CPFP latency:

| Scenario | Pipeline (ms) |
|----------|---------------|
| 1 peer, 4 workers, secp256k1 | 20.69 |
| 1 peer, 4 workers, warm secp256k1 | 20.70 |

Push-based dependent wake-up ensures each child is re-enqueued immediately when its parent enters the pool, eliminating FIFO scan delay. Latency scales linearly with chain depth × single-tx resolve time.

---

## 5. Future: Lock-free Verify Queue

The current verify_queue uses `RwLock<multi_index_map>`. Multiple verify workers serialize on the write lock during `pop_front`. A lock-free replacement using `crossbeam-skiplist::SkipMap` was explored:

```rust
struct VerifyQueue {
    priority: SkipMap<VerifySortKey, ProposalShortId>,  // lock-free ordered pop
    by_id: DashMap<ProposalShortId, VerifyEntry>,       // lock-free id lookup
    flight: RwLock<FlightTracker>,                       // low-frequency reads
    total_tx_size: AtomicUsize,
}
```

`SkipMap` provides lock-free insert, pop_back (highest score), arbitrary remove, and contains_key. Multiple workers can pop concurrently via CAS-based `Entry::remove()`. The key challenge is epoch pinning discipline — Entry handles must be dropped promptly to allow GC of removed nodes.

This remains a design study; implementation is deferred.

---

## 6. Correctness Guarantees

- **Double-spend safety**: submit_entry runs inside the tx_pool write lock. Concurrently verified double-spend transactions cannot both commit.
- **Lock ordering**: `pre_check_queue → ordered_resolve_queue → verify_queue → orphan → tx_pool`.
- **Dependency correctness**: FlightTracker tracks in-flight outputs across all pipeline stages. `check_and_route_dependent` ensures dependent transactions enter OrderedResolveQueue.
- **Worker reliability**: VerifyMgr, OrderedResolver, and DeferredTask workers all catch panics and respawn automatically via CancellationToken.
- **RBF integrity**: if a replacement is rejected after process_rbf removed old transactions, the old transactions are recovered from the conflicts cache.

---

## 7. Configuration

| Field | Meaning |
|-------|---------|
| `max_tx_verify_workers` | Pre-check workers (capped at `available_parallelism`) and verify workers |
| `max_tx_verify_cycles` | Large-cycle threshold for verify queue priority |
| `max_ancestors_count` | Ancestor chain limit |
| `verify_ordering` | Verify queue ordering: `arrival_time` (default) or `fee_rate` |
| Semaphore permits | `max_tx_verify_workers * 2` (actor loop concurrency cap) |

---

## 8. Running the Benchmark

```bash
cargo bench -p ckb-tx-pool --features internal --bench pipeline -- --save-baseline pipeline
```

Matrix selection: `QUICK_BENCH=1` for fast validation, `FULL_BENCH=1` for comprehensive coverage, default is MEDIUM.

---

## 9. Test Coverage

Pipeline unit tests:
- Independent remote/local tx processing
- secp256k1 independent tx processing
- Dependent tx ordering preservation
- Conflicting double-spend rejection
- RBF rejected replacement recovery
- Verify queue fee-rate ordering (5 tests)

Integration tests:
- `RbfOrphanRecovery`: cascading RBF replacements with dependent recovery
- `RbfConcurrency`: concurrent RBF submission stability
- `ReorgRecoversDependentTxs`: reorg parent/child re-enqueue through pipeline
