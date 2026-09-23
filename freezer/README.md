# Freezer

Freezer stores verified historical blocks in append-only LZ4 files. After publishing an exact block-hash index, it reclaims hot payloads by rotating column families (CFs) online. Ordinary block transactions and key/value encodings remain unchanged.

## Enabling and upgrading

Set the following in the node's `ckb.toml`, then restart:

```toml
[store]
freezer_enable = true
```

A database that has never used Freezer can be enabled directly. Startup retains the original 19 CFs, adds the fixed archive-index CF `19`, and treats the original payload CFs as generation 0. No export, resynchronization, or offline payload copy is required. The archive uses the configured `ancient` directory, which defaults to `data_dir/ancient`.

**Enabling Freezer is permanent.** Opening the archive successfully persists a recovery cursor synchronously, even before the first block is archived. Subsequent starts must retain `freezer_enable = true`. Setting it to `false` or removing the setting rejects startup with `Freezer cannot be disabled once enabled`. Startup also recognizes archive files created before an interrupted cursor write. Previously enabled nodes continue background archival and collection.

The only supported archive format is `CKBFZ006`, which uses SHA-256 for metadata and payload checksums. Existing archives in this format are validated and recovered before processing resumes. Legacy Snappy archives and earlier experimental formats cannot be opened or migrated directly. The database upgrade described above applies to original KV databases that have never archived blocks; archive directories from different formats are not interchangeable.

## Safety interval and the first CF rotation

New archival retains the current epoch and the preceding **100 complete epochs** in hot storage. At current epoch `E`, only verified blocks with `epoch < E - 100` are eligible, with the cutoff at the end of a complete epoch:

| Current epoch | Eligible blocks |
|---|---|
| 0–100 | No new archival |
| 101 | Epoch 0, with the genesis block retained in hot storage |
| 150 | Epochs 0–49; epochs 50–150 retain hot payloads |

The interval is fixed and cannot be shortened through configuration. Initial catch-up follows the same boundary. Previously committed archives remain readable and are not truncated or moved back to hot storage when this rule changes.

After leaving initial block download (IBD), the node checks every 60 seconds. Each pass handles at most 30,000 block heights, committing batches of at most 512 records or approximately 16 MiB. Initial enablement does not copy the entire database during startup. It catches up in batches, then starts CF rotation automatically when newly indexed progress reaches `max(1024, ceil(estimated hot payload block count / 2))`. Subsequent generations use the same scheduling rule. Payloads remain in their original CFs until both archival age and collection thresholds are met.

Rotation copies retained payloads, reconciles concurrent changes, commits the new routing and collection cursor synchronously, refreshes the chain snapshot, and drops the previous five payload CFs: `2/3/7/13/15`. Their successors are named `freezer.<generation>.<logical-column>` and retain each column's options. The fixed archive-index CF `19` and metadata CF `4` do not rotate.

Existing snapshots, iterators, and pinned values retain the handles they need and remain readable after rotation. Old SSTs become eligible for deletion only after those views are released, so successful rotation does not imply immediate disk-space reclamation. Unknown keys, unverified blocks, hot overlay records, and blocks inside the safety interval remain hot.

The current indexer and rich-indexer still read the node database through RocksDB Secondary. Their fixed CF list does not follow new payload generations, and they have no reader for archived blocks. Do not enable Freezer alongside either indexer until that separate integration is completed.

The 50% trigger is a scheduling heuristic, using newly indexed record counts and
RocksDB's key-count estimate rather than reclaimable bytes. With similarly sized
blocks, rotating at 25%, 50% or 67% archived copies approximately 3, 1 or 0.5 bytes
for each byte reclaimed. A higher threshold reduces copying but retains more
payload between rotations. The 1,024-record floor avoids tiny generations;
transactions still provide atomic block writes independently of this schedule.

## Recovery and diagnostics

Data files, `INDEX`, and `COMMIT` are synchronized in that order before the database hash index is published. Reopening recovers records committed to files but not yet indexed, and restores CF routing from the persisted active generation. Only uncommitted tails beyond the commit record are reclaimed. Missing archives for an enabled database, committed-data corruption, or an uncertain CF publication state produce an error.

The database and archive directory together constitute the node's data. Stop the node before backing up both, and restore both from the same backup. Normal shutdown stops admission, completes accepted I/O, and synchronizes files. Cancellation or resource-budget exhaustion preserves the old generation for a later retry. Uncertain publication or native Create/Drop outcomes require reopening for recovery.

