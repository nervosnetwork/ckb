All notable changes to this project will be documented in this file.
See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.

# [v0.1.0](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre10...v0.1.0) (2018-11-26)


### Bug Fixes

* Chain index ([8a28fd8](https://github.com/nervosnetwork/ckb/commit/8a28fd8))
* Fix network kad discovery issue ([bc99452](https://github.com/nervosnetwork/ckb/commit/bc99452))
* Prevent multi times dialing kad connection to the same peer ([#20](https://github.com/nervosnetwork/ckb/issues/20)) ([01bcaf4](https://github.com/nervosnetwork/ckb/commit/01bcaf4))
* Fix `relay_compact_block_with_one_tx` random failure ([131d7e1](https://github.com/nervosnetwork/ckb/commit/131d7e1))
* Remove external lock reference of `network::peer_registry` ([e088fd0](https://github.com/nervosnetwork/ckb/commit/e088fd0))
* Remove redundant debug lines ([024177d](https://github.com/nervosnetwork/ckb/commit/024177d))
* Revert block builder ([#2](https://github.com/nervosnetwork/ckb/issues/2)) ([a42b2fa](https://github.com/nervosnetwork/ckb/commit/a42b2fa))
* Temporarily give up timeout ([6fcc0ff](https://github.com/nervosnetwork/ckb/commit/6fcc0ff))


### Features

* **config:** Simplify config and data dir parsing ([#19](https://github.com/nervosnetwork/ckb/issues/19)) ([b4fdc29](https://github.com/nervosnetwork/ckb/commit/b4fdc29))
* **config:** Unify config format with `json` ([d279f34](https://github.com/nervosnetwork/ckb/commit/d279f34))
* Add a new VM syscall to allow printing debug infos from contract ([765ea25](https://github.com/nervosnetwork/ckb/commit/765ea25))
* Add new type script to CellOutput ([820d62a](https://github.com/nervosnetwork/ckb/commit/820d62a))
* Add `uncles_count` to Header ([324488c](https://github.com/nervosnetwork/ckb/commit/324488c))
* Adjust `get_cells_by_redeem_script_hash` RPC with more data ([488f2af](https://github.com/nervosnetwork/ckb/commit/488f2af))
* Build info version ([d248885](https://github.com/nervosnetwork/ckb/commit/d248885))
* Print help when missing subcommand ([#13](https://github.com/nervosnetwork/ckb/issues/13)) ([1bbb3d0](https://github.com/nervosnetwork/ckb/commit/1bbb3d0))
* Default data dir ([8310b39](https://github.com/nervosnetwork/ckb/commit/8310b39))
* Default port ([fea6688](https://github.com/nervosnetwork/ckb/commit/fea6688))
* Relay block to peers after compact block reconstruction ([380386d](https://github.com/nervosnetwork/ckb/commit/380386d))
* **network:** Reduce unnessacery identify_protocol query ([40bb41d](https://github.com/nervosnetwork/ckb/commit/40bb41d))
* **network:** Use snappy to compress data in ckb protocol ([52441df](https://github.com/nervosnetwork/ckb/commit/52441df))
* **network:** Use yamux to do multiplex ([83824d5](https://github.com/nervosnetwork/ckb/commit/83824d5))
* Introduce a maximum size for locators ([143960d](https://github.com/nervosnetwork/ckb/commit/143960d))
* Relay msg to peers and network tweak ([b957d2b](https://github.com/nervosnetwork/ckb/commit/b957d2b))
* Some VM syscall adjustments ([99be228](https://github.com/nervosnetwork/ckb/commit/99be228))


### BREAKING CHANGES

* **config:** Command line arguments and some config options and chan spec options have been
changed. It may break scripts and integration tests that depends on the
command line interface.

# [v0.1.0-pre10](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre09...v0.1.0-pre10) (2018-11-01)

In this release, we added syscalls which allow contract reads cells. We are working on contract SDK, and an RPC is added to get the cells. We also did many refactorings to make the code base easier to improve in the future.

- Feature: Add an intermediate layer between the app and libp2p. @jjyr
- Feature: Use custom serialization for the redeem script hash instead of bincode.
  @xxuejie
- Feature: Add logs when pool rejects transactions.
  @xxuejie
- Feature: Add RPC to get cells by the redeem script hash. @xxuejie
- Feature: Implement `mmap_tx`/`mmap_cell` syscall to read cells in contract.
  @zhangsoledad
- Refactoring: Replace RUSTFLAGS with cargo feature. @quake
- Refactoring: Tweek Cuckoo. @quake
- Refactoring: Rename TipHeader/HeaderView member to `inner`. @quake
- Refactoring: Refactor `ckb-core`. Eliminate public fields to ease future
  refactoring. @quake
- Bug: Add proper syscall number checking in VM. @xxuejie
- Bug: Generate random secret key if not set. @jjyr
- Bug: Fix input signing bug. @xxuejie
- Test: Replace quickcheck with proptest. @zhangsoledad

VM & Contract:

- Feature: Build a mruby based contract skeleton which provides a way to write full Ruby contract @xxuejie
- Feature: Build pure Ruby `secp256k1-sha3-sighash_all` contract @xxuejie

# [v0.1.0-pre09](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre08...v0.1.0-pre09) (2018-10-17)

VM now uses RISCV 64 bit. PoW engine is configurable in runtime.

* Feature: Upgrade VM to latest version with 64 bit support @xxuejie
* Feature: Configurable PoW @zhangsoledad
* Bug: Turn on uncles verification @zhangsoledad
* Chore: Upgrade rust toolchain to 1.29.2 @zhangsoledad
* Feature: Wrapper of flatbuffers builder @quake
* Test: Add RPC for test @zhangsoledad
* Refactoring: Refactor export/import @zhangsoledad

# [v0.1.0-pre08](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre07...v0.1.0-pre08) (2018-10-04)

This release has integrated VM to verify signatures, fixed various bugs, and added more tests.

It has also introduced a newly designed transaction pool.

* Feature: Add a PoW engine which produces new blocks using RPC.  @zhangsoledad
* Feature: Enhance the integration test framework. @zhangsoledad
* Feature: Add network integration test framework. @TheWaWaR
* Feature: Redesign the pool for the new consensus rules, such as transactions proposal.  @kilb
* Feature: Integrate and use VM to verify signatures. @xxuejie
* Feature: Verify uncles PoW. @zhangsoledad
* Feature: Experiment flatbuffer.  @quake
* Bug: Fix the difficulty verification. @quake
* Bug: Fix Cuckoo panic. @zhangsoledad
* Refactoring: Add documentation and cleanup codebase according to code review feedbacks. @doitian
* Chore: Move out integration test as a separate repository to speed up compilation and test. @zhangsoledad

# [v0.1.0-pre07](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre06...v0.1.0-pre07) (2018-09-17)

This release introduces the consensus rule that transactions must be proposed via blocks first.

PoW is refactored to ease switching between different implementations.

ckb:

- Feature: Implement a consensus rule that requires proposing transactions before committing into a block. @zhangsoledad
- Feature: UTXO index cache @kilb
- Feature: Adapter layer for different PoW engines @quake
- Feature: Cuckoo builtin miner @quake
- Test: Network integration test @TheWaWaR
- Test: Nodes integration test @zhangsoledad
- Chore: Upgrade libp2p wrapper @TheWaWaR
- Chore: Switch to Rust stable channel. @zhangsoledad
- Chore: Setup template for the new crate in the repository. @zhangsoledad

ckb-riscv:

- Feature: Implement RISC-V syscalls @xxuejie

# [v0.1.0-pre06](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre05...v0.1.0-pre06) (2018-08-30)

New PoW difficulty adjustment algorithm and some bug fixings and refactoring

- Feature: new difficulty adjustment algorithm. @zhangsoledad
- Fix: undetermined block verification result because of out of order transaction verification. @kilb
- Refactor: transaction verifier. @quake

# [v0.1.0-pre05](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre04...v0.1.0-pre05) (2018-08-14)

This release introduces Uncle blocks

- Feature: Uncle Blocks @zhangsoledad
- Feature: Transaction `dep` double spending verification. @kilb
- Fix: Cellbase should not be allowed in pool. @kilb
- Fix: Prefer no orphan transactions when resolving pool conflict. @kilb
- Feature: Integration test helpers. @quake
- Fix: zero time block; IBD check @zhangsoledad
- Refactoring: Avoid allocating db col in different places @doitian

# [v0.1.0-pre04](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre03...v0.1.0-pre04) (2018-08-02)

Fix serious network issues in v0.1.0-pre03

- Refactoring: Use fnv for small key hash. @TheWaWaR
- Feature: Introduce chain spec. @zhangsoledad
- Refactoring: Rename prefix nervos to ckb. @zhangsoledad
- Feature: Ensure txid is unique in chain. @doitian
- Feature: Modify tx struct, remove module, change capacity to u64. @doitian
- Feature: Sync timeout @zhangsoledad
- Feature: simple tx signing and verification implementation. @quake
- Chore: Upgrade libp2p. @TheWaWaR
- Fix: Network random disconnecting bug. @TheWaWaR
- Feature: verify tx deps in tx pool. @kilb

# [v0.1.0-pre03](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre02...v0.1.0-pre03) (2018-07-22)

It is a version intended to be able to mint and transfer cells.

It has two limitation:

- The node stops work randomly because of network disconnecting bug.
- Cell is not signed and spending is not verified.

# [v0.1.0-pre02](https://github.com/nervosnetwork/ckb/compare/v0.1.0-pre01...v0.1.0-pre02) (2018-04-08)

First runnable node which can creates chain of empty blocks

# [v0.1.0-pre01](https://github.com/nervosnetwork/ckb/compare/40e5830e2e4119118b6a0239782be815b9f46b26...v0.1.0-pre01) (2018-03-10)

Bootstrap the project.
