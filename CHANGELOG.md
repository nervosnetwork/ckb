# [v0.37.0](https://github.com/nervosnetwork/ckb/compare/v0.36.0...v0.37.0) (2020-10-20)

### Features

* #2270 **rpc:** Rework rpc doc (@doitian)
* #2299: Add a default RocksDB options file (@yangby-cryptape)

    The default options file limits the maximum memory usage.

* #2276: Improve migration progress display (@zhangsoledad)
* #2257 **rpc:** Add `ping_peers` rpc (@quake)
* #2260 **rpc:** Add `get_transaction_proof` and `verify_transaction_proof` rpc (@quake)
* #2259 **rpc:** Add `clear_banned_addresses` rpc (@quake)
* #2265 **rpc:** Add `nMinimumChainWork` config (@driftluo)

    The mainnet has been online for a long time, and it is time to add a minimum workload proof to prevent possible node attacks during the initial synchronization.

* #2269: Redesign cell store (@zhangsoledad)

    Previous cell storage is inefficient. This PR proposal a new live cell storage schema, which optimized the resolve transaction bottleneck.

    Breaking Changes:

    * This PR will migrate the database.
    * The RPC `get_cells_by_lock_hash` is deprecated and now it only returns errors.

* #2281 **rpc:** Add tx subscription RPC (@quake)

    This PR added a `new_transaction` topic to subscription rpc, user will get notified when new transaction is submitted to pool.

### Bug Fixes

* #2285: Fix the problem of disconnection caused by uncertainty (@driftluo)
* #2283: Resolve network background service cleanup issue when rpc tcp server is on (@quake)
* #2298: Skip RUSTSEC-2020-0043 temporarily (@yangby-cryptape)

### Improvements

* #2236: Rewrite discovery (@driftluo)
* #2303: Replace legacy crate `lru-cache` (@zhangsoledad)
* #2282 **test:** Create log monitor for integration test check status between nodes (@chuijiaolianying)
* #2286 **test:** Redesign the testing framework (@keroro520)
* #2294 **test:** Redesign the way of Net communicate with peers (@keroro520)
* #2268 **test:** Add some transaction checking utils (@keroro520)

# [v0.36.0](https://github.com/nervosnetwork/ckb/compare/v0.35.0...v0.36.0) (2020-09-21)

### Breaking Changes

* #2251 **RPC:** Deprecated RPC method by adding `deprecated.` prefix to the rpc name (@quake)

    This PR has also deprecated following RPC methods:

    * `get_cells_by_lock_hash`
    * All methods in the Indexer module.

### Features

