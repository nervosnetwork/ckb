# Changelog

Current Unreleased work inherits the objective and hard constraints in
[`architecture-contract.json`](architecture-contract.json). Live phase,
source identity, open findings and next action are owned only by the
manifest-bound [`docs/handoff/txpool-v8/`](docs/handoff/txpool-v8/) control
state. The entries below are external change history; they neither redefine
the objective nor prove that a synchronized implementation has passed terminal
correctness, performance, security or Acceptance.

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add `max_tx_pool_resident_size` and `max_tx_pipeline_resident_size` as
  conservative accepted-pool and retained-pipeline memory budgets.
- Add `verify_ordering` with `arrival_time` and `fee_rate` policies; legacy
  configuration is translated through the existing migration layer.

### Changed

- Complete the ordinary true-shard route migration so ordinary owner mutations
  use the shared lifecycle barrier plus exact shard cuts with no outer-write
  fallback. This synchronized implementation remains Unreleased and is under
  terminal correctness/root-repair audit; it is not yet a performance,
  security or Acceptance winner.
- The complete public Rust API delta is an intentional SemVer-major migration.
  The reconciled release must use a major version greater than the latest
  published `ckb-tx-pool` baseline and include migration notes. No compatibility
  facade may restore mutable transaction or policy authority; generated
  workspace reverse dependencies migrate during the landing rehearsal.
- Replace the fragmented pre-pool queues with one charged transaction
  authority and atomic Plan/Apply transitions. In-flight retained transactions
  are visible through `get_transaction` as `pending` rather than `unknown`.
- Narrow `get_raw_tx_pool.conflicted` to successfully displaced accepted
  victims retained as replacement history. Failed replacement candidates use
  the recent-reject surface.
- Write tx-pool persistence v2 while accepting legacy v1 files as migration
  input. Every restored transaction re-enters validation. Node downgrade and
  reverse persistence migration are not supported.
- Legacy tx-pool configuration files remain accepted on forward node upgrades.
  Missing resident-budget and verify-ordering fields use validated compatibility
  defaults, while the legacy verify-queue budget is translated without shrinking
  its former aggregate pipeline capacity.

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
