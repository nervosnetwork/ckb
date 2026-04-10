# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.1](https://github.com/nervosnetwork/ckb/compare/ckb-tx-pool-v1.2.0...ckb-tx-pool-v1.2.1) - 2026-04-10

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
