# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.1](https://github.com/nervosnetwork/ckb/compare/ckb-script-v1.2.0...ckb-script-v1.2.1) - 2026-09-04

### Changed

- depedencies updated in the workspace root Cargo.toml

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-script-v1.1.1...ckb-script-v1.2.0) - 2026-07-28

### Added

- *(script)* remove the code that suspend and resume the scheduler via a fully suspended state (#5262) (by @mohanson)

### Fixed

- simplify removal of terminated VMs by using remove instead of retain (#5254) (by @mohanson)

### Contributors

- @mohanson

## [1.1.1](https://github.com/nervosnetwork/ckb/compare/ckb-script-v1.1.0...ckb-script-v1.1.1) - 2026-06-08

### Changed

- Merge commit from fork (by @Officeyutong)
- Merge commit from fork (by @Officeyutong)
- [rust-toolchain] Upgrade Rust toolchain to 1.95.0 (#5175) (by @eval-exec)

### Fixed

- fix overflows (by @chenyukang)

### Contributors

- @Officeyutong
- @chenyukang
- @eval-exec

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-script-v1.0.2...ckb-script-v1.1.0) - 2026-03-02

### Added

- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)

### Contributors

- @doitian
- @eval-exec

## [1.0.1](https://github.com/nervosnetwork/ckb/compare/ckb-script-v1.0.0...ckb-script-v1.0.1) - 2025-12-10

### Other

- update Cargo.toml dependencies