Keep both directories under the node's trusted ownership. Archive checksums detect corruption, but an actor able to replace files can recompute them. Matching the archived header hash does not authenticate the transaction body against that header.

Windows flushes regular files and replaces `COMMIT` with write-through semantics, but has no equivalent archive-directory fsync here. POSIX-equivalent power-loss durability is not guaranteed. Process-kill recovery tests do not simulate power loss on any platform.

`Freezer collection` logs report copied and omitted entries, dirty-key memory, and phase timings. `collection deferred` includes active-writer and retained-view state. The default dirty-key budget is 32 MiB; the controller's admission-buffer budget is 128 MiB. Two retained old generations prevent further collection. These limits do not bound the entire node's memory or pause duration; device synchronization latency still matters.

`ckb_freezer_size` counts committed compressed data and index bytes across all archive segments. Pending appends, commit/lock files and filesystem overhead are excluded. The gauge advances after a successful commit and is restored from the validated index when the archive reopens.

With the metrics service enabled, `ckb_freezer_state` reports `0` stopped or
disabled, `1` idle, `2` archiving, `3` collecting, or `4` failed. A failed worker
stays failed until restart. `ckb_freezer_backlog` counts eligible canonical
heights remaining in the last pass, including heights waiting for verification.
`ckb_freezer_number` is the persisted canonical archival cursor's height;
`ckb_freezer_last_progress_timestamp` is its last successful advancement in this
process (Unix seconds, initially zero).

`ckb_freezer_collection_total{outcome}` separates successful attempts from
`cancelled`, `retained_readers`, `writer_wait`, `dirty_keys`, `reconciliation`,
and unexpected `error` outcomes. `ckb_freezer_retired_generations` and
`ckb_freezer_oldest_retired_seconds` sample retained generations when the worker
returns to idle. These are periodic observations, not live per-request counts.
Use state, backlog, progress and the accompanying error log together to
distinguish an idle archive from one that has stopped progressing.

Archive records are located by exact block hash, not by treating height as a record number. Deep reorganizations do not truncate existing archives. Partial payload reads validate the compressed blocks they touch; full reads, recovery, and collection validate complete records. Headers, chain indexes, and active state remain stored, while archive files continue growing. Total node disk usage is not bounded.

## Building and verification

The CKB checkout pins its binding and native source revisions and requires Rust 1.95.0.
The binding pins RocksDB 11.8.1 and retains Snappy support for existing SSTs while
new files default to LZ4. Explicit compression options take precedence. On Linux
GNU targets, CKB statically links its existing jemalloc and bundled liburing;
other targets retain their platform defaults. See the binding README for build
requirements and codec compatibility.

Run from the CKB workspace root:

```sh
cargo test --locked -p ckb-db -p ckb-freezer -p ckb-store -p ckb-shared
make test-freezer-recovery
python3 devtools/freezer/check-upgrade.py /tmp/ckb-upgrade-1.0
```

The ordinary tests cover archive activation, hot/cold reads and writes,
reorganizations, the epoch boundary, retained views and CF rotation.
The recovery target covers interrupted file/index/CF publication, I/O failures,
cancellation and aborted-copy cleanup. Its subprocess helpers are invoked by
parent tests and remain ignored in ordinary test runs. `make test` includes this
recovery target.

The upgrade check requires Python 3.11 or later and a new output directory. It
builds the published 0.21.1 and 0.22.2 bindings separately, writes the original
19-column hot schema with each old engine, and checks both SST and synced-WAL
recovery through the current store. It then enables archival, rotates payload
CFs, restarts, and restores a stopped database/archive backup. The output keeps
the original old databases, writer sources and lockfiles, hashes, and run logs.

The published 0.22.2 writer dynamically links libclang. On macOS with Homebrew
LLVM, configure its build and runtime library paths before the upgrade check:

```sh
export LIBCLANG_PATH="$(brew --prefix llvm)/lib"
export DYLD_FALLBACK_LIBRARY_PATH="$LIBCLANG_PATH"
```

## Reproducible read profiling

Run `sh devtools/freezer/profile.sh /tmp/freezer-profile` from the workspace
root. The benchmark uses 256 committed 64 KiB records with half repeated and
half pseudorandom bytes, then compares warm-cache full-record reads with LZ4
RocksDB `get` calls. It also measures one transaction read from a 16-transaction
archived block. The script saves benchmark results, the build revision, and a
CPU sample of that selective read. macOS `sample` needs permission to inspect
the benchmark process; Linux `perf` needs access to performance counters.
Results are machine and cache-state specific, not end-to-end CKB latency.
