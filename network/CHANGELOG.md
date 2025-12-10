# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.0](https://github.com/nervosnetwork/ckb/compare/ckb-network-v1.0.0...ckb-network-v1.1.0) - 2025-12-10

### Added

- sync use async send
- relay use async send msg
- add metrics

### Fixed

- add invalid data test

### Other

- impl review advice
- Remove `struct NetworkAddresses`
- Change MultiAddr to Multiaddr
- Apply code readbility suggestion
- update network state for public address management
- update identify protocol for onion address sharing
- update peer store to handle onion addresses
- update service builder for proxy support
- add proxy URL validation and configuration
- add NetworkAddresses struct for onion addresses
- improve compress impl
