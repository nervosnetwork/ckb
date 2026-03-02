# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-bin-v1.0.2...ckb-bin-v1.1.0) - 2026-03-02

### Added

- support proxy protocol ([#5105](https://github.com/nervosnetwork/ckb/pull/5105)) (by @driftluo)
- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- implement logs subscription ([#5092](https://github.com/nervosnetwork/ckb/pull/5092)) (by @Officeyutong)
- Enhance `ckb export/import` subcommand with range and verifier selection ([#4924](https://github.com/nervosnetwork/ckb/pull/4924)) (by @eval-exec)
- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)
- Decrease MIN_PROFILING_TIME to 2 ([#5059](https://github.com/nervosnetwork/ckb/pull/5059)) (by @eval-exec)

### Contributors

- @driftluo
- @Officeyutong
- @eval-exec
- @doitian

## [1.0.2](https://github.com/nervosnetwork/ckb/compare/ckb-bin-v1.0.1...ckb-bin-v1.0.2) - 2025-12-18

### Other

- improve robustness of bats test case load_notify_config (by @doitian) - #5033

### Contributors

* @doitian

## [1.0.1](https://github.com/nervosnetwork/ckb/compare/ckb-bin-v1.0.0...ckb-bin-v1.0.1) - 2025-12-10

### Other

- Replace unwrap() with expect() for better error messages
- fix indicatif api changes
