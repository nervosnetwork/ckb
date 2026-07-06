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
```

The script **streams each benchmark's progress in real time** (instead of waiting until the whole mode finishes), and finally prints a summary table.
`--quick` sets `QUICK_BENCH=1`, `--full` sets `FULL_BENCH=1`, and the default uses the medium matrix.

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
| `QUICK_SIZES` | `[50]` |
| `QUICK_PEER_COUNTS` | `[1]` |
| `QUICK_WORKER_COUNTS` | `[8]` |
| `QUICK_WARM_POOL_SIZE` | `50` |
| `QUICK_DEPENDENT_SIZES` | `[10]` |
| `QUICK_DEPENDENT_WARM_POOL_SIZE` | `10` |

Regular workloads and dependent-chain workloads use different size/warm configurations so the chain never grows too long.  Dependent chains are only benchmarked with **1 peer and the first worker count** because they are bottlenecked by serialized orphan recovery; varying peers/workers adds no useful signal.

## Workloads

- `always_success`: independent transactions using the always-success lock and genesis issue outputs.
- `secp256k1`: independent transactions using the secp256k1_blake160_sighash_all lock.
- `dependent_always_success`: a parent -> child dependent chain using the always-success lock.
- `dependent_secp`: a parent -> child dependent chain using the secp lock.

Each workload is tested in two variants:

- **cold pool**: submit the target transactions into an empty pool.
- **warm pool**: pre-fill `WARM_POOL_SIZE` transactions, then submit the target transactions.

## Key implementation details

### Cycles measurement

- All `always_success` transactions have the same cycle cost, so a single sample is measured and reused.
- `secp256k1` and dependent-chain transactions may have different cycle costs, so each transaction is measured individually via `test_accept_tx` / `process_tx` and stored as `HashMap<tx_hash, cycle>` to avoid order mismatches with a plain `Vec`.
- `max_ancestors_count` is set to `1000` in the benchmark config so dependent chains do not hit the ancestor limit.

### Dependent-chain submission strategy

Dependent chains must be submitted in **child -> parent** reverse order. Children land in the orphan pool first and are recovered cascade-style after their parents are accepted.

- **warm benchmark**: the warm prefix is already in the pool, so reversing the target segment is enough for recovery.
- **cold benchmark**: the target segment depends on the warm prefix. To avoid a hang, the warm prefix is submitted in natural order during the setup phase (not measured), and the target segment is submitted in reverse order during the measured phase.

### Resource lifecycle

- `SharedBench` owns the genesis snapshot, network controller, and tokio runtime, and is reused across all benchmark iterations.
- `start_controller` builds a full tx-pool through the production `TxPoolServiceBuilder::start` path and returns a `ServiceHandle`.
- `ServiceHandle::drop` cancels the local `CancellationToken` so the tx-pool actor and background tasks stop cleanly after each iteration, and drops the `tx_relay_sender` clone so the background relay-drain thread exits.
- `start_service` builds a bare `TxPoolService` via `TxPoolServiceBuilder::build_bench_service` and manually spawns the pipeline workers (`pre_check`, `verify_mgr`, `ordered_resolver`) plus the deferred task worker.  It is used only for cycle measurement.
- Both `start_controller` and `start_service` spawn a background thread to drain the relayer channel, preventing the channel from filling up and blocking.
- The deferred task worker only receives clones of the two fields it needs (`ordered_resolve_queue` and `txs_verify_cache`) so it does not hold a `deferred_sender` and the channel can close on shutdown.
- `SharedBench` creates genesis issue outputs according to the workload's actual need (`issue_outputs = max_size + warm_pool_size`), avoiding over-allocation for dependent chains.

### Criterion sampling

The benchmark group uses `SamplingMode::Flat` to avoid the default warning about being unable to collect 10 samples within 5 seconds.

## Output format

Benchmark ID format:

```text
tx_pool_pipeline/{mode}_{peers}peer_{workers}worker_{warm_}{tx_type}_{size}
```

Example:

```text
tx_pool_pipeline/pipeline_1peer_8worker_warm_dependent_secp_20
```

`tx_pool_bench.py` parses the `time:` and `thrpt:` lines after each ID and produces the comparison table.
