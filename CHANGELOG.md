All notable changes to this project will be documented in this file.
See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.

# [v0.15.4](https://github.com/nervosnetwork/ckb/compare/v0.15.3...v0.15.4) (2019-07-04)

### Features

* #1151: Allow providing extra sentry info in config (@doitian)

### Bug Fixes

* #1164: Ban peer when validate received block failed (@TheWaWaR)
* #1167: Proposal reward calculate consistency (@zhangsoledad)
* #1169: Only sync with outbound peer in IBD mode (@quake)


# [v0.15.3](https://github.com/nervosnetwork/ckb/compare/v0.15.0...v0.15.3) (2019-07-02)

### Bug Fixes

* #1143: `get_ancestor` is inconsistent (@zhangsoledad)
* #1148: Sync block download filter (@zhangsoledad)

    Node should fetch download from all peers which have better chain,
    no matter it's known best's ancestor or not.


# [v0.15.0](https://github.com/nervosnetwork/ckb/compare/v0.14.2...v0.15.0) (2019-06-29)

**Important:** The default secp256k1 has changed. Now its code hash is

    0x94334bdda40b69bae067d84937aa6bbccf8acd0df6626d4b9ac70d4612a11933

### Highlights

* #922: Feat: proposer reward (@zhangsoledad)

    This is a breaking change: b:consensus

    1. earliest transaction proposer get 40% of the transaction fee as a reward.
    2. block reward finalized after proposal window close.
    3. enforce one-input one-output one-witness on cellbase.

