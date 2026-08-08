# Changelog

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
