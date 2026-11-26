# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add `verify_ordering`, defaulting to `fee_rate`, with explicit `arrival_time`
  selection also supported. Selection order does not promise completion order.
- Add a hardened VM CPU-time budget for network relay and proposal verification:
  dual-kernel startup calibration (arithmetic/stack and 256 KiB page-strided
  branch/memory kernels) with a 16× safety margin, per-script-group startup
  allowance, an 8-second (`MIN_BLOCK_INTERVAL`) cap, lazy single-flight synthetic
  recalibration, a bounded non-persistent 30-second `(tx_hash, witness_hash)`
  remote replay cooldown, and work-conserving soft peer pacing. Local RPC and
  recovery use synchronous verification without a time budget; determined
  consensus failures take precedence over simultaneous budget exhaustion.
- Add low-cardinality operational metrics covering single-writer authority write
  wait/hold latencies, `ReorgGate` waits, direct request-gate concurrency,
  descendant-index cardinality, pipeline residency, effect outbox usage, VM CPU
  budget ratios/calibration/cooldowns, and relay mailbox/reset counters.

### Changed

- Existing configurations without `verify_ordering` now select by fee rate;
  set `arrival_time` explicitly to select by arrival within each source lane.
  `max_tx_verify_cycles` separates scheduling lanes and does not cap local VM work.
- Replace the fragmented mutable pool and pre-pool queues with an **off-lock
  parallel computation → single-writer atomic commit → ordered off-lock
  publication** authority:
  - Off-lock workers resolve and verify concurrently through the 64-shard
    `TxVerificationCache` on a dedicated bounded `ComputePool` (`TxPoolCpu-*`);
    final state changes validate off-lock `Premises` and commit atomically under
    one short-hold `RwLock<AuthorityState>` (`Store::commit_with` / priority
    three-phase parallel `Store::commit_reorg` with `ReorgGate`) obeying a strict
    two-level lock hierarchy.
  - Committed effects publish strictly after lock release on a dedicated
    `TxPoolPublisher` OS thread (with batched `RecentReject` RocksDB writes),
    while callback-free in-memory local fast-path batches flat-combine inline
    via `Batch::activate_nonblocking`.
  - Only accepted owners supply public transaction bodies and `pending`/`proposed`
    status; other queries use the existing recent-rejection or `unknown` fallback.
- Replace intermediate message channels and request-worker threads with direct
  `TxPoolController` execution behind `RequestGate` (bounded ordinary, external
  read, and isolated callback-read lanes plus shutdown drain), derive compute
  permits and `active_jobs` memory budgets from one shared `ConcurrencyPlan`
  (preserving `W + 2` low-concurrency queue capacity and reserving 3 non-compute
  runtime cores before granting extra local headroom), await `send_transaction`
  asynchronously so HTTP 408 timeouts return independently of admitted work, and
  enforce one end-to-end 30-second deadline on `get_block_template`.
- Scope block priority into `VerificationPause` (pausing background queue
  workers across a chain backlog while keeping local RPCs live during DB writes
  and reconciliation) and `BlockScriptVerificationPause` (pausing local RPC VM
  execution only on blocks with uncached scripts, skipping local pause and Rayon
  dispatch on warm blocks). Ordered chain controls remain independent of waiting
  transaction handlers.
- Transaction notifications now omit events immediately when a bounded service
  or subscriber channel is full, instead of retaining a send task for up to the
  configured timeout. `notify_tx_timeout` remains accepted and is ignored.
  `ckb_notify_transaction_dropped{boundary,reason}` counts omitted handoffs.
- Maintain unified `EvictionBounds`, lazy chunked `RankedPriorities`, and
  reusable compiled template packing graphs with earlier cell-dep reader
  precedence across blocks and dependency-first proposal prefix selection.
