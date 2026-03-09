# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-app-config-v1.1.0...ckb-app-config-v1.2.0) - 2026-03-02

### Added

- add Terminal module for CKB-TUI data provision ([#4989](https://github.com/nervosnetwork/ckb/pull/4989)) (by @zhangsoledad)
- support proxy protocol ([#5105](https://github.com/nervosnetwork/ckb/pull/5105)) (by @driftluo)
- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- Enhance `ckb export/import` subcommand with range and verifier selection ([#4924](https://github.com/nervosnetwork/ckb/pull/4924)) (by @eval-exec)
- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)

### Contributors

- @zhangsoledad
- @driftluo
- @eval-exec
- @doitian

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-app-config-v1.0.1...ckb-app-config-v1.1.0) - 2025-12-18

### Added

- add default time cost limit on indexer rpc (by @driftluo) - #5012

### Contributors

* @driftluo

## [1.0.1](https://github.com/nervosnetwork/ckb/compare/ckb-app-config-v1.0.0...ckb-app-config-v1.0.1) - 2025-12-10

### Other

- support config onion service port by `onion_external_port`
- Change MultiAddr to Multiaddr
- Apply code readbility suggestion
- add onion service configuration handling
- tweak tx verify workers
- increase default channel size
