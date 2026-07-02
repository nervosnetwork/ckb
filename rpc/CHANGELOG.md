# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.3](https://github.com/nervosnetwork/ckb/compare/ckb-rpc-v1.2.2...ckb-rpc-v1.2.3) - 2026-06-08

### Changed

- [rust-toolchain] Upgrade Rust toolchain to 1.95.0 (#5175) (by @eval-exec)

### Fixed

- fix overflows (by @chenyukang)
- Fix RPC test chain service teardown on Windows (#5213) (by @chenyukang)
- fix some comments to improve readability (#5209) (by @caltechustc)

### Contributors

- @chenyukang
- @caltechustc
- @eval-exec

## [1.2.2](https://github.com/nervosnetwork/ckb/compare/ckb-rpc-v1.2.1...ckb-rpc-v1.2.2) - 2026-04-24

### Changed

- *(rpc)* correct PRC typo in docs and comments (#5130) (by @eval-exec)

### Fixed

- ensure temporary directories are cleaned up after tests (#5163) (by @jetjinser)
- generate GitHub-compatible RPC doc anchors (#5136) (by @eval-exec)
- overhaul proposal selection and prioritization logic (#5023) (by @zhangsoledad)

### Contributors

- @jetjinser
- @eval-exec
- @zhangsoledad

## [1.2.1](https://github.com/nervosnetwork/ckb/compare/ckb-rpc-v1.2.0...ckb-rpc-v1.2.1) - 2026-03-08

### Changed

- update `schemars` to version 1 ([#5128](https://github.com/nervosnetwork/ckb/pull/5128)) (by @Officeyutong)

### Contributors

- @Officeyutong

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-rpc-v1.1.1...ckb-rpc-v1.2.0) - 2026-03-02

### Added

- add Terminal module for CKB-TUI data provision ([#4989](https://github.com/nervosnetwork/ckb/pull/4989)) (by @zhangsoledad)
- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- implement logs subscription ([#5092](https://github.com/nervosnetwork/ckb/pull/5092)) (by @Officeyutong)
- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)

### Contributors

- @zhangsoledad
- @Officeyutong
- @doitian
- @eval-exec

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-rpc-v1.0.0...ckb-rpc-v1.1.0) - 2025-12-10

### Added

- relay use async send msg

### Other

- minor improvement for docs
