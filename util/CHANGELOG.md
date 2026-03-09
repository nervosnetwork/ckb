# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.3.0](https://github.com/nervosnetwork/ckb/compare/ckb-util-v1.2.0...ckb-util-v1.3.0) - 2026-03-02

### Added

- add Terminal module for CKB-TUI data provision ([#4989](https://github.com/nervosnetwork/ckb/pull/4989)) (by @zhangsoledad)
- support proxy protocol ([#5105](https://github.com/nervosnetwork/ckb/pull/5105)) (by @driftluo)
- bump crates MSRV to 1.92.0 ([#5076](https://github.com/nervosnetwork/ckb/pull/5076)) (by @doitian)

### Changed

- implement logs subscription ([#5092](https://github.com/nervosnetwork/ckb/pull/5092)) (by @Officeyutong)
- Enhance `ckb export/import` subcommand with range and verifier selection ([#4924](https://github.com/nervosnetwork/ckb/pull/4924)) (by @eval-exec)
- Upgrade rust-toolchain from 1.85.0 to 1.92.0 ([#4993](https://github.com/nervosnetwork/ckb/pull/4993)) (by @eval-exec)

### Contributors

- @zhangsoledad
- @driftluo
- @Officeyutong
- @eval-exec
- @doitian

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-util-v1.1.0...ckb-util-v1.2.0) - 2025-12-18

### Added

- add default time cost limit on indexer rpc (by @driftluo) - #5012

### Contributors

* @driftluo

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-util-v1.0.0...ckb-util-v1.1.0) - 2025-12-10

### Added

- sync use async send
- relay use async send msg
- add onion crate for Tor integration
- disabale compress on filter
- add metrics

### Other

- add publish = false to test-chain-utils crate
- Address review feedback on documentation
- Add documentation for util/types/src/core/extras.rs TODO(doc) markers
- Add documentation for util/types/src/core/cell.rs TODO(doc) markers
- Add documentation for remaining TODO(doc) markers in smaller modules
- Add documentation for module-level TODO(doc) markers
- Merge pull request #5007 from driftluo/remove-unpack-on-other-crate
- Merge branch 'develop' into develop
- upgrade molecule
- impl review advice
- fix indicatif api changes
- Fix ubuntu integration missing go env, fix clippy warning on windows
- support config onion service port by `onion_external_port`
- Change MultiAddr to Multiaddr
- Apply code readbility suggestion
- extract modify_logger_filter to mute fast-socks5 log
- add onion service configuration options
- add onion service configuration handling
- add onion/tor dependencies
- tweak tx verify workers
- increase default channel size
- Merge pull request #4986 from driftluo/optimizing-compress
