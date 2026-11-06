# Freezer

The freezer moves blocks that are old enough to be treated as immutable out of RocksDB and into
append-only flat files under the `ancient` directory (`ancient` in `ckb.toml`, defaulting to
`<data_dir>/ancient`). A node then reads those blocks from the flat files instead of from RocksDB.

It is **off by default**. A node opts in with:

```toml
[store]
freezer_enable = true
```

Only blocks behind the last `THRESHOLD_EPOCH` epochs are eligible, at most
`MAX_FREEZE_LIMIT` blocks are frozen per round, and a background thread runs a round every
minute. See `shared/src/shared.rs` for the schedule and `src/freezer_files.rs` for the file
format.

## Status: experimental, outside the bug bounty scope

The freezer is an experimental maintenance feature. It changes **where** old block data is kept,
not what the node considers valid: block validity and consensus rules must be the same with the
freezer on and off.

Because it is experimental, and because it is disabled on a default node, the freezer module is
**not in scope** for the Nervos bug bounty program.

## Run FailPoints

`cargo test --features fail/failpoints -- --test-threads=1`
