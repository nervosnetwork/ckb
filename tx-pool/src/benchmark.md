# tx-pool Criterion Benchmark

This module measures the throughput of the CKB tx-pool pipeline.

## Running

### Manually

```bash
cargo bench -p ckb-tx-pool --features internal
```

### Using the comparison script

```bash
# default medium matrix (~10–15 minutes)
python3 devtools/tx_pool_bench.py

# small quick matrix (~5 minutes)
python3 devtools/tx_pool_bench.py --quick

# full matrix (~1 hour)
python3 devtools/tx_pool_bench.py --full

# save the median of three complete runs
python3 devtools/tx_pool_bench.py --runs 3 \
  --save-json /tmp/tx-pool-baseline.json

# compare and fail on any measured regression
python3 devtools/tx_pool_bench.py --runs 3 \
  --compare-json /tmp/tx-pool-baseline.json \
  --save-json /tmp/tx-pool-candidate.json \
  --fail-on-regression
```

The script **streams each benchmark's progress in real time** (instead of waiting until the whole mode finishes), aggregates repeated runs by median, records the commit/dirty state/toolchain/platform and raw run medians in JSON, and can enforce the architecture's strict non-regression gate. A failing gate requires the baseline and candidate to come from the same recorded host/toolchain and to use the same repetition count of at least three; a one-run smoke record is never accepted as one side of a release decision.
`--quick` sets `QUICK_BENCH=1`, `--full` sets `FULL_BENCH=1`, and the default uses the medium matrix.

Each checkout builds into its own `<workspace>/target/tx-pool-bench` directory;
an externally supplied shared `CARGO_TARGET_DIR` is deliberately ignored. This
prevents Cargo from reusing a baseline worktree's same-named executable for the
candidate. Strict comparisons also require a byte-identical SHA-256 fingerprint
of the Python runner and Rust benchmark harness.

Every report includes the max-min throughput spread across complete runs. With
`--fail-on-regression`, either side exceeding
`--max-run-spread-percent` (5% by default) is rejected as an invalid/noisy
measurement rather than mislabeled as a code regression. Quick mode is a smoke
and development diagnostic; medium/full repeated records are the architectural
acceptance evidence. Strict records must also come from clean tracked trees, so
the exact measured source can be reconstructed from `git_commit` (untracked
local notes do not invalidate a run).

## Matrices

The matrix is selected at compile time via environment variables:

- `FULL_BENCH=1` — full matrix.
- `QUICK_BENCH=1` — quick matrix.
- default (no env var) — medium matrix.

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

Regular workloads and dependent-chain workloads use different size/warm configurations so the chain never grows too long.  Dependent chains are only benchmarked with **1 peer and the first worker count** because they are bottlenecked by serialized orphan recovery; varying peers/workers adds no useful signal.

## Workloads

- `always_success`: independent transactions using the always-success lock and genesis issue outputs.
- `secp256k1`: independent transactions using the secp256k1_blake160_sighash_all lock.
- `dependent_always_success_parent_first`: a normal parent -> child chain using the always-success lock and the in-flight dependency path.
- `dependent_always_success_child_first`: the same chain submitted in reverse to exercise orphan recovery.
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
- **child-first**: children enter orphan parking and are recovered cascade-style after their parents are accepted.

- **warm benchmark**: the warm prefix is already in the pool; the target segment is submitted in the selected direction.
- **cold benchmark**: the target segment depends on the warm prefix. The warm prefix is submitted in natural order during setup (not measured), then the target segment is submitted parent-first or child-first during measurement.

### Resource lifecycle

- `SharedBench` owns the genesis snapshot, network controller, and tokio runtime, and is reused across all benchmark iterations.
- `start_controller` builds a full tx-pool through the production `TxPoolServiceBuilder::start` path and returns a `ServiceHandle`.
- Before returning a new controller to Criterion, setup completes one dispatcher round-trip and a short Tokio scheduling interval. This keeps freshly spawned worker startup latency outside the measured transaction batch without warming the verification cache or pool.
- `ServiceHandle::drop` cancels the local `CancellationToken`, awaits the main dispatcher (which quiesces all message handlers and production workers), and drops/awaits the relay drain. No cancelled worker, pool save, or blocking drain may overlap the next iteration. A teardown timeout or task panic fails the benchmark instead of silently admitting a contaminated sample.
- Criterion uses `iter_batched_ref`, so that complete service shutdown (worker quiescence, pool save and relay-drain join) happens after the measurement interval rather than being charged to transaction latency.
- `start_service` builds a bare `TxPoolService` via `TxPoolServiceBuilder::build_bench_service` and manually spawns the pipeline workers (`pre_check`, `verify_mgr`, `ordered_resolver`) plus the deferred task worker. It is used only for cycle measurement. Teardown joins those workers first, releases the final service/relay sender, and only then joins the relay drain; joining all three ownership layers at once would deadlock until timeout.
- Both `start_controller` and `start_service` spawn a background thread to drain the relayer channel, preventing the channel from filling up and blocking.
- The deferred task worker only receives clones of the two fields it needs (`ordered_resolve_queue` and `txs_verify_cache`) so it does not hold a `deferred_sender` and the channel can close on shutdown.
- `SharedBench` creates genesis issue outputs according to the workload's actual need (`issue_outputs = max_size + warm_pool_size`), avoiding over-allocation for dependent chains.

### Criterion sampling

Quick mode uses the narrow one-peer/one-worker-count matrix with 20 flat samples, a 2-second warm-up, and an 8-second measurement window. Its larger 100-transaction independent batches and 20-transaction dependent chains improve signal-to-noise without expanding the scenario matrix. Medium/full modes remain the release gates.

Measured completion is event-driven: the pending callback increments an atomic counter and wakes a `Notify` waiter only after the pool transition is stable. No 1 ms polling timer or `get_tx_pool_info` request runs in the measured path.

## Output format

Benchmark ID format:

```text
tx_pool_pipeline/{mode}_{peers}peer_{workers}worker_{warm_}{tx_type}_{size}
```

Example:

```text
tx_pool_pipeline/pipeline_1peer_8worker_warm_dependent_secp_child_first_20
```

`tx_pool_bench.py` parses the `time:` and `thrpt:` lines after each ID and produces the comparison table.
