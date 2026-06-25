# tx-pool Criterion Benchmark

This module measures the throughput of the CKB tx-pool in **pipeline** mode versus **sync** mode.

## Running

### Manually

```bash
# pipeline mode (default features include pipeline)
cargo bench -p ckb-tx-pool --features internal

# sync mode (disable the pipeline feature)
cargo bench -p ckb-tx-pool --no-default-features --features internal
```

### Using the comparison script

```bash
# full matrix (~1 hour)
python3 devtools/tx_pool_bench.py

# small quick matrix (~5 minutes)
python3 devtools/tx_pool_bench.py --quick
```

The script runs pipeline and sync modes back-to-back, **streams each benchmark's progress in real time** (instead of waiting until the whole mode finishes), and finally prints a comparison table.

## Matrices

### Full matrix

| Constant | Value |
|---|---|
| `SIZES` | `[50, 100]` |
| `PEER_COUNTS` | `[1, 2, 4]` |
| `WORKER_COUNTS` | `[4, 8, 12]` |
| `WARM_POOL_SIZE` | `100` |
| `DEPENDENT_SIZES` | `[10, 20]` |
| `DEPENDENT_WARM_POOL_SIZE` | `10` |

### Quick matrix (`--quick` / `QUICK_BENCH=1`)

| Constant | Value |
|---|---|
| `QUICK_SIZES` | `[50]` |
| `QUICK_PEER_COUNTS` | `[1]` |
| `QUICK_WORKER_COUNTS` | `[8]` |
| `QUICK_WARM_POOL_SIZE` | `50` |
| `QUICK_DEPENDENT_SIZES` | `[10]` |
| `QUICK_DEPENDENT_WARM_POOL_SIZE` | `10` |

Regular workloads and dependent-chain workloads use different size/warm configurations so the chain never grows too long.

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
- `secp256k1` and dependent-chain transactions may have different cycle costs, so each transaction is measured individually via `_test_accept_tx` / `process_tx` and stored as `HashMap<tx_hash, cycle>` to avoid order mismatches with a plain `Vec`.
- `max_ancestors_count` is set to `1000` in the benchmark config so dependent chains do not hit the ancestor limit.

### Dependent-chain submission strategy

Dependent chains must be submitted in **child -> parent** reverse order. Children land in the orphan pool first and are recovered cascade-style after their parents are accepted.

- **warm benchmark**: the warm prefix is already in the pool, so reversing the target segment is enough for recovery.
- **cold benchmark**: the target segment depends on the warm prefix. To avoid a hang, the warm prefix is submitted in natural order during the setup phase (not measured), and the target segment is submitted in reverse order during the measured phase.

### Resource lifecycle

- `SharedBench` owns the genesis snapshot, network controller, and tokio runtime, and is reused across all benchmark iterations.
- `ServiceHandle::drop` cancels the local `CancellationToken` so the tx-pool actor and background tasks stop cleanly after each iteration.
- Both `start_controller` and the temporary `start_service` used for cycle measurement spawn a background thread to drain the relayer channel, preventing the channel from filling up and blocking.
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
