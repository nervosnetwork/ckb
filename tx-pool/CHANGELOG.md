# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
