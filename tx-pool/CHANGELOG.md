# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add `verify_ordering`, defaulting to `fee_rate`, with explicit `arrival_time`
  selection also supported. Selection order does not promise completion order.
- Add local active VM-time and initial-load policies, independent of consensus
  cycle accounting and block verification.

### Changed

- Use immutable transaction owners and keyed coupled commits for admission,
  replacement, chain reconciliation and administration. Independent ordinary
  transactions can commit concurrently; required effects publish afterward.
- Intentionally change the public Rust API: fallible builder construction returns
  the controller and sole verification-result receiver; callbacks receive
  immutable snapshots and the old mutable TxPool export is removed. This requires
  a SemVer-major release relative to the prior published crate. See the
  [migration guide](docs/MAINTENANCE.md#integrating-callers-and-stored-data).
- Replace the fragmented pre-pool queues with one charged transaction
  authority and atomic Plan/Apply transitions. In-flight retained transactions
  are visible through `get_transaction` as `pending` rather than `unknown`.
- Narrow `get_raw_tx_pool.conflicted` to successfully displaced accepted
  victims retained as replacement history. Failed replacement candidates use
  the recent-reject surface.
- Write tx-pool persistence v2 while accepting legacy v1 files as migration
  input. Every restored transaction re-enters validation. Node downgrade and
  reverse persistence migration are not supported.
- Existing tx-pool configuration files remain accepted on node upgrades;
  `max_tx_pool_size` keeps its serialized-byte meaning. Internal accepted and
  pipeline memory ceilings scale with larger serialized capacities while retaining
  execution room for small pools. Obsolete released fields remain accepted and
  ignored. See [configuration and capacity](docs/MAINTENANCE.md#configuration-and-capacity).

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