- Intentionally change the public Rust API: fallible builder construction returns
  the controller and sole verification-result receiver; callbacks receive
  immutable snapshots and the old mutable TxPool export is removed.
  `update_tx_pool_for_reorg` no longer accepts obsolete detached proposal IDs.
  These API changes require a SemVer-major release relative to the prior
  published crate. See the
  [migration guide](docs/MAINTENANCE.md#integrating-callers-and-stored-data).
- Narrow `get_raw_tx_pool.conflicted` to successfully displaced accepted
  victims retained as replacement history. Failed replacement candidates use
  the recent-reject surface.
- Write tx-pool persistence v2 while accepting legacy v1 files as migration
  input. Every restored transaction re-enters validation. Node downgrade and
  reverse persistence migration are not supported. A successful v2 write removes
  the superseded v1 file.
- Existing tx-pool configuration files remain accepted on node upgrades;
  `max_tx_pool_size` keeps its serialized-byte meaning. Internal accepted and
  pipeline memory ceilings scale with larger serialized capacities while retaining
  execution room for small pools. Obsolete released fields remain accepted and
  ignored. See [configuration and capacity](docs/MAINTENANCE.md#configuration-and-capacity).

### Fixed

- Bind shared script proofs to the resolved cell origins visible through header
  dependencies, seal `PreparedScriptVerification` inside `ckb-verification`, and
  record zero `cpu_budget_ratio` on cached proof hits.
- Distinguish corruption inside coherent graph captures from retryable concurrent
  changes; structural faults wake blocked consumers and stop the generation.
- Skip pool publication when a chain-only import/replay has no pool consumer,
  while retaining reliable publication during ordinary node startup.

## [1.3.1](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.3.0...ckb-tx-pool-v1.3.1) - 2026-09-28

### Fixed

- prevent overflow in various calculations and add validation  (#5325) (by @chenyukang)
- Verify DAO lock size before tx-pool script execution (#5236) (by @Officeyutong)

### Contributors

- @chenyukang
- @Officeyutong

## [1.3.0](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.2.2...ckb-tx-pool-v1.3.0) - 2026-07-28

### Added

- add bearer token authentication for miner notify mode (#5257) (by @zhangsoledad)

### Changed

- simplify ancestor eviction loop (#5294) (by @eval-exec)
- enable needless_lifetimes and extra_unused_lifetimes lints (#5281) (by @eval-exec)
- avoid stored renotify permits in verify queue (#5249) (by @chenyukang)
- *(tx-pool)* simplify verify queue priority index (#5238) (by @chenyukang)
- cargo fmt --all (#5255) (by @eval-exec)
- reduce remote reject log amplification (#5250) (by @chenyukang)

### Fixed

- Fix stale parent handling during tx-pool ancestor eviction (#5293) (by @chenyukang)
- Fix some public security issues (#5219) (by @Officeyutong)
- *(relay)* fail fast on tx-pool backpressure (#5239) (by @chenyukang)
- Notify relayer when remote tx enqueue fails on full verify queue (#5235) (by @Officeyutong)
- Respect tx-pool suspend commands before popping verify tasks (#5237) (by @Officeyutong)

### Contributors

- @eval-exec
- @chenyukang
- @zhangsoledad
- @Officeyutong

## [1.2.2](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.2.1...ckb-tx-pool-v1.2.2) - 2026-06-08

### Changed

- [rust-toolchain] Upgrade Rust toolchain to 1.95.0 (#5175) (by @eval-exec)

### Fixed

- fix overflows (by @chenyukang)
- enhance orphan transaction handling and add test utilities (#5220) (by @chenyukang)
- Fix flaky ci for orphan tx (#5204) (by @chenyukang)

### Contributors

- @chenyukang
- @eval-exec

## [1.2.1](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.2.0...ckb-tx-pool-v1.2.1) - 2026-04-24

### Fixed

- overhaul proposal selection and prioritization logic (#5023) (by @zhangsoledad)

### Contributors

- @zhangsoledad

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.1.1...ckb-tx-pool-v1.2.0) - 2026-03-02

### Added

- add Terminal module for CKB-TUI data provision ([#4989](https://github.com/nervosnetwork/ckb/pull/4989)) (by @zhangsoledad)
- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)

### Contributors

- @zhangsoledad
- @doitian
- @eval-exec

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.0.0...ckb-tx-pool-v1.1.0) - 2025-12-10

### Added

- compact block async
- sync use async send
- relay use async send msg

### Other

- Add documentation for remaining TODO(doc) markers in smaller modules
- tweak tx verify workers
