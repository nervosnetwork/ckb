All notable changes to this project will be documented in this file.
See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.


# [v0.9.0](https://github.com/nervosnetwork/ckb/compare/v0.8.0...v0.9.0) (2019-04-22)

### Bug Fixes

* #410: network panic errors r=jjyr a=jjyr

    * Peer Store no such table
    * get peer index panic

* #386: flatbuffers vtable `num_fields` overflow r=zhangsoledad a=doitian

    Refs https://github.com/nervosnetwork/cfb/pull/16

* #385: Upgrade p2p fix repeat connection bug r=jjyr a=TheWaWaR

    Related PR: https://github.com/nervosnetwork/p2p/pull/92

* #382: reset peer store connection status when setup r=TheWaWaR a=jjyr

    1. reset peer status
    2. remove banned addrs from peer_attemps result

* #424: many bug fixes of the p2p network issues fix a=TheWaWaR,zhangsoledad

### Features

* #491: update lock cell for segwit and address format a=classicalliu

* #368: segregated witness r=janx,quake a=zhangsoledad

* #409: remove uncle cellbase r=doitian a=zhangsoledad

* #369: Embed testnet chain spec in compiled binary r=doitian a=xxuejie

* #344: Revise script structure r=xxuejie a=xxuejie

* #425: Bundle app config in compiled binary a=doitian

### Improvements

* #392: avoid recursive lock a=zhangsoledad

### BREAKING CHANGES

This release has changed core data structure, please delete the old data directory.

The testnet chain spec is also changed, which is incompatible with previous versions.

Command line argument `-c` is removed, and a new command line argument `-C` is added. See `ckb help` for details.

Now the command `ckb` no longer searches the config file `nodes/default.toml`. It looks for the config file `ckb.toml` or `ckb-miner.toml` in current directory and uses the default config options when not found. A new command `ckb init` is added, see its usage with `ckb init --help`.

Config file `ckb.toml` changes:

- Removed `logger.file`, `db.path` and `network.path` from config file.
- Added config option `logger.log_to_stdout` and `logger.log_to_file`.
- Section `block_assembler` now accepts two options `binary_hash` and `args`.
- Added a new option to set sentry DSN.

File `miner.toml` changes:

- Option `spec` is moved under `chain`, which is consistent with `ckb.toml`.
- Move miner own config options under section `miner`.
- Remove `logger.file` from config file.
- Add config option `logger.log_to_stdout` and `logger.log_to_file`.

It is recommended to export the config files via `ckb init`, then apply the
modifications upon the new config files.


# [v0.8.0](https://github.com/nervosnetwork/ckb/compare/v0.7.0...v0.8.0) (2019-04-08)

### Features

* #336: index whether a tx is a cellbase in the chain r=quake a=u2

    Index whether a tx is a cellbase in the chain, prepare for the cellbase outputs maturity checking.

    Now saving the cellbase index and block number in the `TransactionMeta`, there is another implementation which creates a `HashMap<tx_hash, number>`. The second one may be a little memory saving, but this one is more simple. I think both are ok.

    https://github.com/nervosnetwork/ckb/issues/54

* #350: use TryFrom convert protocol r=doitian a=zhangsoledad
* #340: Integrate discovery and identify protocol r=jjyr a=TheWaWaR

    Known issues:

    - Shutdown network not very graceful.

* #345: Add `random_peers` function to PeerStore r=jjyr a=jjyr
* #335: Enforce `type` field of a cellbase output cell must be absent r=doitian a=zhangsoledad
* #334: Version verification r=doitian a=zhangsoledad
* #295: Replace P2P library r=quake a=jjyr

### Bug Fixes

* #365: trace rpc r=zhangsoledad a=zhangsoledad

    addition:
    * remove integration tests from root workspace
    * fix integration tests logger panic at flush

* #341: verify tx cycles in relay protocol r=zhangsoledad a=jjyr

### Improvements

* #359: merge `cell_set` `chain_state` cell provider r=quake a=zhangsoledad

* #343: use CellProvider r=zhangsoledad a=quake

    This refactoring is intended to remove closure in ChainService and duplicate code in ChainState. And fix bugs in block processing and add some test cases.

* #346: replace unwrap with expect r=doitian a=zhangsoledad
* #342: shrink lock-acquisition r=quake a=zhangsoledad
* #361: refactor network config r=jjyr a=jjyr
* #356: Unify network peer scoring r=jjyr a=jjyr
* #349: Refactor peer store r=jjyr a=jjyr