* #1054: Replace system cell (@driftluo, @jjyr)

    See [feat: use recoverable signature to reduce tx size by jjyr · Pull Request #15 · nervosnetwork/ckb-system-scripts](https://github.com/nervosnetwork/ckb-system-scripts/pull/15)

    BREAKING CHANGE: It changes the default secp256k1 script, which now uses recoverable signature.

### Features

* #937: Initial windows support (@xxuejie)
* #931: Add a function to select all tx-hashes from storage for a block (@yangby-cryptape)
* #910: Implement the alert system in CKB for urgent situation (@jjyr)

    This is a breaking change: b:p2p, b:rpc

* #939: Explicitly specify bundled or file system (@doitian)
* #972: Add `load_code` syscall (@xxuejie)
* #977: Upgrade p2p (@driftluo)

    - upgrade p2p dependence
    - support `upnp` optional

* #978: Use new identify protocol (@driftluo)

    The current identify protocol does not play a role in identifying the capabilities of both parties, and the message structure is not reasonable.

    So, I rewrote it and added the capability ID and network ID.


* #1000: Allow miner add an arbitrary message into the cellbase (@driftluo)
* #1047: Stats uncle rate (@zhangsoledad)
* #905: Add indexer related rpc (@quake)
* #1035: `ckb init` allows setting `ba-data` (@driftluo)
* #1088: Revise epoch rpc (@zhangsoledad)

    This is a breaking change: b:rpc

### Bug Fixes

* #969: Update code hashes to correct value (@xxuejie)
* #998: Fix confusing JsonBytes deserializing error message (@driftluo)
* #1011: `witnesses_root` calculation should include cellbase (@u2)
* #1022: Avoid dummy worker re-solve the same works (@keroro520)
* #1044: Peer_store time calculation overflow (@jjyr)
* #1066: Check new block based on orphan block pool (@keroro520)
* #1077: Resolve `ChainSpec` script deserialize issue (@quake)
* #1084: Cellset consistency (@zhangsoledad)

### Improvements

* #938: Remove low S check from util-crypto (@jjyr)
* #959: Remove redundant interface from ChainProvider (@zhangsoledad)
* #970 **sync:** Fix get ancestor performance issue (@TheWaWaR)
* #976: Flatten network protocol state into `SyncSharedState` (@keroro520)
* #1051: Get `tip_header` from store instead of from `chain_state` (@jjyr)
* #971: Abstract data loader layer to decouple `ckb-script` and `ckb-store` (@jjyr)
* #994: Wrap lock methods to avoid locking a long time (@keroro520)

# [v0.14.2](https://github.com/nervosnetwork/ckb/compare/v0.14.1...v0.14.2) (2019-06-21)

### Bug Fixes

* #1076: `CellSet` is inconsistent in memory and storage (@zhangsoledad)
* #1066: Check new block based on orphan block pool (@keroro520)


# [v0.14.1](https://github.com/nervosnetwork/ckb/compare/v0.14.0...v0.14.1) (2019-06-16)

### Bug Fixes

* #1019: Miner log wrong block hash (@quake)
* #1025: Miner time interval panic (@quake)

# [v0.14.0](https://github.com/nervosnetwork/ckb/compare/v0.13.0...v0.14.0) (2019-06-15) rylai-v3

### BREAKING CHANGES

**Important**: The default secp256k1 code hash is changed to `0xf1951123466e4479842387a66fabfd6b65fc87fd84ae8e6cd3053edb27fff2fd`. Remember to update block assembler config.

This version is not compatible with v0.13.0, please init a new CKB directory.

### Features

* #913: New verification model (@xxuejie)

    This is a breaking change: b:consensus, b:database, b:p2p, b:rpc

    Based on feedbacks gathered earlier, we are revising our verification
    model with the following changes:

    * When validating a transaction, CKB will grab all lock scripts from
      all inputs, and group them based on lock script hash. The script in
      each group will only be run once. The lock script itself will have
      to do the validation task for all inputs in the same group
    * CKB will also grab all type scripts from inputs and outputs(notice
      different from previous version, the type scripts in inputs are
      included here as well), and group them based on type script hash as
      well. Each type script in each group will also be run once. The type
      script itself needs to handle the validation task within the group
    * Syscalls are also revised to allow fetching all the
      inputs/outputs/witnesses within a single group.
    * Input args is removed since the functionality can be replicated elsewhere

* #908: Peers handle disconnect (@keroro520)
* #891: Secp256k1 multisig (@jjyr)
* #845: Limit TXO set memory usage (@yangby-cryptape)

    This is a breaking change: b:database

* #874: Revise uncle rule (@zhangsoledad)

    This is a breaking change: b:consensus, b:database

    1. get rid uncle age limit
    2. try include disconnected block as uncle

* #920: Tweak consensus params (@zhangsoledad)

    This is a breaking change: b:consensus

    tweak `block_cycles_limit` and `min_block_interval`

* #897: Wrap the log macros to fix ill formed macros (@yangby-cryptape)

    And, we have to update the log filters, add prefix `ckb-` for all our crates.

* #919: Synchronizer and relayer share BlocksInflight (@keroro520)
* #924: Add a transaction error `InsufficientCellCapacity` (@yangby-cryptape)
* #926: Make a better error message for miner when method not found (@yangby-cryptape)
* #961: Display miner worker status (@quake)

    BREAKCHANGE: config file `ckb-miner.toml` changed

* #1001: `ckb init` supports setting block assembler (@doitian)

    - `ckb init` accepts options `--ba-code-hash` and `--ba-arg` (which can
    repeat multiple times) to set block assembler.
    - `ckb cli secp256k1-lock` adds an output format `cmd` to prints the
    command line options for `ckb init` to set block assembler.

    The two commands can combine into one to init the directory with a secp256k1 compressed pub key:

        ckb init $(ckb cli secp256k1-lock <pubkey> --format cmd)

* #996: Tweak consensus parameters (@doitian)

    - Change target epoch duration to 4 hours
    - Reduce epoch reward to 1/4
    - Increase secondary epoch reward to 600,000 bytes


### Bug Fixes

* #878: Calculate the current median time from tip (@keroro520)

    This is a breaking change: b:consensus

    Original implementation use `[Tip-BlockMedianCount .. Tip-1]` to calculate the current block median time. According to the notion of BlockMedianTime in [bip-0113](https://github.com/bitcoin/bips/blob/master/bip-0113.mediawiki#specification) , here change to use `[Tip-BlockMedianCount+1 .. Tip]` instead.

* #915: Sync blocked by protected peer (@TheWaWaR)
* #906: Proposal table reload (@zhangsoledad)
* #983: Uncle number should smaller than block (@zhangsoledad)

    This is a breaking change: b:consensus


### Improvements

* #981 **sync:** Fix get ancestor performance issue (@TheWaWaR)

    It's a backport of PR https://github.com/nervosnetwork/ckb/pull/970

### Misc

* #966: Backport windows support and sentry cleanup to v0.14.0 (@doitian)


# [v0.13.0](https://github.com/nervosnetwork/ckb/compare/v0.12.2...v0.13.0) (2019-06-01) rylai-v2

### Features

* #762: Live cell block hash (@keroro520)

    This is a breaking change: b:rpc

    * Return `block_hash` for `get_cells_by_lock_hash`
    * Add `make gen-doc` command

* #841: Apply `tx_pool` limit (@zhangsoledad)

    This is a breaking change: b:cli, b:rpc

    1. apply `tx_pool` limit
    2. tx size verify, enforce tx size below block size limit

    **BREAKING CHANGES:**

    **config** `ckb.toml`

    ```diff
    [tx_pool]
    - max_pool_size = 10000
    - max_orphan_size = 10000
    - max_proposal_size = 10000
    - max_cache_size = 1000
    - max_pending_size = 10000
    - txs_verify_cache_size = 100000
    + max_mem_size = 20_000_000 # 20mb
    + max_cycles = 200_000_000_000
    + max_verfify_cache_size = 100_000
    ```

    **rpc** `tx_pool_info`

    ```diff
    + "total_tx_cycles": "2",
    + "total_tx_size": "156",
    ```

* #890: Revise remainder reward rule (@zhangsoledad)

    This is a breaking change: b:consensus

* #876: Tweak consensus params (@zhangsoledad)

    This is a breaking change: b:consensus

* #889: Add codename in version (@doitian)
* #854: Calculate median time by tracing parents (@keroro520)

    At present, the way calculating the passed median time is that collects block timestamps one by one by block_number. This PR change to collects blocks timestamps by tracing parents. The new way is more robust.

    In addition to this, I use assert-style to rewrite the calculation of passed median time.

* #859: Use snappy to compress large messages (@driftluo)

    This is a breaking change: b:p2p

    On the test net monitoring, the bandwidth usage is often in a full state. We try to use the snappy compression algorithm to reduce network transmission consumption.

    After testing, the compression yield of flatbuffer format is very high, cpu consumption is relatively acceptable.

    The following is the data transmission on the test net:

    ```
    2019-05-20 16:27:41.875 +08:00 tokio-runtime-worker-7 DEBUG compress  raw_data len: 625400, compress used time: 3.635121ms, compress_data size: 335401, compression ratio: 0.536298369043812, decompress used time: 1.496667ms
    2019-05-20 16:27:42.128 +08:00 tokio-runtime-worker-6 DEBUG compress  raw_data len: 633544, compress used time: 3.789752ms, compress_data size: 335462, compression ratio: 0.5295007134468955, decompress used time: 1.490144ms
    2019-05-20 16:27:42.340 +08:00 tokio-runtime-worker-6 DEBUG compress  raw_data len: 633216, compress used time: 3.998678ms, compress_data size: 333458, compression ratio: 0.5266101930462906, decompress used time: 1.593165ms
    2019-05-20 16:27:42.558 +08:00 tokio-runtime-worker-5 DEBUG compress  raw_data len: 632992, compress used time: 3.453616ms, compress_data size: 333552, compression ratio: 0.5269450482786512, decompress used time: 1.052606ms
    2019-05-20 16:27:42.740 +08:00 tokio-runtime-worker-2 DEBUG compress  raw_data len: 633760, compress used time: 1.256847ms, compress_data size: 340022, compression ratio: 0.5365154001514769, decompress used time: 545.473µs
    2019-05-20 16:37:43.934 +08:00 tokio-runtime-worker-1 DEBUG compress  raw_data len: 186912, compress used time: 659.317µs, compress_data size: 42640, compression ratio: 0.22812874507789763, decompress used time: 515.287µs
    2019-05-20 16:37:47.338 +08:00 tokio-runtime-worker-3 DEBUG compress  raw_data len: 186520, compress used time: 189.079µs, compress_data size: 42334, compression ratio: 0.22696761741368218, decompress used time: 150.644µs
    2019-05-20 16:37:50.729 +08:00 tokio-runtime-worker-3 DEBUG compress  raw_data len: 186520, compress used time: 197.656µs, compress_data size: 42336, compression ratio: 0.22697834012438345, decompress used time: 145.5µs
    2019-05-20 16:38:52.549 +08:00 tokio-runtime-worker-4 DEBUG compress  raw_data len: 95904, compress used time: 217.968µs, compress_data size: 33801, compression ratio: 0.3524461961961962, decompress used time: 95.818µs
    2019-05-20 16:39:32.522 +08:00 tokio-runtime-worker-0 DEBUG compress  raw_data len: 47320, compress used time: 418.183µs, compress_data size: 17183, compression ratio: 0.363123415046492, decompress used time: 252.148µs
    ```

    Note that this is a **break change**, the data is modified as follows:

    By default, data above 40k enters compressed mode.

    From the current point of view, the high bit 1 is the compressed format and the high bit 0 is the uncompressed format.

    If you want to support multiple compression formats in the future, you can simply think that 0b1000 is in snappy format and 0b0000 is in uncompressed format.

    ```
     # Message in Bytes:

     +---------------------------------------------------------------+
     | Bytes | Type | Function                                       |
     |-------+------+------------------------------------------------|
     |   0   |  u1  | Compress: true 1, false 0                      |
     |       |  u7  | Reserved                                       |
     +-------+------+------------------------------------------------+
     |  1~   |      | Payload (Serialized Data with Compress)        |
     +-------+------+------------------------------------------------+
    ```

* #921: Upgrade CKB VM to latest version (@xxuejie)

    This upgrade contains the following changes:

    Refactors

    * nervosnetwork/ckb-vm#57 calculate address first before cond operation @xxuejie

    Bug fixes

    * nervosnetwork/ckb-vm#60 fix broken bench tests @mohanson
    * nervosnetwork/ckb-vm#61 VM panics when ELF uses invalid file offset @xxuejie
    * nervosnetwork/ckb-vm#63 out of bound read check in assembly VM

    Chore

    * nervosnetwork/ckb-vm#59 fix a bad way to using machine @mohanson
    * nervosnetwork/ckb-vm#61 add an example named is13 @mohanson


### Bug Fixes

* #812: Prof should respect script config (@xxuejie)
* #810: Discard invalid orphan blocks (@keroro520)

    When accepts a new block, its descendants should be accepted too if valid. So if an error occurs when we try to accept its descendants, the descendants are invalid.

* #850: Ensure EBREAK has proper cycle set (@xxuejie)

    This is a breaking change: b:consensus

    This is a bug reported by @yangby-cryptape. Right now we didn't assign proper cycles for EBREAK, which might lead to potential bugs.

* #886: Integration test cycle calc (@zhangsoledad)
* fix: Cuckoo cycle verification bug (@yangby-cryptape)

### Improvements

* #832: `peer_store` db::PeerInfoDB interface (@jjyr)


# [v0.12.2](https://github.com/nervosnetwork/ckb/compare/v0.12.1...v0.12.2) (2019-05-20)

### Features

* #838: Limit name in chainspec (@doitian)

    Only `ckb_dev` is allowed in the chainspec loaded from file.

* #840: Modify subcommand `ckb init`. (@doitian)

    - Export `specs/dev.toml` when init for dev chain.
    - Deprecate option `--export-specs`.
    - Rename `spec` to `chain` in options.
        - Add option `--chain` and deprecate `--spec`
        - Add option `--list-chains` and deprecate `--list-specs`
    - Rename `export` to `create` in messages.

* #843: Secp256k1 block assembler (@doitian)

    - Remove the default block assembler config. If user want to mine, they must configure it.

* #856: Revamp the secp256k1 support in CKB (@doitian)

    - Remove keygen feature added in #843
    - Add `ckb cli blake160` and `ckb cli blake256` utilities to compute hash.
    - Add `ckb cli secp256k1-lock` to print block assembler config from
    a secp256k1 pubkey.


# [v0.12.1](https://github.com/nervosnetwork/ckb/compare/v0.12.0...v0.12.1) (2019-05-18)

### Bug Fixes

* #825: Filter out p2p field in address (@TheWaWaR)
* #826: Ban peer deadlock (@TheWaWaR)
* #829 **docker:** Fix docker problems found in rylai (@doitian)

    - avoid dirty flag in version info
    - bind rpc on 0.0.0.0 in docker
    - fix docker files permissions

# [v0.12.0](https://github.com/nervosnetwork/ckb/compare/v0.11.0...v0.12.0) (2019-05-18) rylai-v1

### Features

* #633: Remove cycles config from miner (@zhangsoledad)
* #614: Verify compact block (@keroro520)
* #642: Incorporate assembly based CKB VM interpreter (@xxuejie)
* #622: Allow type script in cellbase (@quake)
* #620: Generalize OutPoint struct to reference headers as well (@xxuejie)
* #651: Add syscall to load current script hash (@xxuejie)
* #656: Add rpc `get_epoch_by_number` (@keroro520)
* #662: Add txs verify cache (@zhangsoledad)
* #670: Upgrade CKB VM version (@xxuejie)

    The new version contains fixes for 2 bugs revealed in comprehensive testing.

* #675: Limit sync header timeout by `MAX_HEADERS_LEN` (@keroro520)
* #678: Update lock script due to protocol changes (@xxuejie)
* #671: Add rpc get blockchain info (@keroro520)

    * Add rpc `get_blockchain_info`
    * Add rpc `get_peers_state`, currently only return the info of blocks synchronizing in flight.

* #653: Add rpc experiment module (@keroro520)

    * Add rpc `dry_run_transaction`
    * Add rpc `_compute_transaction_id`
    * Enable Experiment moduel by default

* #689: Upgrade VM to latest version (@xxuejie)

    Noticeable changes here include:

    * Shrink VM memory from 16MB to 4MB now for both resource usage and performance
    * Use Bytes in VM API to avoid unnecessary copying
    * Use i8 as VM return code for better debugging

* #686: Update default lock script to sign on transaction hash now (@xxuejie)
* #690: Use script to generate rpc doc (@keroro520)
* #701: Remove always success code hash (@xxuejie)
* #703: Stringify numbers in rpc (@keroro520)
* #709: Database save positions of CellOutputs in Transaction (@yangby-cryptape)
* #720: Move DryRuResult into jsonrpc-types (@keroro520)

    * Move `DryRunResult` into jsonrpc-types
    * Complete rpc-client used in integration testing

* #718: Initial NervosDAO implementation (@xxuejie)

    Note that this is now implemented as a native module for the ease of experimenting ideas. We will move this to a separate script later when we know more about what the actual NervosDAO implementation should look like.

* #714: Enforce resolve txs order within block (@zhangsoledad)

    Transactions are expected to be sorted within a block
    Transactions have to appear after any transactions upon which they depend

* #731: Use `future_task` to avoid blocking (@jjyr)
* #735: Panic if it's likely to reach a deadlock (@yangby-cryptape)
* #742: Verify uncle max proposals limit (@zhangsoledad)
* #736: Transaction since field support epoch-based verification rule (@jjyr)
* #745: Genesis block customization (@doitian)
* #772: Prof support start from non-zero block (@jjyr)
* #781: Add secp256k1 in dev chainspec (@doitian)
* #811: Upgrade CKB VM to latest version with performance improvements (@xxuejie)
* #822: Add load witness syscall (@xxuejie)
* #806: `peer_store` support retry and refresh (@jjyr)
* #579: epoch revision (@zhangsoledad)
* #632: Ignore staled block (@keroro520)

### Bug Fixes

* #643: A bug caused by merging a stale branch (@yangby-cryptape)
* #641: Spec consensus params (@zhangsoledad)
* #652: epoch init (@zhangsoledad)
* #655: Use the String alias type EpochNumber (@ashchan)
* #660: Information is inconsistent with the transaction pool display (@driftluo)
* #673 **tx\_pool:** insertion order when chain reorg (@zhangsoledad)
* #681: TxPoolExecutor return inconsistent result (@jjyr)
* #692: respond parse error to miner (@jjyr)
* #685: TxPoolExecutor panic when tx conflict (@jjyr)
* #695: metric transaction header mem size (@zhangsoledad)
* #688: initial block download blocked (@TheWaWaR)
* #698: blocktemplate `size_limit` calculate (@zhangsoledad)
* #697: Update p2p library fix network OOM issue (@TheWaWaR)
* #699: Use random port (@keroro520)
* #702: Compact block message flood (@quake)
* #700: Outpoint memsize (@zhangsoledad)
* #711: Update p2p to 0.2.0-alpha.11 fix send message timeout bug (@TheWaWaR)
* #712: Proposal finalize (@zhangsoledad)
* #744: block inflight timeout (@zhangsoledad)
* #743: increase protocols time event interval (@jjyr)
* #749 **deps:** upgrade p2p to 0.2.0-alpha.14 (@TheWaWaR)

    * upgrade p2p to 0.2.0-alpha.14
    * remove peer from peer store when peer id not match
    * Rollback sync/relay notify interval

* #751: token unreachable bug (@TheWaWaR)
* #753: genesis epoch remainder reward (@zhangsoledad)
* #746: block size calculation should not include uncle's proposal zones (@zhangsoledad)
* #758: fix NervosDAO calculation logic (@xxuejie)
* #776 **deps:** Upgrade p2p fix gracefully shutdown network service (@TheWaWaR)

    ✨ Silky smooth `Ctrl + C` experience ✨

* #788: correct `block_median_count` (@keroro520)
* #793: Outbound peer service and discovery service (@TheWaWaR)
* #797 **network:** Avoid dial too often (@TheWaWaR)
* #819: `load_script_hash` should use script's own hash for lock script (@xxuejie)
* #820: proposal deduplication (@zhangsoledad)
* #739: next epoch calculate off-by-one (@zhangsoledad)

### Improvements

* #729: stop processing all relay messages on IBD mod and avoiding compact block message flood (@quake)
* #640: calculate some hashes when constructing (@yangby-cryptape)
* #734: refactor block verification (@zhangsoledad)
* #634: avoiding unnecessary store lookup and trait bound tweak (@quake)
* #591: specify different structs for JSON-RPC requests and responses (@yangby-cryptape)
* #659: move VM config from chain spec to CKB config file (@xxuejie)
* #657: remove ProposalShortId hash and Proposals root (@yangby-cryptape)
* #668: store transaction hashes into database to avoid computing them again (@yangby-cryptape)
* #706: improve core type fmt debug (@zhangsoledad)
* #715: rename staging to proposed and remove trace RPC (@zhangsoledad)
* #724: don't repeat resolve tx when calculate tx fee (@zhangsoledad)
* #732: move `verification` field from ChainService struct to `process_block` fn params (@quake)
* #723: revise VM syscalls used in CKB (@xxuejie)
* #754 **network:** Spawn more than 4 tokio core threads when possible (@TheWaWaR)
* #805: only parallelism verify tx in block verifier (@quake)
* #747: make pow verify logic consistent with resolve (@zhangsoledad)

### BREAKING CHANGES

- Database version is incompatible, please remove the old data dir.
- Genesis header hash changed.
- Genesis cellbase transaction hash changed.
- System cells start from 1 in the genesis cellbase outputs instead of 0.
- System cells lock changed from all zeros to always fail.
- Always success is no longer included in dev genesis block.
- Header format changed, use proposals hash to replace proposals root.


# [v0.11.0](https://github.com/nervosnetwork/ckb/compare/v0.10.0...v0.11.0) (2019-05-14)

### Features

* #631: add a new rpc: `get_block_by_number` (@yangby-cryptape)
* #628: inspect and test well-known hashes (@doitian)
* #623: add syscall for loading transaction hash (@xxuejie)
* #599: Use DNS txt records for peer address seeding (optional) (@TheWaWaR)
* #587: lazy load cell output (@jjyr)
* #598: add RPC `tx_pool_info` to get transaction pool information (@TheWaWaR)
* #586: Relay transaction by hash (@TheWaWaR)
* #582: Verify genesis on startup (@keroro520)
* #570: check if the data in database is compatible with the program (@yangby-cryptape)
* #569: check if genesis hash in config file was same as 0th block hash in db (@yangby-cryptape)
* #559: add panic logger hook (@keroro520)
* #554: support ckb prof command (@jjyr)
* #551: FeeCalculator get transaction from cache priori (@keroro520)
* #528: capacity uses unit shannon which is `10e-8` CKBytes (@yangby-cryptape)

### Bug Fixes

* #630: blocktemplate cache outdate check (@zhangsoledad)
* #627: block assembler limit (@zhangsoledad)
* #624: only verify unknown tx in block proposal (@jjyr)
* #621: Get headers forgot update best known header (@TheWaWaR)
* #615: clean corresponding cache when receive proposals (@keroro520)
* #616: Sync message flood (@TheWaWaR)

    Avoid send too much GetHeaders when received CompactBlock (this will cause message flood)

* #618: remove rpc call to improve miner profermance (@jjyr)

    call `try_update_block_template` will take 200 ~ 400ms when node have too many txs

* #617: BlockCellProvider determine cellbase error (@jjyr)
* #612: rpc `get_live_cell` return null cell (@jjyr)
* #609: fix cpu problem (@driftluo)
* #601: Stop ask for transactions when initial block download (@TheWaWaR)
* #588: Initial block download message storm (@driftluo)
* #583: Return early for non-existent block (@keroro520)
* #584: Disconnect wrong peer when process getheaders message (@TheWaWaR)
* #581: Send network message to wrong protocol (@TheWaWaR)
* #578: Testnet hotfix (@TheWaWaR)

    **Main changes**:
    1. Adjust relay filter size to avoid message flood
    2. Send `getheaders` message when get UnknownParent Error
    3. Fix send message to wrong protocol cause peer banned
    4. Update `p2p` dependency
    5. Fix BlockAssembler hold chain state lock most of the time when cellbase is large

* #568: Add DuplicateDeps verifier (@jjyr)
* #537: testnet relay (@jjyr)

    Refactoring ugly code,
    Add `is_bad_tx` function on `PoolError` and `TransactionError`, use this method to detect a tx is intended bad tx or just caused by the different tip.

* #565: update testnet genesis hash (@doitian)

### BREAKING CHANGES

* Database is incompatible, please clear data directory.
* Config file `ckb.toml`:
    * `block_assembler.binary_hash` is renamed to `block_assembler.code_hash`.
* P2P message flatbuffers schema changed.


# [v0.10.0](https://github.com/nervosnetwork/ckb/compare/v0.9.0...v0.10.0) (2019-05-06)

### Bug Fixes

- #510: fix VM hang bug for certain invalid programs (@xxuejie)
- #509: fix incorrect occupied capacity computation for `Script` (@yangby-cryptape)
- #480: fix Transaction interface behavior inconsistency (@driftluo)
- #497: correct `send_transaction` rpc error message for unknown input or dep (@quake)
- #477: fix mining dependent txs in one block (@jjyr)
- #474: `valid_since` uses String instead u64 in RPC (@jjyr)
- #471: CuckooEngine verify invalid length proof should not panic (@quake)
- #469: fix PeerStore unique constraint failures (@jjyr)
- #455: fix Sqlite can not start (@TheWaWaR)
- #439: fix mining bug caused by type changes in RPC (@xxuejie)
- #437: RPC `local_node_info` returns duplicated addr (@rink1969)
- #418: try to repair a corrupted rocksdb automatically (@yangby-cryptape)
- #414: clear tx verfy cache when chain reorg (@zhangsoledad)


### Features

- #501: add parameters to control the block generation rate in dummy mode (@yangby-cryptape)
- #503: rpc resolve tx from pending and staging (@driftluo)
- #481: configurable cellbase maturity (@zhangsoledad)
- #473: cellbase maturity verification (@u2)
- #468: ckb must loads config files from file system. (@doitian)
- #448: relay known filter (@zhangsoledad)
- #372: tx valid since (@jjyr)
- #441: use hex string represent lock args in config (@zhangsoledad)
- #434: Change all u64 fields in RPC to String (@xxuejie)
- #422: Remove version from Script (@xxuejie)

### Improvements

- #504: refactor: check peers is_banned without query db (@jjyr)
- #476: modify jsonrpc types (@yangby-cryptape)

    - chore: let all jsonrpc types be public (for client)
    - feat: change all u64 fields in RPC to String and hide internal Script struct
    - chore: replace unnecessary TryFrom by From
    - docs: fix README.md for JSON-RPC protocols

- #435: refactor store module (@quake)
- #392: avoid recursive lock (@zhangsoledad)


### BREAKING CHANGES

- This release has changed the underlying database format, please delete the old data directory before running the new version.
- RPC `get_live_cell` returns `(null, "unknown")` when looking up null out point before, while returns `(null, "live")` now.
- RPC uses `string` to encode all 64bit integrers now.
- The executble `ckb` requires config files now, use `ckb init` to export the default ones.
- The new features tx valid sicne (#372) and removal of version from Script (#422) have changed the core data structure. They affect both RPC and the P2P messages flatbuffers schema.


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
