# Freezer

Freezer stores historical block payloads in a checksummed, append-only archive. Consensus, canonical chain selection and transaction verification continue to use the normal node rules. Freezer is experimental, disabled by default and outside the Nervos bug bounty program.

## Activation

Enable segmented writes in `ckb.toml` and restart the node.

```toml
[store]
freezer_enable = true
freezer_epoch_segments = true
```

A fresh segmented database uses 16 epochs per column family (CF). For an 8-epoch store, add `freezer_epoch_width = 8` under `[db]` before activation. Both widths use the same format. Subsequent opens read the persisted width. An explicit conflicting width fails startup before CF cleanup.

The node first recovers the archive and then opens an empty segment for the current tip. Existing fixed-column rows, including unarchived side branches, stay in place. A database with a published fixed-column generation continues to resolve its existing payloads through that generation. The manifest keeps segmented writes active across restarts. Keep `freezer_enable = true` once the archive has been activated. A database configured for archival alone continues to use its fixed-column collector until segmented writes are enabled.

## Storage and reads

The default-CF key `freezer/epoch-segments/v1` stores the four bytes `ROW2`, a little-endian u64 width, and one or more little-endian u64 starts. Starts are aligned to the width and strictly increasing. The last start selects the current write destination. Each listed `freezer.epoch.<start>` CF contains a block metadata row and individually addressable transaction rows. A late arrival can enter the current segment. A far-future header cannot open arbitrary windows. Genesis stays in the fixed layout.

The exact block hash determines the payload route. An absent mapping selects the existing fixed columns, eight zero bytes select a materialized fixed override, and `E` plus an eight-byte start selects a live epoch CF. An archive mapping stores the positive record number and the original witness hashes. Each transaction, snapshot and indexer reads with its own view. An edit to an immutable payload materializes the fixed copy and changes the route in the same transaction or batch.

The archive lives under the configured `ancient` directory, normally `<data_dir>/ancient`. Its `CKBFZ006` records use LZ4, including records archived from a Snappy database. The file commit precedes publication of DB routes. Recovery checks the committed prefix and reconciles outstanding records before resuming archival. Checksums, block identity and transaction witnesses protect the corresponding read and retirement paths.

The hot RocksDB store uses LZ4 for a new database. On upgrade, the binding retains each existing column family's saved compression, so a Snappy store continues to write Snappy SSTs. New epoch column families inherit the default family's codec. An explicit `compression` in the node's RocksDB options file overrides this policy.

## Retirement

After initial block download, the worker runs once per minute and retains genesis, the current epoch and the preceding 100 complete epochs. It follows canonical hashes through a reorganization and can publish up to 30,000 eligible canonical blocks per pass. It appends batches of up to 512 blocks with a 16 MiB target.

A segment must be sealed and entirely below the retention boundary. The collector enumerates its physical members, including forks. A pending member or fixed override holds that CF. Other covered CFs can retire independently. Before retiring a CF, the collector checks the committed archive contents and witnesses against every member in a fresh view. It drains writers, rechecks retention, publishes the smaller live set synchronously, refreshes the chain snapshot and drops the CF. Existing readers may still pin its files.

If a complete scan finds only the same blocked windows left, the worker can defer up to three successive scans of that set. A changed set, a new worker or the fourth poll triggers enumeration. Canonical publication continues on every eligible pass. The worker logs each held start with the first reason found. Age and storage pressure never grant permission to drop data.

Both built-in indexer implementations can read the archive through a native RocksDB Secondary. The node's usual choice of `--indexer` or `--rich-indexer` applies. During layout changes each indexer catches up or reopens, validates its view of the manifest and archive cursor, and checks block identity before advancing.

## Recovery and upgrade

Before the first start with RocksDB 11.13.2, take a stopped-node backup, including the database and its matching archive if present. Even with Freezer disabled, a normal flush can write format-version 7 SSTs that the previous RocksDB 8.5.4 node cannot open. To return to that node, restore its pre-upgrade files and resume from the backup tip. SST compression and SST format version are independent.

Back up the stopped chain DB and its matching archive together. A published CF must exist. A future, unpublished CF must be empty before recovery removes it. After an interrupted retirement, the durable marker identifies the CFs to retain. An uncertain publication or a failed physical cleanup stops further layout writes until reopen. A missing paired archive, unknown marker or damaged published record needs investigation or restoration from a complete backup.

Ordinary startup supports unarchived hot databases and compatible fixed-generation archives. The experimental `ROW1` marker omitted the width. Startup refuses it because the same starts can have different meanings. Preserve such a pair for its matching binary. Legacy flat-file archive formats also need their matching reader.

### Optional stopped-pair rebuild

To reclaim verified duplicates left in an existing fixed-column, generation-zero store with a recovered `CKBFZ006` archive, stop the node and run the following on a POSIX filesystem.

```sh
ckb migrate -C NODE_DIRECTORY --freezer-output NEW_DIRECTORY
```

Provide space for a complete destination, a temporary on-disk index of proven duplicate keys and the original pair. The command takes the native DB and archive locks, uses the physical CF options and requires the standard CKB column set and local database paths. It streams every source row. It omits only named body, uncle, proposal or extension rows whose verified archive bytes, header, witness list and hot values agree. Height/hash mappings, fixed overrides, pending forks and opaque keys are copied. An unsupported layout or inconsistent input stops the rebuild.

A verified pair appears together under `NEW_DIRECTORY/ready/{db,ancient}` with a synced `READY` receipt. The writer reopens the closed target and compares keys and values in both directions before publication. A failed run leaves the old pair available and has no published target. Retry in a fresh directory.

For cutover, select both rebuilt paths in the stopped node configuration and enable segmented writes. Keep the original pair and configuration together. Rolling back selects that pair at its original height. Blocks and index progress accumulated after cutover belong to the newer pair. Check the tip, an archived block and the indexer tip after either switch.

## Monitoring and verification

`ckb_freezer_backlog`, `ckb_freezer_number`, `ckb_freezer_state` and `ckb_freezer_last_progress_timestamp` report publication progress. Inspect `ckb_freezer_blocked_segments`, `ckb_freezer_oldest_hold_seconds`, `ckb_freezer_blocked_sst_bytes` and the per-window log for persistent holds. The SST gauge reports RocksDB live file sizes at the last completed scan and uses -1 when unavailable. Reader pins have separate count and age gauges. Compare filesystem free space and the indexer's tip with these process metrics. `ckb_freezer_examined_bytes_total` covers the initial enumeration in completed passes. Deferred polls are counted under `ckb_freezer_collection_total{outcome="deferred"}`.

The workspace lockfile pins the binding and its native patches. Run `make test-freezer-recovery` for interruption tests. The store, shared, RPC, VM and light-client suites cover the corresponding read and service paths. The old-engine upgrade check in `devtools/freezer/check-upgrade.py` exercises stopped SST and synced-WAL inputs.