* #2276: Improve database migration progress display (@zhangsoledad)
* #2199: Add metrics service (@yangby-cryptape)

    [How to enable the metrics service](https://github.com/nervosnetwork/ckb/blob/0db57dafaad73efbfcf5330ec289efba94fd6975/util/metrics-config/src/lib.rs#L5-L22)

* #2243: Refactor identify network protocol by removing `Both` (@driftluo)
* #2239: Support to control memory usage for header map (@yangby-cryptape)
* #2248: Add verbosity param to chain related rpc (@quake)

    This PR adds an optional `verbosity ` param to chain related rpc, returns data in hex format without calculated hash values, it will improve performance in some scenarios.

### Bug Fixes

* #2283: Resolve network background service cleanup issue when rpc tcp server is on. (@quake)
* #2266: Use forked metrics and forked sentry to fix RUSTSEC-2020-0041 temporarily (@yangby-cryptape)
* #2212: Advance last_common_header even the peer is worse than us (@keroro520)
* #2238: Tx-pool block_on async oneshot (@zhangsoledad)

    Replace crossbeam-channel with async oneshot

* #2216: Integration test random failures (@quake)

    While waiting for the `get_blocks` message in the integration test, we should determine whether the last block hash is equal or not.

### Improvements

* #2220: Split logger config and service (@yangby-cryptape)
* #2213: Reduce useless field and reduce get time call (@driftluo)
* #2245 **logger:** Replace lazy_static with once_cell (@zhangsoledad)
* #2229: Rewrite the ping network protocol (@driftluo)
* #2244: Re-export crossbeam-channel (@zhangsoledad)

    Re-export crossbeam-channel from facade wrapper, unify version specify.

    Use tilde requirements specify for crossbeam-channel, prevent automate dependency updates.

# [v0.35.0](https://github.com/nervosnetwork/ckb/compare/v0.34.2...v0.35.0) (2020-08-24)

### Features

* #2038 **rpc:** Re-organize RPC errors (@doitian)

    This is a breaking change: b:rpc

    This PR reworks on the RPC errors:

    * Use JSONRPC error code to differentiate different errors. Also prefix the error code in the message to be search engine friendly.
    * Make error message simple and easy to understand. The dump of the error is added as the data instead.
    * Avoid reusing the same error message for different reasons.

    **Breaking Changes**

    * The error object `data` field is always absent before, now it can be a string which contains the detailed error information.
    * The `code` in error object is always -3 for all the CKB internal errors, now it can have different values to differentiate different errors. Check the file `rpc/src/error.rs`.
    * The error messages have been updated to improve readability.

* #2049 **rpc:** Improve error messages from send transaction RPC (@doitian)

* #2178 **rpc:** Add `generate_block` RPC to IntegrationTest module (@quake)

    It allows user to generate block through RPC, it's a convenient feature for dApp integration test (like `truncate` RPC)

* #2188 **rpc:** Add sync state RPC (@driftluo)

    Wallet can fetch the best known block header the node gets from the P2P network.

* #2184 **rpc:** `tx_pool_info` include tip hash (@keroro520)

* #2144 **rpc:** Add `set_network_active` RPC (@quake)

    Allows user to pause and restart p2p network message processing through RPC.

* #2190 **rpc:** Move `add_node` / `remove_node` RPC to `Net` module (@quake)
* #2196 **rpc:** Add more fields to RPC `get_peers` (@quake)

    Added `connected_duration`, `last_ping_duration`, `protocols` and `sync_state` to `get_peers` RPC.

* #2195 **rpc:** Add more fields to `local_node_info` RPC (@quake)

    Added `active`, `connections` and `protocols` fields to `local_node_info`.

* #2159: Load db options from a file; support configuring column families (@yangby-cryptape)
* #2175: Support multiple file loggers in `ckb.toml` (@yangby-cryptape)
* #2182: Take full control of main logger filter via RPC (@yangby-cryptape)

### Bug Fixes

* #2158: Panic if db options is empty (@yangby-cryptape)
* #2157: The option of db path doesn't work (@yangby-cryptape)
* #2177: Fix the lenient logger filter parser (@yangby-cryptape)
* #2134: Update proposal table after chain reorg (@zhangsoledad)

    Previously, proposal-table update not considered in chain rollback, it's almost impossible to happen in hashrate-based chain. But can be triggered by `truncate` RPC.

* #2197: Should exit with error code when setup failed (@yangby-cryptape)

    Issue: if the config was malformed and an error was thrown in `setup_app`, the process wouldn't exit.

### Improvements

* #2152: Change storage molecule table to struct (@quake)

    This is a breaking change: b:database

    Storage structs `HeaderView`, `EpochExt` and `TransactionInfo` are fixed size, we should use molecule `struct` instead of `table`, it reduces storage size and improves the performance a little bit.

* #2150: Don't query store twice in method chaining (@yangby-cryptape)
* #2151: Reduce times of querying header map (@yangby-cryptape)
* #2147: Don't cache all data of header map in memory during IBD (@yangby-cryptape)
* #2154: Split chain iter (@zhangsoledad)
* #2153: Decoupling migration from db (@zhangsoledad)

    Previously, migration coupling with DB, this sacrifice flexibility. In a case like this, opening a read-only DB will be trouble.
    This PR proposal split migration.

* #2163: Add HeaderProvider trait and split DataLoader to smaller trait (@quake)
* #1988: Use a new method to detect headers sync timeout (@yangby-cryptape)

    To avoid possible performance issues on headers synchronization.

* #2180: Add case description and some assertion for `alert_propagation` integration test (@chuijiaolianying)
* #2179: Refactor about integration service mining relate cases. (@chuijiaolianying)
* #2189: Add case description and update case assertions for consensus related cases. (@chuijiaolianying)
* #2204: Add some trait for integration cases (@chuijiaolianying)
* #2164: Improve script error (@doitian)
* #2168: Improve error when submitting block (@doitian)
* #2169: Small tx-pool refactoring (@zhangsoledad)

    * rename ContextualTransactionVerifier -> TimeRelativeTransactionVerifier
    * split NonContextualTransactionVerifier from TransactionVerifier
    * check syntactic correctness first before
    * refactory tx-pool rejection error
    * re-broadcast when duplicated tx submit

# [v0.34.2](https://github.com/nervosnetwork/ckb/compare/v0.34.0...v0.34.2) (2020-08-08)

### Bug Fixes

* GHSA-q73f-w3h7-7wcc: Syscall to get data hash has inconsistent behaviors. (@zhangsoledad)
* GHSA-wjxc-pjx9-4wvm: Upgrade snappy to 1.0. (@quake)
* GHSA-3gjh-29fv-8hr6: Limit the decompressed size of p2p message. (@quake)

# [v0.34.0](https://github.com/nervosnetwork/ckb/compare/v0.33.1...v0.34.0) (2020-07-17)

### Features

* #2067: Optimize scheduler (@driftluo)

    Problems with the current scheduler:
    1. The calculation is too frequent
    2. Inability to adapt to complex network environments.

    This PR implements an adaptive scheduler based on past data, removing most of the redundant calculations.

* #2145: Don't cache all block status in memory (@yangby-cryptape)

    When a node is start from number 0, it will sync all headers at first, then the `block_status_map` will be full quickly.
    Then, along with the block sync, all data in `block_status_map` will be removed.
    When the IBD is done, there will be nothing left in `block_status_map`.

    But when another new-started node try to sync data from this node, this node will fetch all block statuses from database and insert them into `block_status_map` without deletions. And full block status will store in memory until the node is shutdown.

* #2113: Change logger filter dynamically via RPC (@yangby-cryptape)
* #2036: Monitor rocksdb memory usages in logs (optional; default: disable) (@yangby-cryptape)
* #2114: Add command to generate peer id (@driftluo)

    ```
    $ ckb peer-id gen --secret_path ./a.txt
    $ ckb peer-id from-secret --secret-path ./a.txt
    ```

* #2045: New subcommand replay (@zhangsoledad)

    The new subcommand can be used in both profiling and sanity check, such as verifiying the downloaded data directory archive.

* #2042: Return filename of `jemalloc_profiling_dump` (@keroro520)
* #2064: Add RPC truncate (@keroro520)

    For convenient to reproduce a specified environment when test, this PR adds RPC `truncate(target_tip_hash)` to roll-back the blockchain downto the target block.

* #2081: Update `last_common_header` only in `find_blocks_to_fetch` (@keroro520)

    `peer.best_known_block` refers to the best-known block we know this peer has announced; `peer.last_common_header` refers to the last block we both stored between local and peer. This PR proposes a new process to update the two fields.

* #2136: Add RPC `clear_tx_pool` to remove all the transactions in the tx-pool (@keroro520)

### Bug Fixes

* #2101: Resolve an unexpected shutdown issue when we got a `ProtoHandleBlock` error in p2p (@quake)
* #2124: `prepare_uncles_test` failed on `block_template` update delayed (@zhangsoledad)
* #2140: Shrink state map (@zhangsoledad)

    Cause rust hash table capacity does not shrink automatically, we need explicit call `shrink` for predictable limit memory usage.
* #2109: Fix deadlock caused by conflicting lock order (@BurtonQin)

### Improvements

* #2126: Remove fee estimator (@zhangsoledad)

    This `estimate_fee_rate` RPC is experimental, due to availability and performance issues, we decide to remove it.

* #2128: Try next listen address on parsing error (@doitian)
* #2107: Use generic key / value in template context (@quake)

    This PR changed `TemplateContext` key/value from fixed field to hashmap, it made the `ckb-resource` crate easier to use in 3rd party applications

* #2103: Use generic type in NetworkService (@quake)

    This PR changed `NetworkService`'s exit_condvar to generic type and removed node_version from `start` fn, make it easier to use `ckb-network` crate as a lib

* #2096: Move network protocol related variables to SupportProtocols (@quake)

    To eliminate dependence of `ckb-sync` crate,  this PR refactored network protocol related variables and move them to a new enum: `SupportProtocols`

# [v0.33.1](https://github.com/nervosnetwork/ckb/compare/v0.33.0...v0.33.1) (2020-07-02)

### Bug Fixes

* [GHSA-r9rv-9mh8-pxf4](https://github.com/nervosnetwork/ckb/security/advisories/GHSA-r9rv-9mh8-pxf4): BlockTimeTooNew should not be considered as invalid block (@zhangsoledad)

# [v0.33.0](https://github.com/nervosnetwork/ckb/compare/v0.33.0...v0.32.2) (2020-06-19)

### Bug Fixes

* #2052: Return connected address in RPC `get_peers` (@keroro520)

    The RPC `get_peers` miss the peer connected address. Hence it may be empty addresses returned for inbound peers.

### Improvements

* #2043: Upgrade tokio for tx-pool (@zhangsoledad)

    * bump tokio 0.2
    * refactor tx-pool with async/await

* #2100: Move all `Config` structs to ckb-app-config (@quake)

    To eliminate large dependences of `ckb-app-config`, this PR moved all config related structs to this crate and reversed dependencies of other crates

* #2091: Logger filter parse crate name leniently (@yangby-cryptape)

# [v0.32.2](https://github.com/nervosnetwork/ckb/compare/v0.32.1...v0.32.2) (2020-06-15)

* [GHSA-pr39-8257-fxc2](https://github.com/nervosnetwork/ckb/security/advisories/GHSA-pr39-8257-fxc2): Avoid crash when parsing network address (@driftluo)
* #2109: Fix deadlock caused by conflicting lock order (@BurtonQin)

# [v0.32.1](https://github.com/nervosnetwork/ckb/compare/v0.32.0...v0.32.1) (2020-05-29)

### Bug Fixes

* [GHSA-84x2-2qv6-qg56](https://github.com/nervosnetwork/ckb/security/advisories/GHSA-84x2-2qv6-qg56): Add rate limit to avoid p2p DoS attacks (@quake)

# [v0.32.0](https://github.com/nervosnetwork/ckb/compare/v0.31.1...v0.32.0) (2020-05-22)

### Features

* #2002: Avoid explosion of disordering blocks based on BLOCK_DOWNLOAD_WINDOW (@keroro520)
* #2018: Prof command support specify execution path (@zhangsoledad)
* #1999: Optimize block download tasks with a simple task scheduler (@driftluo)
* #2069: Reset testnet aggron to v4 (@doitian)
* #2084: Expose methods so we can use CKB as a library (@xxuejie)

### Bug Fixes

* nervosnetwork/tentacle#218: Fix FutureTask signals memory leak (@TheWaWaR)
* #2039: Use wrong function to get a slice to decode ping messages (@yangby-cryptape)
* #2035: Remove unsupport configurations in Cargo.toml (@yangby-cryptape)
* #2054: Fix a typo of a thread name (@yangby-cryptape)
* #2074: Orphan block pool deadlock (@quake)
* #2075: Fix collaboration issues between two protocol (@driftluo)
* #2063: Should use an empty peer store when failed to load data from file (@quake)

### Improvements

* #1968: Simplify network protocols (@TheWaWaR)
* #2006: Cache system cell for resolve deps (@zhangsoledad)

# [v0.31.1](https://github.com/nervosnetwork/ckb/compare/v0.31.0...v0.31.1) (2020-04-23)

### Bug Fixes

* [GHSA-q669-2vfg-cxcg](https://github.com/nervosnetwork/ckb/security/advisories/GHSA-q669-2vfg-cxcg): Fix undefined behavior that dereference an unaligned pointer. (@yangby-cryptape)

# [v0.31.0](https://github.com/nervosnetwork/ckb/compare/v0.30.2...v0.31.0) (2020-04-02)

### Sync Improvements

* #1947: Repair using of snapshot (@zhangsoledad)
* #1959: Improve get_ancestor efficiency (@keroro520)
* #1957: Concurrent download blocks on ibd (@driftluo)
* #1966: Enhanced locator (@driftluo)
* #1961: Fix bug on last common marked (@driftluo)
* #1985: Speed up fetch collect (@driftluo)
* #1979: Fix build_skip performance bug (@TheWaWaR)

### Features

* #1954: Add detect-asm feature to script (@xxuejie)
* #1955: Bump CKB VM to fix a performance regression (@xxuejie)
* #1948: Use module disable error instead of method not found (@driftluo)
* #1956: Replace rocksdb wrapper (@zhangsoledad)
* #1946: Use same allocator for all (@yangby-cryptape)
* #1940: Add a feature to enable jemalloc profiling (@yangby-cryptape)
* #1881: Remove memory cellset (@zhangsoledad)
* #1923: Network upgrade to async (@driftluo)
* #1978: Built-in miner should support https RPC URL (@quake)
* #1958: Log more sync and relay metrics (@keroro520)
* #1992: Add an option to control how many blocks the miner has to mine (@yangby-cryptape)

    ```bash
    ckb miner -C . --limit 10 # Exit after 10 nonces found
    ckb miner -C . -l 5       # Exit after 5 nonces found
    ckb miner -C .            # Run forever
    ckb miner -C . --limit 0  # Run forever, too
    ```

* #1993: Add metrics filter (@keroro520)

    Filter metrics via `log_enabled!` inside `metric!`.

### Bug Fixes

* #1977: Fix false positive in IllTransactionChecker (@xxuejie)
* #1996: Wait for RPC server to cleanup on shutdown (@zhangsoledad)
* #1997: Orphan_block_pool should record block origin (@zhangsoledad)

# [v0.30.2](https://github.com/nervosnetwork/ckb/compare/v0.30.1...v0.30.2) (2020-04-02)

### Bug Fixes

* #1989: Fix `build_skip` performance bug (@TheWaWaR)

# [v0.30.1](https://github.com/nervosnetwork/ckb/compare/v0.30.0...v0.30.1) (2020-03-23)

Reset Aggron the testnet genesis hash to
0x63547ecf6fc22d1325980c524b268b4a044d49cda3efbd584c0a8c8b9faaf9e1

# [v0.30.0](https://github.com/nervosnetwork/ckb/compare/v0.29.0...v0.30.0) (2020-03-20)

### Breaking Changes

* #1939: Add new response field `min_fee_rate` in RPC `tx_pool_info` (@driftluo)

    BREAKING CHANGE: RPC interface

### Features

* #1848: Add a new json rpc method `get_block_economic_state` (@yangby-cryptape)

    Replace the JSON-RPC method [`get_cellbase_output_capacity_details`].

* #1915: Reject new scripts with known bugs (@xxuejie)

    For compatibility reasons, there're certain bugs that we have to leave
    to the next hardfork to fix. However those bugs, especially VM bugs
    might lead to surprising unexpected behaviors. This change adds a new
    checker that checks against newly created cells for scripts with bugs,
    and reject those transaction when we can. This way we can alert users
    about the bugs as early as we can.

### Improvements

* #1856: Define StatusCode to indicate the result of sync operation (@keroro520)

    Learned from HTTP Response, use `StatusCode` to indicate the result of sync-operation, try to replace original `Result<T, future::Error>`.

* #1941: Uses feature flags to enable deadlock detection (@zhangsoledad)

    we should disable deadlock detection by default.
    use the `deadlock_detection` feature flag enable  deadlock detection.

* #1931: Collect metrics by logger (@keroro520)

### Bug Fixes

* #1916: Transaction should be relayed when node connects peers (@quake)
* #1921: Estimate_fee RPC error msg (@jjyr)
* #1922: `CKBProtocolContext#connected_peers` should filter peers by protocol id (@quake)
* #1950: Fix incorrect error messages for JSON uints (@yangby-cryptape)

# [v0.29.0](https://github.com/nervosnetwork/ckb/compare/v0.28.0...v0.29.0) (2020-02-26)

### Breaking Changes

* #1928: Null outputs_validator means passthrough. (@doitian)

    The default behavior is incompatible with v0.28.0, but is compatible with v0.27.1 and older versions.

# [v0.28.0](https://github.com/nervosnetwork/ckb/compare/v0.27.0...v0.28.0) (2020-01-31)

### Breaking Changes

* #1879: add `outputs_validator` to `send_transaction` rpc (@quake)

### Features

* #1900: Add RPC subscription, a.k.a, pub/sub (@quake)
* #1908: Periodically disconnect peers which open invalid sub-protocols (@jjyr)

# [v0.27.0](https://github.com/nervosnetwork/ckb/compare/v0.26.1...v0.27.0) (2020-01-10)

### Features

* #1882: Add tcp and websocket to rpc service (@quake). This is required for #1867.
* #1890 **spec:** Configurable block bytes limit (@zhangsoledad)

    Provide `max_block_bytes` option supports configurable block bytes limit.

* #1891: Notify service (@quake)

    This PR resolve #1860 and refactor network alert script notification by adding a notify service, and it's required to implement #1867.

    **configuration  file breaking change**

    ```diff
    -# [alert_notifier]
    -# # Script will be notified when node received an alert, first arg is alert message string.
    -# notify_script = "echo"
    +# [notifier]
    +# # Execute command when the new tip block changes, first arg is block struct in json format string.
    +# new_block_notify_script = "your_new_block_notify_script.sh"
    +# # Execute command when node received an network alert, first arg is alert message string.
    +# network_alert_notify_script = "your_network_alert_notify_script.sh"
    ```
### Bug Fixes

* #1889: `get_cell_meta` should return None if output index does not exist (@jjyr)
* #1895: Fix peer store saving failed due to temp dir does not exist (@jjyr)
* #1899 **tests:** Rpc server should explicit close (@zhangsoledad)

### Improvements

* #1894: Reduce useless clone / to_owned use (@driftluo)

    Reduce useless clone / to_owned use

# [v0.26.1](https://github.com/nervosnetwork/ckb/compare/v0.26.0...v0.26.1) (2019-12-30)

### Features

* #1875 **P2P:** Move feeler behind identify (@driftluo)

    after this pr, all protocol will open after `identify` open, avoid feeler interacting with different networks and compatible with older versions

* #1888: Add `get_capacity_by_lock_hash` RPC (@quake)


### Bug Fixes

* #1874 **P2P:** Remove duplicate p2p phase in discovery protocol (@jjyr)

    * Consider space-efficient, we do not store p2p phase of multiaddr in peer store.
    * Reattach the p2p phase when we send multiaddr to other nodes.

* #1859 **P2P:** Fix lost sync/relayer protocol registration (@driftluo)

    fix lost sync/relayer protocol registration

* #1873 **P2P:** Ban peer that are not on the same network (@driftluo)

    If it cannot be parser, ban it, only two possibilities:
    1. message format error（molecule）
    2. not on same net

# [v0.26.0](https://github.com/nervosnetwork/ckb/compare/v0.25.2...v0.26.0) (2019-12-13)

### Features

* #1836: Include calculated minimal fee in RPC's error response (@xxuejie)
* #1838: Add `output_data_len` and `cellbase` to `get_cells_by_lock_hash` rpc (@quake)
* #1864: Add `output_data_len` and `cellbase` to `get_live_cells_by_lock_hash` rpc (@quake)
* #1851: upgrade p2p to 0.2.7 (@driftluo)
    * Upgrade moleculec to 0.4.2
    * Add transport connection number limit on listener
* #1854: Upgrade ckb-vm to 0.18.1 (@xxuejie)
    * Tweak slot calculation algorithm

### Bug Fixes

* #1863: `fetch_random_addrs` should be able to return peers addrs (@jjyr)

### Improvements

* #1839: After exiting the IBD mode, the invalid notify should be removed (@driftluo)
* #1840: DB migration (@quake)

    Database migration may involve multiple iterations and different db (indexer/chain store), this PR added a `Migration` trait and improve the API.

* #1862: Move main chain shortcut to `get_ancestor` (@jjyr)

    Move the main chain shortcut from `get_locator` to `get_ancestor`, there are bunch functions other than `get_locator` call `get_ancestor` directly, this change saves many DB queries when the base block on main chain.

* #1853: Update error message and prompt of ckb init subcommand (@ashchan)
* #1849: No debug symbols as default and add a command to build with debug symbols (@yangby-cryptape)

# [v0.25.2](https://github.com/nervosnetwork/ckb/compare/v0.25.1...v0.25.2) (2019-11-17)

### Features

* #1824: Switch to mainnet (@doitian)

    - `ckb init` initializes mainnet node by default.
    - update docs related to mainnet.


### Improvements

* #1823: Enhance the binary packages. (@doitian)

    - Static link openssl in macOS package, so it will not require openssl as a runtime dependency.
    - Add bat files in Windows package to ease starting a node in Windows.


# [v0.25.1](https://github.com/nervosnetwork/ckb/compare/v0.25.0...v0.25.1) (2019-11-15)

Embed lina chain spec

# [v0.25.0-p1](https://github.com/nervosnetwork/ckb/compare/v0.25.0...v0.25.0-p1) (2019-11-15)

### Bug Fixes

* #1817: Fix SortedTxMap inconsistent descendants links error (@jjyr)
* #1819: Fix: txs relay order (@zhangsoledad)

# [v0.25.0](https://github.com/nervosnetwork/ckb/compare/v0.24.0...v0.25.0) (2019-11-14)

### Features

* #1785: Upgrade system script for modified multi-sign lock script (@xxuejie)

    See https://github.com/nervosnetwork/ckb-system-scripts/pull/61 for related changes.

* #1779: Upgrade rocksdb with ReadOnlyDB changes (@xxuejie)

    See https://github.com/nervosnetwork/rust-rocksdb/pull/1 for changes for the rocksdb library.

    While this won't affect CKB, it provides a different rocksdb version that can aid ReadOnly mode when using ckb packages.

* #1784: Support limit `max_tx_verify_cycles` (@jjyr)

    The purpose is to limit max verify cycles on single tx, to reduce DDOS vulnerability.

* #1788: Limit tx max ancestors count (@jjyr)

    Txs with long ancestors chain affect tx pool performance. we limit max ancestors count of a single tx to resolve this issue, tx pool will reject txs which ancestors count large than the limit.

    The default `max_ancestors_count` is 25.

* #1797: Allow specify single consensus param in spec (@zhangsoledad)
* #1803: Allow overriding system script cell capacity (@doitian)

    This make the system script cell capacity determined.


### Bug Fixes

* #1752: Return non-zero rewards for the first 11 blocks (@keroro520)

    * fix: Return non-zero rewards for the first 11 blocks
    * test: Add DAOVerifier to verify the dao_fields

* #1770: Skip cellbase short-id collision validation (@quake)
* #1791: Error message on calculate dao max withdraw (@driftluo)
* #1792: Add missing type script in RPC (@driftluo)
* #1804: Retrieve few burned ckb in genesis block (@yangby-cryptape)
* #1805: Proposal table bound (@zhangsoledad)
* #1801: Calculate interest with older withdraw header (@keroro520)

    This small bug will not cause any validity problems.

* #1813: Fix get locator performance bug (@TheWaWaR)

    When get header from main chain we can get it from snapshot

### Improvements

* #1729: DB iterator interface (@zhangsoledad)

    * get rid of useless lifetimes and unnecessary intermediate conversion code
    * property api

* #1655: Avoid reproposed uncle proposals (@keroro520)

# [v0.24.0](https://github.com/nervosnetwork/ckb/compare/v0.23.0...v0.24.0) (2019-11-02)

### Breaking Changes

* #1739: Use molecule to serialize witnesses (@jjyr)

    System contracts read witness as serialized `WitnessArgs`

* #1769: Adapt to 2-phase Nervos DAO implementation (@xxuejie)

    Depends on https://github.com/nervosnetwork/ckb-system-scripts/pull/59

* #1726: Tweak consensus params (@zhangsoledad)

    * `TWO_IN_TWO_OUT_COUNT`   3875 ->  1600
    * `MAX_BLOCK_PROPOSALS_LIMIT` ->  2400
    * remove useless `HEADER_VERSION`
    * `BLOCK_VERSION`, `TX_VERSION`, `TYPE_ID_CODE_HASH`  move to `consensus`

* #1707: Resolve uncles hash calculation issue (@quake)

    Uncles hash is the blake2b on concatenated uncle block hashes.

* #1785: Upgrade system script for modified multi-sign lock script (@xxuejie)

    See https://github.com/nervosnetwork/ckb-system-scripts/pull/61 for related changes.

### Features

* #1701: Zeroize  when privkey dropped (@zhangsoledad)
* #1711: Enable ansi support for windows (@zhangsoledad)
* #1720: Add load transaction syscall (@xxuejie)
* #1730: Upgrade CKB VM to 0.18.0 (@xxuejie)

    See https://github.com/nervosnetwork/ckb-vm/releases/tag/0.18.0 for
    changes in CKB VM 0.18.0

* #1731: Security issuance satoshi cell by use all zeros lock (@jjyr)
* #1659: Fee estimate RPC (@jjyr)

    This PR adds a new RPC [estimate_fee_rate](https://github.com/nervosnetwork/ckb/pull/1659/files#diff-622e6d119ac5d43f7eb41cb596159f9fR907). It takes the basic idea from bitcoin's [estimatesmartfee](https://bitcoincore.org/en/doc/0.16.0/rpc/util/estimatesmartfee/), however, we ignore the magic numbers and tricks from the original code.

    We estimate the tx fee rate by track txs that entered tx pool. See details https://github.com/nervosnetwork/ckb/pull/1659/files#diff-ff03764a87b23e747dadc645fcf8df8bR21

* #1705: Verify genesis block specific rules on start (@jjyr)
* #1735: Expose methods to tweak CKB VM with CKB runtime outside CKB (@xxuejie)
* #1757: Shutdown when protocol handle panic (@driftluo)
* #1740: Add multisig system script cell (@jjyr)

    https://github.com/nervosnetwork/ckb-system-scripts/pull/60

* #1772: Limit p2p protocol message size (@TheWaWaR)

### Bug Fixes

* #1712: Fix `tx_pool_info` (@u2)
* #1697: Fix `get_header_view` panic bug (@TheWaWaR)

    Update best headers(peers/global) after update header_map

* #1714: Tx `sorted_keys` order by relation (@u2)
* #1736: Fix tx pool inconsistent when receive duplicated hash txs. (@jjyr)
* #1741: Overflow panic in `load_cell_data_as_code` syscall (@xxuejie)
* #1743: Exclude primary/secondary issuance in genesis (@keroro520)
* #1742: Ignore fork branch when `get_cellbase_output_capacity_details`, `get_header` and `get_block` rpc (@u2)
* #1765: Avoids creating tmp folder for db initialization (@quake)
* #1763: Fix cli output and ban reason (@driftluo)
* #1752: Return non-zero rewards for the first 11 blocks (@keroro520)

    * fix: Return non-zero rewards for the first 11 blocks
    * test: Add DAOVerifier to verify the `dao_fields`

### Improvements

* #1515: Change enum from `[byte; 1]` to `byte` (@quake)
* #1760: Replace non-maintained jsonrpc client (@zhangsoledad)
* #1768: Unified protocol handshake information format (@driftluo)
* #1729: Refactor DB iterator interface (@zhangsoledad)

# [v0.23.0](https://github.com/nervosnetwork/ckb/compare/v0.22.0...v0.23.0) (2019-10-05)

### Features

* #1645: Min transaction fee filter (@jjyr)

### Bug Fixes

* #1696: Set `next_epoch_diff` to one instead of panic when it is zero (@doitian)
* #1683: Remove descendants of committed txs from pending pool (@keroro520)
* #1698: WebAssembly build for core packages (@xxuejie)
* #1665: Remove committed before expired during reorg (@keroro520)
* #1706: Fix orphan tx package (@zhangsoledad)
* #1712: Fix `tx_pool_info` (@u2)

    Count transactions in gap in pending.

* #1697: Fix `get_header_view` panic bug (@TheWaWaR)

    Update best headers (peers/global) after update header_map.

* #1714: Tx `sorted_keys` order by relation (@u2)
* #1736: Fix tx pool inconsistent when receive duplicated hash txs. (@jjyr)

# [v0.22.0](https://github.com/nervosnetwork/ckb/compare/v0.21.2...v0.22.0) (2019-10-05)

### Breaking Changes

* #1585: Include fractions in epoch number representations (@xxuejie)

    This change introduce fractions in 2 places where epoch numbers might
    be used:

    * The epoch field in the header
    * Cell input's since part when using epoch values

    Here we use a rational number format to represent epoch number.
    The lower 54 bits of the epoch number field are split into 3
    parts(listed in the order from higher bits to lower bits):

    * The highest 16 bits represent the epoch length
    * The next 16 bits represent the current block index in the epoch
    * The lowest 24 bits represent the current epoch number

    Assuming we are at block number 11555, epoch 50, and epoch 50 starts
    from block 11000, has a length of 1000. The epoch number for this
    particular block will then be 9326559282, which is calculated
    in the following way:

    ```
    50 | ((11555 - 11000) << 24) | (1000 << 16)
    ```

* #1643: Compress header (@zhangsoledad)

    * remove `uncles_count`
    * merge `transactions_root` and `witnesses_root`, where `new
      transactions_root = blake256(old transactions_root || old
      witnesses_root)`
    * replace difficulty with `compact_target`

* #1632: Change script args and witness to single bytes (@quake)

    1. Change args and witness from `Vec<Bytes>` to `Bytes`.
    2. Add `load_script` system call. The `main` method no longer receives
       script args as argv.

* #1599: Adjust NervosDAO stats calculation logic (@xxuejie)

    The rules to generate dao field in block header has changed.

* #1618: Change return type of RPC submit block (@keroro520)

    Change return type of RPC `submit_block` from `Result<Option<H256>>` to `Result<H256>`

* #1641: Script cycle adjustments (@xxuejie)

* #1646: Use epoch as the basic maturity unit (@yangby-cryptape)

    Cellbase outputs can be used after 4 epochs.

* #1609: Use DAO type script hash in DAO transaction (@TheWaWaR)

    DAO deposite must use *type* as the `hash_type` to reference the DAO system script.

* #1617: Setup issuance schedule (@doitian)

    BREAKING CHANGE: primary/secondary epoch reward has changed

* #1666: Expand nonce to 128-bit (@zhangsoledad)

    * Expand nonce to 128-bit
    * Change `pow_message` from `[nonce + pow_hash]` to `[pow_hash + nonce]`

### Features

* #1602: Use all zeros as lock script which can never be unlocked (@driftluo)
* #1674: Allow putting a message in cellbase witness (@TheWaWaR)
* #1681: Allow setting the spec file in ckb init (@doitian)

### Bug Fixes

* #1622: Default executor misused (@zhangsoledad)
* #1613: Use `serialized_size` while `calculate_txs_size_limit` (@u2)
* #1660: JSON type number must use hex string (@driftluo)
* #1678: `get_block_transactions_process` should fill missing uncle in response (@zhangsoledad)

# [v0.21.2](https://github.com/nervosnetwork/ckb/compare/v0.21.0...v0.21.2) (2019-09-26)

### Bug Fixes

* #1623: Default executor misused (@zhangsoledad)
* #1619: Peer store persistent (@jjyr)
* #1629: Fix peer store `fetch_random` return empty (@jjyr)
* #1644: Fix duplicate p2p phase in `get_peers` (@jjyr)

# [v0.21.0](https://github.com/nervosnetwork/ckb/compare/v0.20.0...v0.21.0) (2019-09-21)

### Breaking Changes

* #1527: RPC `get_live_cell` added `with_data` argument and changed the response structure. (@TheWaWaR)
* #1528: P2P uses molc to serialize handshake message. (@driftluo)
* #1551: Cellbase output data must be empty. (@driftluo)

    Because the ckb reward is delayed, the ownership of the cellbase of the current block is not the miner who digs out the block, so cellbase's output data must be disabled.

* #1550: Headers can only be used in header deps after maturity period (@xxuejie)
* #1518: Add `chain_root` to block header. (@jjyr)

    **Attention**: We're going to revert this in the final release.

* #1584: Hexilize jsonrpc numbers (@keroro520)

    * feat: Return numbers in heximal format(without leading zeros)
    * feat: Allow accept numbers in heximal format
    * feat: Refuse numbers with redundant leading zeros

    Reference: https://github.com/ethereum/wiki/wiki/JSON-RPC#hex-value-encoding

* #1559: Block serialized size should not include uncles proposals serialized size (@yangby-cryptape)
* #1592: RPC returns errors on unknown request fields. (@TheWaWaR)

### Features

* #1510: Check system cells lock script when build genesis block (@TheWaWaR)
* #1538: Overall bench (@zhangsoledad)
* #1574: Keep only one version of VM by removing ScriptConfig (@xxuejie)
* #1568: Indexer configuration (@quake)
* #1571: `is_better_chain` uses first-received policy (@u2)
* #1586: Script package build adjustment (@xxuejie)
* #1558: Expose method to invoke a single script on a transaction (@xxuejie)

    This can be helpful in CKB script debugger's development

* #1576: Satoshi's gift (@jjyr)

    This PR allows a special cell "satoshi's gift" in the genesis block. When the mainnet launching, this cell will issue 1/4 of total genesis capacity, and 60% capacity of the cell will be calculated as occupied, this affects the Nervos DAO contract interests.

    Satoshi's gift cell, as the name, the lock script of this cell verifies an `H160(pubkey)` that satoshi used in Bitcoin's genesis, satoshi can use the private key to sign a tx to spent the cell on CKB.

### Bug Fixes

* #1533: Consensus constructor should init epoch reward from parameter. (@doitian)
* #1534: Update secp type script hash in genesis (@doitian)
* #1608: Fix AddrManager out of index error (@jjyr)

### Improvements

* #1386: Create ckb-error and use it as the global system error type (@keroro520)

# [v0.20.0](https://github.com/nervosnetwork/ckb/compare/v0.19.2...v0.20.0) (2019-09-07)

### Features

* #1464: Use secp256k1 referenced by hash type "type" as the default lock. (@doitian)
* #1505: Refactor serialization schema for performance. (@doitian)
* #1508: Add `load_header_by_field` syscall for fetching epoch data (@xxuejie)
* #1469: Change `Header#dao` to Byte32 (@quake)

### Bug Fixes

* #1393: Use path-clean (@zjhmale)
* #1463: Change default lock script (@TheWaWaR)
* #1413: Use remote peer observed address (@jjyr)

    Observe address is a feature to leverage remote peers to report external IPs for a node.

* #1487: Fix dao statistics in genesis block (@TheWaWaR)
* #1519: Indexer store should lock the state when syncing data (@quake)
* #1548: Consensus constructor should init epoch reward from parameter. (@doitian)
* #1555: Fix orphan block race storage (@keroro520)
* #1562: Block serialized size should not include uncles proposals serialized size (@yangby-cryptape)
* #1590: Fix genesis DAO satoshi gift incorrect calculation (@jjyr)
* #1588: Fix potential inconsistency (@zhangsoledad)

    1. remove `ChainProvider`, it's superfluous.
    2. fix access storage directly in many places, it has no consistency guarantee, it's potential problems

### Improvements

* #1352: Conduct GCD before rational ops (@u2)
* #1486: Use Byte32 to replace the majority of H256 (@yangby-cryptape)
* #1520: Resolve indexer store performance issue (@quake)
* #1485: Compact Block only includes uncle blocks hash (@u2)

    The peer should already have the uncle blocks at a high probability. If the peer cannot find the uncle locally, use download uncle to get the header and proposals.

* #1569: Rewrite peer store, remove the dependencies of SQLite. (@jjyr)
* #1587: Remove global store cache (@zhangsoledad)
* #1567: Rewrite pool as a service (@zhangsoledad)

# [v0.19.2](https://github.com/nervosnetwork/ckb/compare/v0.18.2...v0.19.2) (2019-08-24)

### Features

* #1297: Add RPC `get_block_finalized_reward_info` (@u2)

    Get info about the amount of every part in the reward.

* #1270: When a node is in IBD, it will tell others it is in IBD as the response on requests sent from peers. (@driftluo)
* #1312: Upgrade CKB VM to 0.15.1 (@xxuejie)

    Please refer to the following URLs for changes from 0.13.0 to 0.15.1 in CKB VM.

    https://github.com/nervosnetwork/ckb-vm/releases/tag/v0.14.0
    https://github.com/nervosnetwork/ckb-vm/releases/tag/0.15.0
    https://github.com/nervosnetwork/ckb-vm/releases/tag/0.15.1

    One important note is that even though CKB VM supports the all-new AOT mode right now, we are still only using the ASM interpreter in CKB since the performance is already good enough.

* #1252: Uncle descendant limit (@zhangsoledad)

    This is a breaking change: b:consensus

    A block B1 is considered to be the uncle of another block B2 if B1's parent is either B2's ancestor or embedded in B2 or its ancestors as an uncle.

* #1316: Add script hash type in block assembler config (@xxuejie)

    This is a breaking change: b:cli

* #1311: Remove RPC `_compute_code_hash` (@doitian)

    This is a breaking change: b:rpc

* #1329: Allow dep and input in the same transaction use the same previous output (@TheWaWaR)

    This is a breaking change: b:consensus

* #1323: Relay new transaction hashes in batch (@u2)

    This is a breaking change: b:p2p

* #1319: Leverage rocksdb transaction (@zhangsoledad)
* #1343: Tweak cellbase maturity (@zhangsoledad)

    This is a breaking change: b:consensus

* #1307: Difficulty adjustment rfc version (@zhangsoledad)

    This is a breaking change: b:consensus, b:database

    Apply new difficulty adjustment mechanism according to [RFC](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#dynamic-difficulty-adjustment-mechanism)

* #1342: Add bench test to run secp256k1 lock script (@zhangsoledad)
* #1341: Implement cell's type ID as special system script (@xxuejie)
* #1383: Allow DNS resolver on rpc server config (@driftluo)
* #1385 **ckb-bin:** Add interactive mode for init sub command (@zjhmale)
* #1382 **ckb-bin:** Add reset data subcommand (@zjhmale)
* #1381: Split load data logic from load code syscall (@xxuejie)
* #1387: Add dep group support (@TheWaWaR)

    This is a breaking change: b:consensus, b:database

* #1335: Pool sorts transactions by fee rate (@jjyr)
* #1415: Ignore genesis cellbase maturity rule (@TheWaWaR)
* #1427: Upgrade system cells with dep group support (@TheWaWaR)
* #1356: Refactor transaction structure, split deps into cell deps and header deps. (@TheWaWaR)
* #1249: New serialization (@yangby-cryptape)
* #1317: Fill get peers RPC version field (@driftluo)

    BREAKING CHANGE: identify message adds a new field

* #1318: Only accept blocks with a height greater than tip - N (@u2)
* #1305: IBD only with protect/whitelist peers (@driftluo)
* #1379: Expose data field in jsonrpc-types' Witness (@xxuejie)
* #1359: Allow multiple type ID cell creation in single transaction (@xxuejie)
* #1384: Chain snapshot (@zhangsoledad)

    Introduce chain snapshot, which leverage rocksdb `snapshot` and `hamt` to achieve captures point-in-time view of the chain,  get rid of `chain_state` and global lock. get 3x improve when switch fork.

* #1423: Use
  [eaglesong](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0010-eaglesong/0010-eaglesong.md) as new pow (@quake)
* #1451: Add type script for some system cells (@TheWaWaR)
* #1417: Remove `data_hash` from CellOutput (@quake)

    This is a breaking change: b:consensus, b:rpc, b:database

    This PR removed `data_hash` from CellOutput and refactored `Transaction` struct in `ckb.mol`

* #1454: Change `is_dep_group` (bool) to `dep_type` (enum) and use underscore
  case for all enum values in RPC.

    This is a breaking change: b:consensus, b:rpc, b:database

### Bug Fixes

* #1267: Header verifier uses the wrong header resolver (@u2)
* #1282: Byte index is not a char boundary for non-ASCII char (@yangby-cryptape)
* #1268: Node should not reject the compact block which is in a worse fork (@u2)
* #1306: Network should not retry dialing on failed address (@jjyr)
* #1310: Compact block median time is wrong (@u2)
* #1313: Fix Tx pool config typo and new config param (@quake)

    fix typo `max_verfify_cache_size` => `max_verify_cache_size`, txs verify cache should use this config value instead of a hardcoded value.

    added a new config param for conflict txs cache capacity

* #1309: Script hash type should be preserved when converting to/from witness (@xxuejie)
* #1314: Fix Randomly failed integration test valid since (@jjyr)
* #1322: Transaction is rejected if a script matches deps via type and there are multiple matches (@xxuejie)
* #1337: Fix process block bench (@zhangsoledad)
* #1334: Remove useless `epoch_reward` from EpochView in RPC (@spartucus)
* #1411: Fix Header provider index check (@zhangsoledad)
* #1349: Fix Compact block `short_id` collition (@u2)
* #1432: Pool should return error when `pending_tx` failed (@u2)
* #1442: Use new serialization to calculate `type_id` (include since) (@TheWaWaR)
* #1399: Should check `sync_started` before handle `InIBD` (@keroro520)
* #1455: Remove `block_ext` cache to fix data inconsistence (@u2)

### Improvements

* #1236: Add BlockTransactions verifier (@u2)
* #1280: Explicit deny alert when version does not match (@jjyr)
* #1286: Use `block_hash` instead of `block_number` in `get_block_proposal` message (@u2)
* #1279: Extract data field from CellOutput to Transaction (@jjyr)
* #1308: Use ProposalShortId in CompactBlock (@quake)

    This is a breaking change: b:p2p

    This PR changes `CompactBlock#short_id` to `ProposalShortId`, and `reconstruct_block` will try to get tx from the entire tx pool instead of proposal tx pool only.

* #1326: Add committed txs cache for compact block reconstruction (@quake)
* #1336: Refactoring block body store (@quake)

    This PR splits block body (transactions) into small value store and use rocksdb prefix seek API to improve the DB fetch performance.

* #1128: Add more cache in store to speed up reward calculation (@u2)
* #1361: Use `TransactionInfo` instead of `BlockInfo` (@u2)
* #1328: Method `get_cell_data` should use cache (@quake)


# [v0.18.2](https://github.com/nervosnetwork/ckb/compare/v0.18.0...v0.18.2) (2019-08-17)

### Bug Fixes

* #1407: Calculate transaction fees in order (@keroro520)
* #1411: Header provider index check (@zhangsoledad)
* #1412: Hardcode allow 34827 (@keroro520)

    This is a workaround for #1411 to keep the current testnet main chain valid. It will not go into future versions.

* #1420: Failed to sync for long forks (@keroro520)

# [v0.18.0](https://github.com/nervosnetwork/ckb/compare/v0.17.0...v0.18.0) (2019-08-10)

### Features

* #1351: Difficulty adjustment rfc version (@zhangsoledad)
* #1358: Add 10x faster miner (@kilb)

### Bug Fixes

* #1267: Header verifier with wrong header resolver (@u2)


# [v0.17.0](https://github.com/nervosnetwork/ckb/compare/v0.16.0...v0.17.0) (2019-07-27)

### Features

* #1119: Remove rules of special reserve blocks (@doitian)

    This is a breaking change: b:consensus, b:database

    For block 1~11, the reward target is genesis block. Genesis block must have the lock serialized in the cellbase witness, which is set to `bootstrap_lock`.

* #1125: Add `get_header` and `get_header_by_number` RPC methods (@TheWaWaR)
* #1094: Secondary miner issurance, split DAO as a separate contract (@xxuejie)

    This is a breaking change: b:consensus, b:database

* #1137: Remove output for bootstrap lock in genesis block (@doitian)

    The lock has already been written into the cellbase witness in the genesis block.

* #1165: Reference script code via dep cell's type hash (@xxuejie)

    This allows us to build a new paradigm that allows upgrading of
    scripts without affecting lock/type hash.

* #1203: Add bootnode mode (@driftluo)
* #1213: Remove `block_number` from API `BlockMedianTimeContext::block_median_time` (@keroro520)
* #1215: Alert notify script (@jjyr)
* #1212: Partition nonce for miners who use multi-threads (@yangby-cryptape)
* #1225: Log found block as info when stderr is not tty (@doitian)
* #1230: Use `tokio_threadpool::blocking` to handle heavy future task (@TheWaWaR)
* #1232: IBD with whitelist (@driftluo)
* #1220: Network flood control (@TheWaWaR)

    Do not send blocks to peer when session send buffer is full.

* #1211: Ban the peer when receive misbehave compact-block, add test for compact block process (@u2)
* #1258: Only allow default secp256k1 block assembler (@doitian)

    Unless start the node with `ckb run --ba-advanced`

* #1237: Cache `BLOCK_INVALID`/`BLOCK_VALID` status (@keroro520)
* #1246: Send a message to remote peer when disconnect (@TheWaWaR)
* #1274: Adjust max block interval to 30s (@doitian)

    This is a breaking change: b:consensus

* #1032 **storage:** Use flatbuffer instead of bincode in storage (@yangby-cryptape)
* #1301: Add RPC `get_cellbase_output_capacity_details` (@u2)

### Bug Fixes

* #1092: Random failure caused by dirty exit in RPC test (@doitian)

    Close the server before exit RPC test.

* #1100: Resolve compact block switch fork issue (@quake)
* #1101: Fix debug log state error (@driftluo)
* #1108: Potential error in alert version compare (@jjyr)

    As @keroro520 mentioned, there is a potential bug in case like: `"0.10.0" < "0.9.10"`

* #1117: Rpc test (@jjyr)
* #1127: Process orphan blocks when their parents were relayed (@keroro520)
* #1109: Mark failed dialing as feeler (@jjyr)
* #1135: Total difficulty comparison should include hash (@quake)
* #1139: Resolve fresh proposal txs checking bug (@quake)
* #1144: Prof tps exclude cellbase (@jjyr)
* #1150: Correct block number from `tx_pool_excutor` (@keroro520)

    NOTE: **This is a breaking change**

* #1196: Reserved only do nothing except for connect all reserved peers (@driftluo)

    Reserved only do nothing except for connect all reserved peers

* #1014: Locate blocks by hash (@keroro520)

    When calculates block_median_time, we need to locate the specific blocks.
    Using block_hash instead block_number to locate the specific blocks is more accurate.

    **BREAKING CHANGE**: The format of `TransactionMeta`  is changed, which is affected by `BlockInfo`

* #1149: Add cellset test and fix `new_overlay` (@u2)
* #1214: Reset `current_time` of block template (@keroro520)
* #1227: Should check tx from pool when the `short_id` set is not empty (@lerencao)
* #1226: Resolve rpc `remove_node` and network `report_peer` bug (@quake)

    we shouldn't call peer_registry `remove_peer ` before session was closed, it will be removed in disconnect event.

* #1238: Build.rs failed without git dir (@doitian)
* #1247: Skiplist test use `gen_range` the wrong way (@TheWaWaR)
* #1251: There is an incorrect deserialization in indexer (@yangby-cryptape)
* #1272: Clean status of new inserted block (@keroro520)

    fix: Clear the newly inserted block from block_status_map.

### Improvements

* #1072: Reveal network errors and involver handle it (@keroro520)

    * feat: Reveal network errors. Currently, when `CKBProtocolContext` receives an error from p2p, it only logs the error and doesn't return to the caller. I change to `CKBProtocolContext` return the error to the caller, and caller handles it.

    * perf: Short-circuiting break if occurs network error, `Synchronizer` responses `Blocks` and `Transactions`. This is the original intention of this PR. To achieve it, I have to make  `CKBProtocolContext` reveals the network errors, which introduces most of the change code.

* #1073: Define a general Filter struct (@keroro520)
* #1098: Avoid re-requesting blocks in orphan pool (@keroro520)
* #1126: Skip stored block processing (@quake)
* #1140: Shrink chain state lock scope in relayer (@quake)
* #1168: Use BlockStatus to filter things (@keroro520)


# [v0.16.0](https://github.com/nervosnetwork/ckb/compare/v0.15.0...v0.16.0) (2019-07-13)

### Features

* #1151: Allow providing extra sentry info in config (@doitian)

### Bug Fixes

* #1183: Ibd should remain false once returned false (@quake)
* #1190: Fix sync logic (@driftluo, @quake)
* #1189: Fix debug log state error (@driftluo, @quake)
* #1185: Resolve fresh proposal txs checking bug (@quake)
* #1176: Use tip header to ignore compact block (@TheWaWaR)
* #1179: Random failure caused by dirty exit in RPC test (@doitian)

    Close the server before exit RPC test.

* #1195: Hotfix rc0.15 (@zhangsoledad)

    * fix fetch invalid block
    * fix response invalid block
    * fix repeat process block overwrite block ext

* #1164: Ban peer when validate received block failed (@TheWaWaR)
* #1167: Proposal reward calculate consistency (@zhangsoledad)
* #1169: Only sync with outbound peer in IBD mode (@quake)
* #1143: `get_ancestor` is inconsistent (@zhangsoledad)
* #1148: Sync block download filter (@zhangsoledad)

    Node should fetch download from all peers which have better chain,
    no matter it's known best's ancestor or not.


# [v0.15.0](https://github.com/nervosnetwork/ckb/compare/v0.14.0...v0.15.0) (2019-06-29)

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
* #1044: `Peer_store` time calculation overflow (@jjyr)
* #1077: Resolve `ChainSpec` script deserialize issue (@quake)
* #1076: `CellSet` is inconsistent in memory and storage (@zhangsoledad)
* #1025: Miner time interval panic (@quake)

### Improvements

* #938: Remove low S check from util-crypto (@jjyr)
* #959: Remove redundant interface from ChainProvider (@zhangsoledad)
* #970 **sync:** Fix get ancestor performance issue (@TheWaWaR)
* #976: Flatten network protocol state into `SyncSharedState` (@keroro520)
* #1051: Get `tip_header` from store instead of from `chain_state` (@jjyr)
* #971: Abstract data loader layer to decouple `ckb-script` and `ckb-store` (@jjyr)
* #994: Wrap lock methods to avoid locking a long time (@keroro520)

### Bug Fixes

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


# [v0.13.0](https://github.com/nervosnetwork/ckb/compare/v0.12.0...v0.13.0) (2019-06-01) rylai-v2

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

### Bug Fixes

* #812: Prof should respect script config (@xxuejie)
* #810: Discard invalid orphan blocks (@keroro520)

    When accepts a new block, its descendants should be accepted too if valid. So if an error occurs when we try to accept its descendants, the descendants are invalid.

* #850: Ensure EBREAK has proper cycle set (@xxuejie)

    This is a breaking change: b:consensus

    This is a bug reported by @yangby-cryptape. Right now we didn't assign proper cycles for EBREAK, which might lead to potential bugs.

* #886: Integration test cycle calc (@zhangsoledad)
* fix: Cuckoo cycle verification bug (@yangby-cryptape)
* #825: Filter out p2p field in address (@TheWaWaR)
* #826: Ban peer deadlock (@TheWaWaR)
* #829 **docker:** Fix docker problems found in rylai (@doitian)

    - avoid dirty flag in version info
    - bind rpc on 0.0.0.0 in docker
    - fix docker files permissions


### Improvements

* #832: `peer_store` db::PeerInfoDB interface (@jjyr)


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
- #414: clear tx verify cache when chain reorg (@zhangsoledad)


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
- The new features tx valid since (#372) and removal of version from Script (#422) have changed the core data structure. They affect both RPC and the P2P messages flatbuffers schema.


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


# [v0.5.0](https://github.com/nervosnetwork/ckb/compare/v0.4.0...v0.5.0) (2019-02-11)

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