### BREAKING CHANGES

* #361: network config

```diff
[network]
- reserved_nodes = []
-only_reserved_peers = false
-max_peers = 8
-min_peers = 4
-secret_file = "secret_key"
-peer_store_path = "peer_store.db"

+reserved_peers = []
+reserved_only = false
+max_peers = 125
+max_outbound_peers = 30
+config_dir_path = "default/network"
+ping_interval_secs = 15
+ping_timeout_secs = 20
+connect_outbound_interval_secs = 15
```


# [v0.7.0](https://github.com/nervosnetwork/ckb/compare/v0.6.0...v0.7.0) (2019-03-25)


This version requires Rust 1.33.0.

### Bug Fixes

* remove use of upgradable reads ([#310](https://github.com/nervosnetwork/ckb/issues/310)) ([f9e7f97](https://github.com/nervosnetwork/ckb/commit/f9e7f97))
* `block_assembler` selects invalid uncle during epoch switch ([05d29fc](https://github.com/nervosnetwork/ckb/commit/05d29fc))
* **miner:** uncles in solo mining ([abe7a8b](https://github.com/nervosnetwork/ckb/commit/abe7a8b))


### Features

* use toml for miner and chain spec ([#311](https://github.com/nervosnetwork/ckb/issues/311)) ([4b87df3](https://github.com/nervosnetwork/ckb/commit/4b87df3))
* move config `txs_verify_cache_size` to section `tx_pool` ([06a0b3c](https://github.com/nervosnetwork/ckb/commit/06a0b3c))
* Use blake2b as the hash function uniformly ([6a42874](https://github.com/zhangsoledad/ckb/commit/6a42874))
* refactor: avoid  multiple lock ([d51c197](https://github.com/nervosnetwork/ckb/commit/d51c197))
* refactor: rename `txo_set` -> `cell_set` ([759eea1](https://github.com/nervosnetwork/ckb/commit/759eea1))
* refactor: txs verify cache required ([79cec0a](https://github.com/nervosnetwork/ckb/commit/79cec0a))

### BREAKING CHANGES

* Use TOML as config file format. Please copy and use the new TOML config file templates.
* Move `txs_verify_cache_size` to section `tx_pool`.
* Change miner config `poll_interval` unit from second to millisecond.


# [v0.6.0](https://github.com/nervosnetwork/ckb/compare/v0.5.0...v0.6.0) (2019-02-25)

### Bug Fixes

* amend trace api doc ([#218](https://github.com/nervosnetwork/ckb/issues/218)) ([f106ee8](https://github.com/nervosnetwork/ckb/commit/f106ee8))
* cli arg matches ([36902c3](https://github.com/nervosnetwork/ckb/commit/36902c3))
* db type should not be configurable ([6f51e93](https://github.com/nervosnetwork/ckb/commit/6f51e93))


### Features

* add bench for `process_block` ([bda09fc](https://github.com/nervosnetwork/ckb/commit/bda09fc))
* allow disable `txs_verify_cache` ([cbd80b2](https://github.com/nervosnetwork/ckb/commit/cbd80b2))
* block template cache ([0c8e273](https://github.com/nervosnetwork/ckb/commit/0c8e273))
* block template refresh ([9c8340a](https://github.com/nervosnetwork/ckb/commit/9c8340a))
* delay full block verification to fork switch ([#158](https://github.com/nervosnetwork/ckb/issues/158)) ([07d6a69](https://github.com/nervosnetwork/ckb/commit/07d6a69))
* impl rfc `get_block_template` ([99b6551](https://github.com/nervosnetwork/ckb/commit/99b6551))
* make rocksdb configurable via config file ([f46b4fa](https://github.com/nervosnetwork/ckb/commit/f46b4fa))
* manually shutdown ([32e4ca5](https://github.com/nervosnetwork/ckb/commit/32e4ca5))
* service stop handler ([e0143eb](https://github.com/nervosnetwork/ckb/commit/e0143eb))
* measure occupied capacity ([8ce61c1](https://github.com/nervosnetwork/ckb/commit/8ce61c1))
* refactor chain spec config ([#224](https://github.com/nervosnetwork/ckb/issues/224)) ([4f85163](https://github.com/nervosnetwork/ckb/commit/4f85163))
* upgrade RPC `local_node_id` to `local_node_info` ([64e41f6](https://github.com/nervosnetwork/ckb/commit/64e41f6))
* use new merkle proof structure ([#232](https://github.com/nervosnetwork/ckb/issues/232)) ([da97390](https://github.com/nervosnetwork/ckb/commit/da97390))
* rewrite jsonrpc http server ([6cca12d](https://github.com/nervosnetwork/ckb/commit/6cca12d))
* transaction verification cache ([1aa6788](https://github.com/nervosnetwork/ckb/commit/1aa6788))
* refactoring: extract merkle tree as crate (#223) ([a159cdf](https://github.com/nervosnetwork/ckb/commit/a159cdf)), closes [#223](https://github.com/nervosnetwork/ckb/issues/223)


### BREAKING CHANGES

* RPC `local_node_id` no longer exists, use new added RPC `local_node_info` to get node addresses.
* The chain spec path in node's configuration JSON file changed from "ckb.chain" to "chain.spec".
* Config file must be updated with new DB configurations as below

```diff
{
+    "db": {
+        "path": "db"
+    }
}
```

* RPC `get_block_template` adds a new option `block_assembler` in config file.
* Miner has its own config file now, the default is `nodes_template/miner.json`
* The flatbuffers schema adopts the new `MerkleProof` structure.


# [v0.5.0](https://github.com/nervosnetwork/ckb/compare/v0.4.1...v0.5.0) (2019-02-11)

### Features

* collect clock time offset from network peers ([413d02b](https://github.com/nervosnetwork/ckb/commit/413d02b))
* add tx trace api ([#181](https://github.com/nervosnetwork/ckb/issues/181)) ([e759128](https://github.com/nervosnetwork/ckb/commit/e759128))
* upgrade to rust 1.31.1 ([4e9f202](https://github.com/nervosnetwork/ckb/commit/4e9f202))
* add validation for `cycle_length` ([#178](https://github.com/nervosnetwork/ckb/issues/178))

### BREAKING CHANGES

* config: new option `pool.trace`


# [v0.4.0](https://github.com/nervosnetwork/ckb/compare/v0.3.0...v0.4.0) (2019-01-14)


### Bug Fixes

* unnecessary shared data clone ([4bf9555](https://github.com/nervosnetwork/ckb/commit/4bf9555))

### Features

* upgrade to Rust 1.31.1
* **cell model**: rename CellBase to Cellbase ([71dec8b](https://github.com/nervosnetwork/ckb/commit/71dec8b))
* **cell model**: rename CellStatus old -> dead, current -> live ([ede5108](https://github.com/nervosnetwork/ckb/commit/ede5108))
* **cell model**: rename OutofBound -> OutOfBound ([f348821](https://github.com/nervosnetwork/ckb/commit/f348821))
* **cell model**: rename `CellOutput#contract` to `CellOutput#_type` ([6e128c1](https://github.com/nervosnetwork/ckb/commit/6e128c1))
* **consensus**: add block level script cycle limit ([22adb37](https://github.com/nervosnetwork/ckb/commit/22adb37))
* **consensus**: past blocks median time based header timestamp verification ([c63d64b](https://github.com/nervosnetwork/ckb/commit/c63d64b))
* **infrastructure**: new merkle tree implementation ([#143](https://github.com/nervosnetwork/ckb/issues/143)) ([bb83898](https://github.com/nervosnetwork/ckb/commit/bb83898))
* **infrastructure**: upgrade `config-rs` and use enum in config parsing ([#156](https://github.com/nervosnetwork/ckb/issues/156)) ([aebeb7f](https://github.com/nervosnetwork/ckb/commit/aebeb7f))
* **p2p framework**: remove broken kad discovery protocol ([f2d86ba](https://github.com/nervosnetwork/ckb/commit/f2d86ba))
* **p2p framework**: use SQLite implement PeerStore to replace current MemoryPeerStore ([#127](https://github.com/nervosnetwork/ckb/pull/127))
* **p2p protocol**: add transaction filter ([6717b1f](https://github.com/nervosnetwork/ckb/commit/6717b1f))
* **p2p protocol**: unify h256 and ProposalShortId serialization (#125) ([62f57c0](https://github.com/nervosnetwork/ckb/commit/62f57c0)), closes [#125](https://github.com/nervosnetwork/ckb/issues/125)
* **peripheral**: add RPC `max_request_body_size` config ([4ecf813](https://github.com/nervosnetwork/ckb/commit/4ecf813))
* **peripheral**: add cycle costs to CKB syscalls ([6e10311](https://github.com/nervosnetwork/ckb/commit/6e10311))
* **peripheral**: jsonrpc types wrappers: use hex in JSON for binary fields ([dd1ed0b](https://github.com/nervosnetwork/ckb/commit/dd1ed0b))
* **scripting**: remove obsolete secp256k1 script in CKB ([abf6b5b](https://github.com/nervosnetwork/ckb/commit/abf6b5b))
* refactor: rename ambiguous tx error ([58cb857](https://github.com/nervosnetwork/ckb/commit/58cb857))


### BREAKING CHANGES

* JSONRPC changes, see the diff of [rpc/doc.md](https://github.com/nervosnetwork/ckb/pull/167/files#diff-4f42fac509e2d1b81953e419e628555c)
    * Binary fields encoded as integer array are now all in 0x-prefix hex string.
    * Rename transaction output `contract` to `type`
    * Rename CellStatus old -> dead, current -> live
* P2P message schema changes, see the diff of
  [protocol/src/protocol.fbs](https://github.com/nervosnetwork/ckb/pull/167/files#diff-bc09df1e2436ea8b2e4fa1e9b2086977)
    * Add struct `H256` for all H256 fields.
    * Add struct `ProposalShortId`
* Config changes, see the diff of
  [nodes\_template/default.json](https://github.com/nervosnetwork/ckb/pull/167/files#diff-315cb39dece2d25661200bb13db8458c)
    * Add a new option `max_request_body_size` in section `rpc`.
    * Changed the default miner `type_hash`


# [v0.3.0](https://github.com/nervosnetwork/ckb/compare/v0.2.0...v0.3.0) (2019-01-02)


### Bug Fixes

* **consensus**: resolve mining old block issue ([#87](https://github.com/nervosnetwork/ckb/issues/87)) ([e5da1ae](https://github.com/nervosnetwork/ckb/commit/e5da1ae))
* **p2p framework**: use new strategy to evict inbound peer ([95451e7](https://github.com/nervosnetwork/ckb/commit/95451e7))
* **p2p protocol**: fix calculation of headers sync timeout ([06a5e29](https://github.com/nervosnetwork/ckb/commit/06a5e29))
* **p2p protocol**: sync header verification ([366f077](https://github.com/nervosnetwork/ckb/commit/366f077))
* **scripting**: regulate parameters used in syscalls ([09e7cc7](https://github.com/nervosnetwork/ckb/commit/09e7cc7))
* cli panic ([c55e076](https://github.com/nervosnetwork/ckb/commit/c55e076))
* cli subcommand setting ([bdf323f](https://github.com/nervosnetwork/ckb/commit/bdf323f))
* uncheck subtract overflow ([#88](https://github.com/nervosnetwork/ckb/issues/88)) ([36b541f](https://github.com/nervosnetwork/ckb/commit/36b541f))

### Features

* **cell model**: rename outpoint to out\_point as its type is OutPoint (#93) ([3abf2b1](https://github.com/nervosnetwork/ckb/commit/3abf2b1)))
* **p2p framework**: add peers registry for tests([9616a18](https://github.com/nervosnetwork/ckb/commit/9616a18))
* **p2p framework**: impl NetworkGroup for peer and multiaddr ([e1e5750](https://github.com/nervosnetwork/ckb/commit/e1e5750))
* **p2p framework**: peerStore implements scoring interface ([d160d1e](https://github.com/nervosnetwork/ckb/commit/d160d1e))
* **p2p framework**: try evict inbound peers when inbound slots is full ([d0db77e](https://github.com/nervosnetwork/ckb/commit/d0db77e))
* **peripheral**: jsonrpc API modules ([f87d9a1](https://github.com/nervosnetwork/ckb/commit/f87d9a1))
* **peripheral**: use crate faketime to fake time ([#111](https://github.com/nervosnetwork/ckb/issues/111)) ([5adfd82](https://github.com/nervosnetwork/ckb/commit/5adfd82))
* **scripting**: add `DATA_HASH` field type in syscall *Load Cell By Field* ([2d0a378](https://github.com/nervosnetwork/ckb/commit/2d0a378))
* **scripting**: add dep cell loading support in syscalls ([cae937f](https://github.com/nervosnetwork/ckb/commit/cae937f))
* **scripting**: assign numeric numbers for syscall parameters ([3af9535](https://github.com/nervosnetwork/ckb/commit/3af9535))
* **scripting**: use serialized flatbuffer format in referenced cell ([49fc513](https://github.com/nervosnetwork/ckb/commit/49fc513))


### BREAKING CHANGES

* In P2P and RPC, field `outpoint` is renamed to `out_point`.
* Config has changed, please see the
  [diff](https://github.com/nervosnetwork/ckb/compare/v0.2.0...9faa91a#diff-315cb39dece2d25661200bb13db8458c).


# [v0.2.0](https://github.com/nervosnetwork/ckb/compare/v0.1.0...v0.2.0) (2018-12-17)

In this release, we have upgraded to Rust 2018. We also did 2 important refactoring:

- The miner now runs as a separate process.
- We have revised the VM syscalls according to VM contracts design experiments.

### Bug Fixes

* fix IBD sync process ([8c8382a](https://github.com/nervosnetwork/ckb/commit/8c8382a))
* fix missing output lock hash ([#46](https://github.com/nervosnetwork/ckb/issues/46)) ([51b1675](https://github.com/nervosnetwork/ckb/commit/51b1675))
* fix network unexpected connections to self ([#21](https://github.com/nervosnetwork/ckb/issues/21)) ([f4644b8](https://github.com/nervosnetwork/ckb/commit/f4644b8))
* fix syscall number ([c21f5de](https://github.com/nervosnetwork/ckb/commit/c21f5de))
* fix syscall length calculation ([#82](https://github.com/nervosnetwork/ckb/issues/82)) ([fb23f33](https://github.com/nervosnetwork/ckb/commit/fb23f33))
* in case of missing cell, return `ITEM_MISSING` error instead of halting ([707d661](https://github.com/nervosnetwork/ckb/commit/707d661))
* remove hash caches to avoid JSON deserialization bug ([#84](https://github.com/nervosnetwork/ckb/issues/84)) ([1274b03](https://github.com/nervosnetwork/ckb/commit/1274b03))
* fix `rpc_url` ([62e784f](https://github.com/nervosnetwork/ckb/commit/62e784f))
* resolve mining old block issue ([#87](https://github.com/nervosnetwork/ckb/issues/87)) ([01e02e2](https://github.com/nervosnetwork/ckb/commit/01e02e2))
* uncheck subtract overflow ([#88](https://github.com/nervosnetwork/ckb/issues/88)) ([2b0976f](https://github.com/nervosnetwork/ckb/commit/2b0976f))


### Features

* refactor: embrace Rust 2018 (#75) ([313b2ea](https://github.com/nervosnetwork/ckb/commit/313b2ea))
* refactor: replace ethereum-types with numext ([2cb8aca](https://github.com/nervosnetwork/ckb/commit/2cb8aca))
* refactor: rpc and miner (#52) ([7fef14d](https://github.com/nervosnetwork/ckb/commit/7fef14d))
* refactor: VM syscall refactoring ([9573905](https://github.com/nervosnetwork/ckb/commit/9573905))
* add `get_current_cell` rpc for fetching unspent cells ([781d5f5](https://github.com/nervosnetwork/ckb/commit/781d5f5))
* add `LOAD_INPUT_BY_FIELD` syscall ([c9364f2](https://github.com/nervosnetwork/ckb/commit/c9364f2))
* add new syscall to fetch current script hash ([#42](https://github.com/nervosnetwork/ckb/issues/42)) ([d4ca022](https://github.com/nervosnetwork/ckb/commit/d4ca022))
* dockerfile for hub ([#48](https://github.com/nervosnetwork/ckb/issues/48)) ([f93e1da](https://github.com/nervosnetwork/ckb/commit/f93e1da))
* print full config error instead of just description ([#23](https://github.com/nervosnetwork/ckb/issues/23)) ([b7d092c](https://github.com/nervosnetwork/ckb/commit/b7d092c))


### BREAKING CHANGES

* Miner is a separate process now, which must be started to produce new
  blocks.
* The project now uses Rust 2018 edition, and the stable toolchain has to be
  reinstalled:

    ```
    rustup self update
    rustup toolchain uninstall stable
    rustup toolchain install stable
    ```

  If you still cannot compile the project, try to reinstall `rustup`.



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
