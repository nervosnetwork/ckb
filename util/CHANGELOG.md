# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.0](https://github.com/nervosnetwork/ckb/compare/ckb-util-v1.1.0...ckb-util-v1.2.0) - 2025-12-18

### <!-- 1 -->⛰️ Features

- add default time cost limit on indexer rpc (by @driftluo) - #5012

### Contributors

* @doitian
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
